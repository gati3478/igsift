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
//!   damaging here than missing attribution. The 21 entries with
//!   non-empty `Username` but empty `Name` (mostly `close_friends.json`
//!   entries) are dropped at build time so the empty-string key cannot
//!   become a phantom resolution path.
//!
//! The folder name `<inbox>/<thread>/` is **not** a usable bridge:
//! validation showed it is the participant's display name sanitized
//! (lowercased, spaces/punctuation stripped), not the handle. The 3 of 30
//! sampled followings whose folder prefix equaled the handle were
//! coincidences where the display name happens to equal the handle.

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
                let Some((name, handle)) = label_pair(entry) else {
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

fn label_pair(entry: &ShapeCEntry) -> Option<(&str, &str)> {
    // Both fields must be non-empty to be a usable bridge. The 2026-05-11
    // export ships 21 entries (mostly in `close_friends.json`) with a
    // non-empty `Username` but an empty `Name` — likely accounts that
    // never set a display name, so the handle IS the display name.
    // Including them as `("", handle)` would lump every empty-name entry
    // under the same `""` key, polluting the collision-detection logic
    // and creating a phantom resolution path no DM thread can hit
    // (display name `""` does not occur in participant lists).
    let mut name = None;
    let mut handle = None;
    for lv in &entry.label_values {
        match lv.label.as_deref() {
            Some("Name") => name = lv.value.as_deref().filter(|s| !s.is_empty()),
            Some("Username") => handle = lv.value.as_deref().filter(|s| !s.is_empty()),
            _ => {}
        }
    }
    Some((name?, handle?))
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
    fn ignores_entries_missing_either_field() {
        let entries = vec![
            entry(&[("Name", "Alice")]),     // no Username
            entry(&[("Username", "bob_h")]), // no Name
            entry(&[("Name", "Carol"), ("Username", "carol_h")]),
        ];
        let r = NameResolver::build(&[&entries]);
        assert_eq!(r.unique_name_count(), 1);
        assert_eq!(r.resolve("Carol"), Some("carol_h"));
        assert_eq!(r.resolve("Alice"), None);
    }

    #[test]
    fn empty_name_or_username_is_not_a_bridge() {
        // The 2026-05-11 export ships 21 entries with non-empty Username
        // but empty Name (mostly close_friends entries where the user
        // never set a display name). Treating those as ("", handle)
        // bridges would lump them all under the empty-string key —
        // polluting collision detection and creating an unreachable
        // resolution path.
        let entries = vec![
            entry(&[("Name", ""), ("Username", "handle_a")]),
            entry(&[("Name", ""), ("Username", "handle_b")]),
            entry(&[("Name", "Real Name"), ("Username", "")]),
            entry(&[("Name", "Bob Synth"), ("Username", "bob_h")]),
        ];
        let r = NameResolver::build(&[&entries]);
        assert_eq!(r.unique_name_count(), 1, "only Bob Synth should remain");
        assert_eq!(r.collision_count(), 0);
        assert_eq!(r.resolve("Bob Synth"), Some("bob_h"));
        assert_eq!(r.resolve(""), None);
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
