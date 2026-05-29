//! Brand-detection heuristic + keep-allowlist gate.
//!
//! DESIGN.md "Account-class heuristic" gates the `unfollow` recommendation
//! on `account_class == Personal`. This module owns the username /
//! display-name lexicon match that promotes followees out of `Personal`,
//! and the user-maintained never-unfollow override the lexicon can't be
//! trusted on.
//!
//! ## Lexicon
//!
//! High-precision substring matching against a curated [`BRAND_LEXICON`].
//! Aho-corasick (one automaton, single pass over each input) instead of N
//! independent `str::contains` calls — DESIGN.md "Account-class heuristic"
//! prescribes the choice. Matching is ASCII-case-insensitive on both sides.
//!
//! Token length floor is 4 chars (relaxed from 5 in round 4 — see
//! `docs/TUNING.md`). The active rule is **empirical**: a token is added
//! only after a 0-false-positive grep against the real export's
//! followee list. 3-char tokens like `bar` and `art` are deliberately
//! deferred — they need word-boundary semantics on the matcher to be
//! safe (`klaras_bar` matches but `barbara` doesn't), which is a
//! structural matcher change not justified by the marginal recall gain
//! at current scale. False positives are costlier than false negatives
//! here — a missed brand stays `Personal` and remains eligible for the
//! close_friend / favorited / allowlist gates, whereas a falsely-flagged
//! personal handle silently suppresses a real Unfollow recommendation.
//! Lexicon entries are checked against both `username` and
//! `display_name`: brands sometimes ship a personal-looking handle
//! (`bobsmith`) but a brand display name (`Studio Ghibli`), and vice
//! versa.
//!
//! ## Allowlist
//!
//! `config/keep_allowlist.txt` is the user-maintained list of handles that
//! must never bucket as Unfollow regardless of engagement — brands the
//! heuristic misses, public figures, and personal accounts the export
//! under-represents (sparse signal, out-of-band relationship). It is NOT a
//! classification source: allowlisted handles stay `Personal` at the
//! [`AccountClass`] level so the column doesn't misrepresent a close
//! friend's profile. The override surfaces as
//! [`AccountFeatures::is_keep_allowlisted`](crate::features::AccountFeatures)
//! parallel to `is_close_friend` / `is_favorited`; the scoring layer's
//! [`assign_bucket`](crate::scoring) folds it into the Unfollow-gate check.

use std::collections::HashSet;

use aho_corasick::AhoCorasick;

use crate::features::AccountClass;

/// Curated lexicon for brand-suffix detection on `username` /
/// `display_name`. Every entry is ≥ 4 chars and has been verified
/// 0-false-positive against the real export's followee list (see
/// `docs/TUNING.md` round 4 for the per-token audit). Extend
/// conservatively — false positives silently suppress real Unfollow
/// recommendations.
const BRAND_LEXICON: &[&str] = &[
    // Initial 8 tokens (round-3 brand-gate slice).
    "official", "studio", "magazine", "records", "gallery", "news", "media", "agency",
    // Round-4 expansion, 5+ chars (0 export-FPs).
    "books", "press", "games", "store", "comics",
    // Round-4 expansion, 4 chars (same 0-export-FP guard; the relaxed
    // floor is justified per-token in docs/TUNING.md).
    "zine", "shop", "cafe",
];

/// Bundle of the brand-detection automaton + the two user-maintained
/// handle lists (keep-allowlist + drop-list). Built once per run (in
/// `lib::run`) and threaded into
/// [`aggregate`](crate::features::aggregate) via `AggregateInputs`.
#[derive(Debug)]
pub struct Classifier {
    matcher: AhoCorasick,
    /// Allowlist entries are pre-lowercased on insert so lookup just does
    /// a single `to_ascii_lowercase` on the query handle.
    allowlist: HashSet<String>,
    /// Drop-list entries, same lowercased-on-insert convention as
    /// `allowlist`. The exact inverse signal: forces Unfollow.
    drop_list: HashSet<String>,
}

impl Classifier {
    /// Build a classifier wrapping the production [`BRAND_LEXICON`] and
    /// the (already-lowercased) handle lists from
    /// [`crate::allowlist::load_default`] (keep) and
    /// [`crate::allowlist::load_drop_list`] (drop).
    pub fn new(allowlist: HashSet<String>, drop_list: HashSet<String>) -> Self {
        let matcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(BRAND_LEXICON)
            .expect("BRAND_LEXICON is non-empty ASCII");
        Self {
            matcher,
            allowlist,
            drop_list,
        }
    }

    /// Classify a followee by matching the lexicon against the handle and
    /// (when available) the resolved display name. Either surface hitting
    /// the lexicon promotes to [`AccountClass::Brand`].
    pub fn classify(&self, username: &str, display_name: Option<&str>) -> AccountClass {
        if self.matcher.is_match(username) {
            return AccountClass::Brand;
        }
        if let Some(name) = display_name
            && self.matcher.is_match(name)
        {
            return AccountClass::Brand;
        }
        AccountClass::Personal
    }

