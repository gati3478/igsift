//! Bridge DM participant display names to Instagram handles.
//!
//! DM threads ship `participants[].name` and `messages[].sender_name` as
//! display names — never handles. Every other parser output keys by handle
//! (the username in `following.json`, the `Owner.Username` in
//! `liked_posts.json`, etc.). The aggregator needs a `display_name ↔
//! handle` join to attribute DM signals to a followed account.
//!
//! The only export-internal bridge is the seven `label_values` files —
//! `close_friends.json`, `profiles_you've_favorited.json`,
//! `blocked_profiles.json`, `restricted_profiles.json`,
//! `recently_unfollowed_profiles.json`, `removed_suggestions.json`,
//! `hide_story_from.json`. Each [`ShapeCEntry`] there carries BOTH a
//! `Name` label and a `Username` label at the outer `label_values` level.
//!
//! Two recon facts from the 2026-05-11 export shape this module:
//!
//! - **Partial coverage.** 217/581 (37%) of 1:1 DM threads resolve under
//!   the strict-collision policy below; the rest score from activity-side
//!   features (handle-keyed at source) only — DM features are sparse by
//!   design. An earlier Python recon reported 240 by overwriting on
//!   collision (last-write-wins, effectively guessing); the production
//!   resolver refuses that guess.
//! - **Name collisions.** 12 of the 281 unique display names map to more
//!   than one handle (e.g., `"Mike"` → {`bermudalckt`, `hairycub81`,
//!   `leahcim333`}). [`NameResolver::resolve`] returns `None` for any
//!   colliding name rather than guessing — wrong attribution is more
//!   damaging here than missing attribution.
//! - **No-display-name accounts.** ~23 entries (mostly `close_friends.json`)
//!   carry a non-empty `Username` but an empty `Name` — accounts that
//!   never set a display name. For these, IG emits the **handle** as the
//!   DM `sender_name` and `participants[].name`. [`NameResolver::build`]
//!   registers an identity mapping (`handle → handle`) so their threads
//!   resolve to themselves instead of being dropped. Earlier versions
//!   dropped these entries, silently losing whole threads (a 436-message
//!   close-friend thread, 277 inbound, reported as `dm=0`). The identity
//!   key is the handle, not `""`, so no phantom empty-string path is
//!   created; a real display name elsewhere that equals the handle still
//!   collides safely to `None`.
//!
//! The folder name `<inbox>/<thread>/` is **not** a usable bridge:
//! validation showed it is the participant's display name sanitized
//! (lowercased, spaces/punctuation stripped), not the handle. The handful
//! of sampled followings whose folder prefix equaled the handle were the
//! no-display-name case above (handle used everywhere), now handled via
//! the `sender_name`-side identity mapping rather than the folder name.

use std::collections::HashMap;

use crate::export::ShapeCEntry;

/// Bidirectional join between display name and Instagram handle, built
/// from `label_values` entries that carry both a `Name` and a `Username`.
///
/// See module docs for coverage and collision semantics.
#[derive(Debug, Default)]
pub struct NameResolver {
    /// Names with multiple distinct handles are kept under all of them so
    /// [`resolve`] can detect the collision and return `None`. Each
    /// `Vec` is short — the 2026-05-11 export's worst collision is 3.
    ///
    /// [`resolve`]: NameResolver::resolve
    name_to_handles: HashMap<String, Vec<String>>,
    /// Inverse direction for the CSV writer (`display_name` column). The
    /// collision policy mirrors the forward direction: a handle that
    /// appears with multiple distinct display names across the seven
    /// sources is treated as ambiguous — [`display_name_for`] returns
    /// `None` and the CSV emits an empty string rather than guessing
    /// one spelling. Handles are unique on Instagram, so collisions here
    /// are rare (a handle whose owner edited their display name between
    /// when two different `label_values` files were generated).
    ///
    /// [`display_name_for`]: NameResolver::display_name_for
    handle_to_names: HashMap<String, Vec<String>>,
}

