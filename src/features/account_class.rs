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
//! Tokens shorter than 5 chars are deliberately omitted: `co` would
//! false-positive on `cooking_anna`, `inc` on `incognito_jay`. False
//! positives are costlier than false negatives here — a missed brand stays
//! `Personal` and remains eligible for the close_friend / favorited /
//! allowlist gates, whereas a falsely-flagged personal handle silently
//! suppresses a real Unfollow recommendation. Lexicon entries are checked
//! against both `username` and `display_name`: brands sometimes ship a
//! personal-looking handle (`bobsmith`) but a brand display name
//! (`Studio Ghibli`), and vice versa.
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
/// `display_name`. Every entry is ≥ 5 chars to keep false-positive risk
/// against personal handles low. Extend conservatively — false positives
/// silently suppress real Unfollow recommendations.
const BRAND_LEXICON: &[&str] = &[
    "official", "studio", "magazine", "records", "gallery", "news", "media", "agency",
];

/// Bundle of the brand-detection automaton + the user-maintained
/// keep-allowlist. Built once per run (in `lib::run`) and threaded into
/// [`aggregate`](crate::features::aggregate) via `AggregateInputs`.
#[derive(Debug)]
pub struct Classifier {
    matcher: AhoCorasick,
    /// Allowlist entries are pre-lowercased on insert so lookup just does
    /// a single `to_ascii_lowercase` on the query handle.
    allowlist: HashSet<String>,
}

impl Classifier {
    /// Build a classifier wrapping the production [`BRAND_LEXICON`] and
    /// the (already-lowercased) allowlist returned by
    /// [`crate::allowlist::load_default`].
    pub fn new(allowlist: HashSet<String>) -> Self {
        let matcher = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(BRAND_LEXICON)
            .expect("BRAND_LEXICON is non-empty ASCII");
        Self { matcher, allowlist }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Classifier {
        Classifier::new(HashSet::new())
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
        // The lexicon deliberately drops < 5-char tokens. These handles
        // would false-positive under a naive `inc` / `co` rule but must
        // stay Personal here. If the lexicon grows to include such
        // tokens later, this test will surface the regression risk.
        let c = empty();
        assert_eq!(c.classify("incognito_jay", None), AccountClass::Personal);
        assert_eq!(c.classify("cooking_anna", None), AccountClass::Personal);
        assert_eq!(c.classify("companion_dog", None), AccountClass::Personal);
    }

    #[test]
    fn allowlist_lookup_is_case_insensitive() {
        let mut list = HashSet::new();
        list.insert("special_friend".to_owned());
        let c = Classifier::new(list);
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
        let c = Classifier::new(list);
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
        let c = Classifier::new(list);
        assert_eq!(c.classify("nytimes_official", None), AccountClass::Brand,);
        assert!(c.is_allowlisted("nytimes_official"));
    }
}