    /// Check the keep-allowlist. Case-insensitive; cheap on real handles
    /// (Instagram disallows non-ASCII / uppercase, so the lowercase form
    /// equals the input).
    pub fn is_allowlisted(&self, username: &str) -> bool {
        self.allowlist.contains(&username.to_ascii_lowercase())
    }

    /// Check the drop-list. Mirror of [`is_allowlisted`](Self::is_allowlisted)
    /// against the inverse-signal set.
    pub fn is_drop_listed(&self, username: &str) -> bool {
        self.drop_list.contains(&username.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Classifier {
        Classifier::new(HashSet::new(), HashSet::new())
    }

    #[test]
    fn lexicon_hit_on_username_promotes_to_brand() {
        let c = empty();
        assert_eq!(c.classify("nytimes_official", None), AccountClass::Brand);
        assert_eq!(c.classify("vogue_magazine", None), AccountClass::Brand);
        assert_eq!(c.classify("warner_records", None), AccountClass::Brand);
        assert_eq!(c.classify("creative_studio", None), AccountClass::Brand);
    }

    #[test]
    fn no_lexicon_hit_stays_personal() {
        let c = empty();
        assert_eq!(c.classify("alice_synth", None), AccountClass::Personal);
        assert_eq!(
            c.classify("bob_synth", Some("Bob Synth")),
            AccountClass::Personal,
        );
        // Empty username with no display name → still Personal (not Brand).
        // The aggregator already filters empty handles before reaching the
        // classifier; this assertion pins the classifier's own behaviour
        // for completeness.
        assert_eq!(c.classify("", None), AccountClass::Personal);
    }

    #[test]
    fn empty_display_name_does_not_promote_to_brand() {
        // The resolver returns `Option<&str>` but the aggregator's call site
        // doesn't pre-filter `Some("")`. Defends against an aho-corasick
        // version where `is_match("")` ever returns `true` — today it
        // doesn't, but pin it so a future bump can't silently flip every
        // empty-display-name followee into Brand.
        let c = empty();
        assert_eq!(c.classify("alice", Some("")), AccountClass::Personal);
    }

    #[test]
    fn lexicon_hit_via_display_name_only() {
        // A brand can ship a personal-looking handle but a brand display
        // name — must still classify as Brand. This is the load-bearing
        // reason the classifier takes both surfaces.
        let c = empty();
        assert_eq!(
            c.classify("bobsmith", Some("Studio Ghibli")),
            AccountClass::Brand,
        );
        assert_eq!(c.classify("xyz", Some("Daily News")), AccountClass::Brand);
    }

    #[test]
    fn matching_is_ascii_case_insensitive() {
        let c = empty();
        assert_eq!(c.classify("NYTimes_OFFICIAL", None), AccountClass::Brand);
        assert_eq!(c.classify("bob", Some("DAILY NEWS")), AccountClass::Brand);
    }

    #[test]
    fn short_tokens_excluded_to_avoid_false_positives() {
        // The lexicon's floor is 4 chars (relaxed from 5 in round 4 —
        // see TUNING.md). 3-char tokens like `inc` / `co` / `bar` / `art`
        // would false-positive on these personal handles, so they stay
        // out until word-boundary semantics is on the matcher. If the
        // lexicon ever grows to include such a token under plain
        // substring matching, this test surfaces the regression.
        let c = empty();
        assert_eq!(c.classify("incognito_jay", None), AccountClass::Personal);
        assert_eq!(c.classify("cooking_anna", None), AccountClass::Personal);
        assert_eq!(c.classify("companion_dog", None), AccountClass::Personal);
    }

    #[test]
    fn books_token_matches_real_handles() {
        // Round-4 addition. Real-export brand hits: kona_books_,
        // kjartbooks, bucksbooks, japaneseavantgardebooks.
        let c = empty();
        assert_eq!(c.classify("kona_books_", None), AccountClass::Brand);
        assert_eq!(
            c.classify("japaneseavantgardebooks", None),
            AccountClass::Brand,
        );
    }

    #[test]
    fn press_token_matches_real_handles() {
        // Round-4 addition. Covers both standalone-suffix
        // (`blackdragonpress`) and dotted-prefix (`press.centri`) shapes.
        let c = empty();
        assert_eq!(c.classify("blackdragonpress", None), AccountClass::Brand);
        assert_eq!(c.classify("press.centri", None), AccountClass::Brand);
    }

    #[test]
    fn games_token_matches_real_handles() {
        // Round-4 addition. Common video-game-publisher suffix.
        let c = empty();
        assert_eq!(c.classify("specialreservegames", None), AccountClass::Brand,);
        assert_eq!(c.classify("limitedrungames", None), AccountClass::Brand);
    }

    #[test]
    fn comics_and_store_tokens_match_real_handles() {
        // Round-4 additions, single-hit tokens grouped to keep the test
        // file flat. `floatingworldcomics` and `eagletokyostore` are
        // each the only real-export hit for their token.
        let c = empty();
        assert_eq!(c.classify("floatingworldcomics", None), AccountClass::Brand);
        assert_eq!(c.classify("eagletokyostore", None), AccountClass::Brand);
    }

    #[test]
    fn four_char_tokens_zine_shop_cafe_match_real_handles() {
        // Round-4 addition: the 4-char tokens. Pinning each because the
        // 4-char floor itself is the load-bearing relaxation — a future
        // maintainer who shortens this further needs to add word
        // boundaries, not just bump the length.
        let c = empty();
        assert_eq!(c.classify("danarti_zine", None), AccountClass::Brand);
        assert_eq!(c.classify("blackdogshoptbilisi", None), AccountClass::Brand);
        assert_eq!(c.classify("estupendacafebar", None), AccountClass::Brand);
    }

    #[test]
    fn known_substring_false_positives_are_pinned() {
        // The current lexicon — `news`, `gallery`, `media` (and the rest)
        // — uses pure substring match, so a handle that contains any of
        // those letter-runs gets classified as Brand even when it isn't
        // one. We accept this cost: a false-positive Brand demotes
        // Unfollow → Review (manual triage), not silent suppression.
        // The real-export `butt_news` case is the documented example
        // in TUNING.md. These three are empirically-verified additional
        // matches; pinning them documents the surface so a future
        // lexicon edit can't silently change which false positives we
        // accept. If you make one of these stop matching, update both
        // the test AND TUNING.md.
        let c = empty();
        assert_eq!(c.classify("renewsletter", None), AccountClass::Brand);
        assert_eq!(c.classify("gallerymate", None), AccountClass::Brand);
        assert_eq!(c.classify("mediator_jake", None), AccountClass::Brand);
    }

    #[test]
    fn allowlist_lookup_is_case_insensitive() {
        let mut list = HashSet::new();
        list.insert("special_friend".to_owned());
        let c = Classifier::new(list, HashSet::new());
        assert!(c.is_allowlisted("special_friend"));
        assert!(c.is_allowlisted("SPECIAL_FRIEND"));
        assert!(c.is_allowlisted("Special_Friend"));
        assert!(!c.is_allowlisted("other_handle"));
    }

    #[test]
    fn allowlist_membership_does_not_change_class() {
        // The allowlist is a separate signal — `is_allowlisted` returns
        // `true` but `classify` keeps returning `Personal` unless the
        // lexicon also fires. A personal close-friend on the allowlist
        // must not be misrepresented as a Brand in the CSV.
        let mut list = HashSet::new();
        list.insert("special_friend".to_owned());
        let c = Classifier::new(list, HashSet::new());
        assert_eq!(c.classify("special_friend", None), AccountClass::Personal);
        assert!(c.is_allowlisted("special_friend"));
    }

    #[test]
    fn allowlist_and_brand_coexist() {
        // A brand handle the user also allowlists: classifier stamps
        // Brand AND reports allowlisted. Both signals fire independently
        // — scoring's Unfollow gate is satisfied to bump to Review by
        // either one alone, so the redundancy is harmless.
        let mut list = HashSet::new();
        list.insert("nytimes_official".to_owned());
        let c = Classifier::new(list, HashSet::new());
        assert_eq!(c.classify("nytimes_official", None), AccountClass::Brand,);
        assert!(c.is_allowlisted("nytimes_official"));
    }

    fn with_drop_list(handles: &[&str]) -> Classifier {
        let drop = handles.iter().map(|h| (*h).to_owned()).collect();
        Classifier::new(HashSet::new(), drop)
    }

    #[test]
    fn drop_list_lookup_is_case_insensitive() {
        // Mirror of `allowlist_lookup_is_case_insensitive` — the drop-list
        // is stored ASCII-lowercased, so lookups normalize the query.
        let c = with_drop_list(&["meaning_to_drop"]);
        assert!(c.is_drop_listed("meaning_to_drop"));
        assert!(c.is_drop_listed("MEANING_TO_DROP"));
        assert!(c.is_drop_listed("Meaning_To_Drop"));
        assert!(!c.is_drop_listed("other_handle"));
    }

    #[test]
    fn drop_list_membership_does_not_change_class() {
        // The drop-list is a scoring-gate signal, not a classification
        // source — `is_drop_listed` returns true but `classify` keeps
        // returning Personal unless the lexicon fires independently.
        let c = with_drop_list(&["meaning_to_drop"]);
        assert_eq!(c.classify("meaning_to_drop", None), AccountClass::Personal);
        assert!(c.is_drop_listed("meaning_to_drop"));
    }

    #[test]
    fn keep_and_drop_lists_are_independent() {
        // A classifier carrying both lists answers each lookup against its
        // own set — no cross-contamination between the two HashSets.
        let mut keep = HashSet::new();
        keep.insert("protected".to_owned());
        let mut drop = HashSet::new();
        drop.insert("doomed".to_owned());
        let c = Classifier::new(keep, drop);
        assert!(c.is_allowlisted("protected"));
        assert!(!c.is_allowlisted("doomed"));
        assert!(c.is_drop_listed("doomed"));
        assert!(!c.is_drop_listed("protected"));
    }
}