impl NameResolver {
    /// Build the resolver from the union of `label_values` entry slices.
    ///
    /// Each source is a slice of already-parsed [`ShapeCEntry`]s from one
    /// of the seven relationship-flag files. Entries that don't carry
    /// both a `Name` and a `Username` are skipped — they don't bridge.
    pub fn build(sources: &[&[ShapeCEntry]]) -> Self {
        let mut name_to_handles: HashMap<String, Vec<String>> = HashMap::new();
        let mut handle_to_names: HashMap<String, Vec<String>> = HashMap::new();
        for source in sources {
            for entry in *source {
                let (name, handle) = label_fields(entry);
                let Some(handle) = handle else {
                    continue;
                };
                let Some(name) = name else {
                    // No display name: IG emits the handle as
                    // participants[].name and sender_name, so register an
                    // identity mapping (handle → handle) on the forward
                    // side only. Keyed on the handle (not ""), so there is
                    // no phantom empty-string path; handles are unique, so
                    // the only way this key collides is a real display
                    // name elsewhere that equals the handle — which
                    // correctly surfaces as a collision (resolve → None).
                    // The reverse side is left untouched: the account has
                    // no display name, so the CSV column stays empty
                    // rather than echoing the handle back as a name.
                    let handles = name_to_handles.entry(handle.to_owned()).or_default();
                    if !handles.iter().any(|h| h == handle) {
                        handles.push(handle.to_owned());
                    }
                    continue;
                };
                // Fix IG's UTF-8-as-Latin-1 mojibake on the display-name
                // side. The DM-side capture points (participants,
                // sender_name) apply the same fix at parse time, so
                // joins downstream still match. Handles (and their
                // Latin-1-safe Instagram-rule character set) need no
                // repair.
                let name = crate::text::fix_mojibake(name);
                let names = handle_to_names.entry(handle.to_owned()).or_default();
                if !names.iter().any(|n| n.as_str() == name.as_ref()) {
                    names.push(name.clone().into_owned());
                }
                let handles = name_to_handles.entry(name.into_owned()).or_default();
                if !handles.iter().any(|h| h == handle) {
                    handles.push(handle.to_owned());
                }
            }
        }
        Self {
            name_to_handles,
            handle_to_names,
        }
    }

    /// Resolve a display name to a single handle.
    ///
    /// Returns `None` when the name is unknown OR when it maps to more
    /// than one handle — collisions are NOT guessed. The aggregator
    /// should treat the unresolvable thread as having no attributable
    /// DM partner rather than misattribute it.
    pub fn resolve(&self, name: &str) -> Option<&str> {
        let handles = self.name_to_handles.get(name)?;
        if handles.len() == 1 {
            Some(handles[0].as_str())
        } else {
            None
        }
    }

    /// Inverse of [`resolve`]: handle → display name for the CSV writer.
    ///
    /// `None` when the handle is unknown OR when the same handle appears
    /// with multiple distinct names across the source files (rare —
    /// requires the user to have edited their display name between
    /// snapshots). Same "no guessing" posture as the forward direction.
    ///
    /// [`resolve`]: NameResolver::resolve
    pub fn display_name_for(&self, handle: &str) -> Option<&str> {
        let names = self.handle_to_names.get(handle)?;
        if names.len() == 1 {
            Some(names[0].as_str())
        } else {
            None
        }
    }

    /// Number of distinct display names known to the resolver. Includes
    /// names that collide — they are still "known", just unresolvable.
    pub fn unique_name_count(&self) -> usize {
        self.name_to_handles.len()
    }

    /// Number of display names that map to ≥ 2 distinct handles —
    /// unresolvable per the collision policy.
    pub fn collision_count(&self) -> usize {
        self.name_to_handles
            .values()
            .filter(|v| v.len() > 1)
            .count()
    }
}

/// Extract the `(Name, Username)` pair from a relationship-flag entry,
/// each empty-filtered to `None`.
///
/// A `Username` with no `Name` is the no-display-name case: the 2026-05-11
/// export ships ~23 such entries (mostly in `close_friends.json`) for
/// accounts that never set a display name. IG then emits the *handle* as
/// the DM `sender_name` / `participants[].name`, so [`NameResolver::build`]
/// registers an identity mapping for them rather than dropping them — the
/// fix for the silently-dropped DM threads (e.g. a 436-message thread read
/// as `dm=0`). The caller, not this function, applies that policy.
fn label_fields(entry: &ShapeCEntry) -> (Option<&str>, Option<&str>) {
    let mut name = None;
    let mut handle = None;
    for lv in &entry.label_values {
        match lv.label.as_deref() {
            Some("Name") => name = lv.value.as_deref().filter(|s| !s.is_empty()),
            Some("Username") => handle = lv.value.as_deref().filter(|s| !s.is_empty()),
            _ => {}
        }
    }
    (name, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::ShapeCLabelValue;

    fn entry(pairs: &[(&str, &str)]) -> ShapeCEntry {
        ShapeCEntry {
            fbid: None,
            timestamp: None,
            label_values: pairs
                .iter()
                .map(|(label, value)| ShapeCLabelValue {
                    label: Some((*label).to_owned()),
                    value: Some((*value).to_owned()),
                    title: None,
                    dict: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn resolves_unique_name() {
        let entries = vec![entry(&[
            ("Name", "Alice Synth"),
            ("Username", "alice_handle"),
        ])];
        let r = NameResolver::build(&[&entries]);
        assert_eq!(r.resolve("Alice Synth"), Some("alice_handle"));
        assert_eq!(r.resolve("unknown"), None);
        assert_eq!(r.unique_name_count(), 1);
        assert_eq!(r.collision_count(), 0);
    }

    #[test]
    fn collision_returns_none() {
        // Same name from two sources, two different handles — must NOT
        // guess one. Misattribution is worse than missing attribution.
        let entries = vec![
            entry(&[("Name", "Mike"), ("Username", "mike_a")]),
            entry(&[("Name", "Mike"), ("Username", "mike_b")]),
        ];
        let r = NameResolver::build(&[&entries]);
        assert_eq!(
            r.resolve("Mike"),
            None,
            "collision must surface as None, not guess",
        );
        assert_eq!(r.collision_count(), 1);
        assert_eq!(r.unique_name_count(), 1);
    }

    #[test]
    fn aggregates_across_multiple_sources() {
        // Mirrors the real-world wiring: each of the seven label_values
        // files is a separate slice; the resolver unions them.
        let close_friends = vec![entry(&[("Name", "Alice"), ("Username", "alice_h")])];
        let favorited = vec![entry(&[("Name", "Bob"), ("Username", "bob_h")])];
        let blocked = vec![entry(&[("Name", "Spam"), ("Username", "spam_h")])];
        let r = NameResolver::build(&[&close_friends, &favorited, &blocked]);
        assert_eq!(r.unique_name_count(), 3);
        assert_eq!(r.resolve("Alice"), Some("alice_h"));
        assert_eq!(r.resolve("Bob"), Some("bob_h"));
        assert_eq!(r.resolve("Spam"), Some("spam_h"));
    }

    #[test]
    fn entry_without_username_is_ignored_without_name_is_identity() {
        let entries = vec![
            entry(&[("Name", "Alice")]),     // no Username → can't bridge
            entry(&[("Username", "bob_h")]), // no Name → identity mapping
            entry(&[("Name", "Carol"), ("Username", "carol_h")]),
        ];
        let r = NameResolver::build(&[&entries]);
        // Carol (full bridge) + bob_h (identity) are known; Alice is not.
        assert_eq!(r.unique_name_count(), 2);
        assert_eq!(r.resolve("Carol"), Some("carol_h"));
        assert_eq!(r.resolve("bob_h"), Some("bob_h"));
        assert_eq!(r.resolve("Alice"), None);
    }

    #[test]
    fn empty_name_resolves_handle_to_itself() {
        // When a DM counterparty never set an Instagram display name, IG
        // emits the *handle* in participants[].name and sender_name. The
        // matching label_values entry then carries a Username but an empty
        // Name. Register an identity mapping (handle → handle) so the
        // thread resolves to itself instead of being dropped — the
        // bubblegumflavoredhippo case: a 436-message close-friend thread
        // (277 inbound) silently reported as dm=0. Keyed on the handle, so
        // no phantom empty-string path; handles are unique, so no
        // collision risk from this side.
        let entries = vec![entry(&[("Name", ""), ("Username", "hippo_handle")])];
        let r = NameResolver::build(&[&entries]);
        assert_eq!(r.resolve("hippo_handle"), Some("hippo_handle"));
        // The account genuinely has no display name → the reverse
        // direction stays None so the CSV display_name column stays empty
        // rather than fabricating the handle as a display name.
        assert_eq!(r.display_name_for("hippo_handle"), None);
        // No phantom empty-string resolution path (the original concern).
        assert_eq!(r.resolve(""), None);
    }

    #[test]
    fn identity_mapping_collides_safely() {
        // If some other account's real display name equals this handle
        // string, the identity key collides — resolve returns None (no
        // guess), preserving "misattribution is worse than missing
        // attribution".
        let entries = vec![
            entry(&[("Name", ""), ("Username", "ambiguous")]),
            entry(&[("Name", "ambiguous"), ("Username", "other_handle")]),
        ];
        let r = NameResolver::build(&[&entries]);
        assert_eq!(r.resolve("ambiguous"), None);
    }

    #[test]
    fn empty_username_is_not_a_bridge() {
        // The mirror case stays excluded: a Name with no Username can't
        // bridge (nothing to map to). An empty Name with a Username now
        // registers an identity mapping instead of being dropped.
        let entries = vec![
            entry(&[("Name", ""), ("Username", "handle_a")]), // identity now
            entry(&[("Name", "Real Name"), ("Username", "")]), // no handle → skip
            entry(&[("Name", "Bob Synth"), ("Username", "bob_h")]),
        ];
        let r = NameResolver::build(&[&entries]);
        assert_eq!(r.resolve("Real Name"), None, "no Username → no bridge");
        assert_eq!(r.resolve("Bob Synth"), Some("bob_h"));
        assert_eq!(
            r.resolve("handle_a"),
            Some("handle_a"),
            "empty Name → identity",
        );
        assert_eq!(r.collision_count(), 0);
        assert_eq!(r.resolve(""), None);
    }

    #[test]
    fn mojibake_repair_joins_dm_side_to_label_side() {
        // The whole point of the mojibake fix is that the
        // display-name side (label_values Name → NameResolver
        // build) and the DM side (sender_name / participants in
        // export.rs) BOTH get repaired so the join still matches.
        // This test exercises the join end-to-end: a mojibake'd
        // Name in label_values resolves to the handle when looked
        // up by the *repaired* form (which is what DM thread
        // parsing will yield after applying fix_mojibake at
        // parse time). Drop the fix from either side and the
        // assertion below fails.
        //
        // Mojibake form of "Hüseyin" (UTF-8 c3 bc misread as two
        // Latin-1 chars Ã ¼). Built byte-exact so the test is
        // invariant to source-file encoding.
        let mojibake: String = [b'H', 0xc3, 0xbc, b's', b'e', b'y', b'i', b'n']
            .iter()
            .map(|&b| b as char)
            .collect();
        let entries = vec![entry(&[
            ("Name", mojibake.as_str()),
            ("Username", "hgurell"),
        ])];
        let r = NameResolver::build(&[&entries]);

        // The repaired form ("Hüseyin") is what export.rs will
        // produce for participants[].name and sender_name — so
        // looking up under that form must succeed.
        assert_eq!(r.resolve("Hüseyin"), Some("hgurell"));
        // The mojibake'd form must NOT resolve — otherwise the
        // fix isn't actually applied on the label side and we'd
        // get false-positive joins from un-repaired raw bytes.
        assert_eq!(r.resolve(&mojibake), None);
        // Reverse direction (handle → repaired display name) for
        // the CSV/MD/HTML output: must surface the clean form.
        assert_eq!(r.display_name_for("hgurell"), Some("Hüseyin"));
    }

    #[test]
    fn duplicate_pair_across_sources_does_not_collide() {
        // Same (name, handle) pair appearing in two files (e.g., a close
        // friend who is also favorited) must not produce a collision.
        let close_friends = vec![entry(&[("Name", "Alice"), ("Username", "alice_h")])];
        let favorited = vec![entry(&[("Name", "Alice"), ("Username", "alice_h")])];
        let r = NameResolver::build(&[&close_friends, &favorited]);
        assert_eq!(r.resolve("Alice"), Some("alice_h"));
        assert_eq!(r.collision_count(), 0);
    }

    #[test]
    fn display_name_for_returns_unique_name() {
        let entries = vec![
            entry(&[("Name", "Alice Synth"), ("Username", "alice_h")]),
            entry(&[("Name", "Bob Synth"), ("Username", "bob_h")]),
        ];
        let r = NameResolver::build(&[&entries]);
        assert_eq!(r.display_name_for("alice_h"), Some("Alice Synth"));
        assert_eq!(r.display_name_for("bob_h"), Some("Bob Synth"));
        assert_eq!(r.display_name_for("unknown_h"), None);
    }

    #[test]
    fn display_name_for_returns_none_when_handle_has_multiple_names() {
        // A handle whose owner edited their display name between snapshot
        // files surfaces in two sources with different `Name` values.
        // Same "no guessing" posture as the forward direction: collision
        // returns None rather than picking one spelling arbitrarily.
        let close_friends = vec![entry(&[("Name", "Old Name"), ("Username", "alice_h")])];
        let favorited = vec![entry(&[("Name", "New Name"), ("Username", "alice_h")])];
        let r = NameResolver::build(&[&close_friends, &favorited]);
        assert_eq!(
            r.display_name_for("alice_h"),
            None,
            "handle with two distinct names must surface as None",
        );
        // The forward direction still resolves both names (each maps
        // uniquely back to the same handle).
        assert_eq!(r.resolve("Old Name"), Some("alice_h"));
        assert_eq!(r.resolve("New Name"), Some("alice_h"));
    }
}
