//! Per-account feature aggregation — handle-keyed half (slice 7A).
//!
//! Folds the parsed [`crate::export`] outputs into one [`AccountFeatures`]
//! record per followed account. Scope is the followings set per
//! [`docs/DESIGN.md`](../../../docs/DESIGN.md) ("the ranking covers accounts
//! **you follow**"); `is_blocked` and `recently_unfollowed` accounts are
//! hard-excluded at input — they never appear in the output, the two
//! eponymous fields are always `false`.
//!
//! Slice 7A landed:
//! - boolean flags from the seven `label_values` files (outer-level `Username`,
//!   not the nested `Owner.Username` used by activity files),
//! - `follow_tenure_days` from `FollowingEntry.followed_at`,
//! - raw (not yet decay-weighted) activity counts: `likes_given`,
//!   `comments_given`, `story_interactions_out`, `stories_viewed`,
//!   `saved_their_content`.
//!
//! Slice 7B will populate the DM features (defaulted to zero/`None` here),
//! apply exponential decay per `config/scoring.toml` `[decay]`, and emit the
//! `*_90d` / `*_180d` windowed counts for the CSV columns. DESIGN.md is
//! explicit that the decay-weighted score inputs and the windowed
//! human-readable counts are *different aggregations*; both live in this
//! module once 7B lands.
//!
//! Honest-counting posture inherited from the parsers: an activity entry
//! whose target handle is not in the followings set is silently dropped at
//! aggregation time, mirroring "filter to followings is the scope" from
//! DESIGN.md. The structural unit tests below pin that semantic.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;

use crate::export::{CommentEntry, FollowingEntry, ShapeAEntry, ShapeCEntry, owner_username};

/// One row of per-account features. Slice 7A populates the handle-keyed
/// fields; DM-derived fields are defaulted to zero / `None` and filled in
/// slice 7B. See [`docs/DESIGN.md`](../../../docs/DESIGN.md) ("Per-account
/// features") for the feature semantics.
#[derive(Debug, Clone)]
pub struct AccountFeatures {
    pub username: String,
    pub follow_tenure_days: Option<u32>,

    pub is_close_friend: bool,
    pub is_favorited: bool,
    pub is_blocked: bool,
    pub is_restricted: bool,
    pub is_hide_story_from: bool,
    pub is_removed_suggestion: bool,
    pub recently_unfollowed: bool,

    pub likes_given: u32,
    pub comments_given: u32,
    pub story_interactions_out: u32,
    pub stories_viewed: u32,
    pub saved_their_content: u32,

    pub dm_messages_total: u32,
    pub dm_recency_days: Option<u32>,
    pub dm_balance: Option<f32>,
    pub dm_reactions_given: u32,
    pub dm_reactions_received: u32,
    pub inbound_dm_request: bool,
}

/// Borrowed bundle of every parser output the aggregator consumes.
///
/// Wraps the 20-odd parser outputs into one parameter so [`aggregate`] stays
/// callable without a long positional argument list. Lifetime `'a` ties each
/// borrowed slice to the same scope — the caller (`lib::run`) owns the
/// underlying `Vec`s for the duration of the aggregation pass.
#[derive(Debug)]
pub struct AggregateInputs<'a> {
    pub followings: &'a [FollowingEntry],

    pub close_friends: &'a [ShapeCEntry],
    pub favorited: &'a [ShapeCEntry],
    pub blocked: &'a [ShapeCEntry],
    pub restricted: &'a [ShapeCEntry],
    pub hide_story_from: &'a ShapeCEntry,
    pub recently_unfollowed: &'a [ShapeCEntry],
    pub removed_suggestions: &'a [ShapeCEntry],

    pub liked_posts: &'a [ShapeCEntry],
    pub liked_comments: &'a [ShapeAEntry],
    pub stories_viewed: &'a [ShapeCEntry],
    pub saved_posts: &'a [ShapeCEntry],

    pub story_polls: &'a [ShapeAEntry],
    pub story_quizzes: &'a [ShapeAEntry],
    pub story_questions: &'a [ShapeAEntry],
    pub story_emoji_sliders: &'a [ShapeAEntry],
    pub story_emoji_reactions: &'a [ShapeAEntry],
    pub story_reaction_stickers: &'a [ShapeAEntry],
    pub story_countdowns: &'a [ShapeAEntry],

    pub post_comments: &'a [CommentEntry],
    pub reels_comments: &'a [CommentEntry],
    pub hype: &'a [CommentEntry],
}

/// Build one [`AccountFeatures`] row per followee, keyed by handle.
///
/// `now` is passed explicitly so tests pin a stable reference point without
/// drifting against real time; production callers should pass
/// [`Timestamp::now`].
///
/// Followings whose handle appears in `blocked` or `recently_unfollowed` are
/// hard-excluded — DESIGN.md treats those as input-set filters, not features.
pub fn aggregate(inputs: &AggregateInputs<'_>, now: Timestamp) -> Vec<AccountFeatures> {
    let blocked = collect_handles(inputs.blocked);
    let recently_unfollowed = collect_handles(inputs.recently_unfollowed);
    let close_friend = collect_handles(inputs.close_friends);
    let favorited = collect_handles(inputs.favorited);
    let restricted = collect_handles(inputs.restricted);
    let removed_suggestion = collect_handles(inputs.removed_suggestions);
    let hide_story_target = flag_username(inputs.hide_story_from);

    let mut by_handle: HashMap<&str, AccountFeatures> =
        HashMap::with_capacity(inputs.followings.len());

    for f in inputs.followings {
        let handle: &str = f.username.as_str();
        // Defends both halves of the empty-handle phantom-match path: an
        // empty `title` in `following.json` (schema drift — `#[serde(default)]
        // String` would deserialize to `""`) cannot anchor an output row,
        // so neither flag-file entries nor activity entries with an empty
        // `Username` / `Owner.Username` can credit it through `get_mut("")`.
        if handle.is_empty() || blocked.contains(handle) || recently_unfollowed.contains(handle) {
            continue;
        }
        let features = AccountFeatures {
            username: f.username.clone(),
            follow_tenure_days: f.followed_at.and_then(|ts| tenure_days(ts, now)),
            is_close_friend: close_friend.contains(handle),
            is_favorited: favorited.contains(handle),
            is_blocked: false,
            is_restricted: restricted.contains(handle),
            is_hide_story_from: hide_story_target == Some(handle),
            is_removed_suggestion: removed_suggestion.contains(handle),
            recently_unfollowed: false,
            likes_given: 0,
            comments_given: 0,
            story_interactions_out: 0,
            stories_viewed: 0,
            saved_their_content: 0,
            dm_messages_total: 0,
            dm_recency_days: None,
            dm_balance: None,
            dm_reactions_given: 0,
            dm_reactions_received: 0,
            inbound_dm_request: false,
        };
        by_handle.insert(handle, features);
    }

    for entry in inputs.liked_posts {
        if let Some(target) = owner_username(entry)
            && let Some(features) = by_handle.get_mut(target)
        {
            features.likes_given += 1;
        }
    }
    for entry in inputs.liked_comments {
        if let Some(features) = by_handle.get_mut(entry.username.as_str()) {
            features.likes_given += 1;
        }
    }

    for entry in inputs
        .post_comments
        .iter()
        .chain(inputs.reels_comments.iter())
        .chain(inputs.hype.iter())
    {
        if let Some(features) = by_handle.get_mut(entry.target_username.as_str()) {
            features.comments_given += 1;
        }
    }

    for entry in inputs
        .story_polls
        .iter()
        .chain(inputs.story_quizzes.iter())
        .chain(inputs.story_questions.iter())
        .chain(inputs.story_emoji_sliders.iter())
        .chain(inputs.story_emoji_reactions.iter())
        .chain(inputs.story_reaction_stickers.iter())
        .chain(inputs.story_countdowns.iter())
    {
        if let Some(features) = by_handle.get_mut(entry.username.as_str()) {
            features.story_interactions_out += 1;
        }
    }

    for entry in inputs.stories_viewed {
        if let Some(target) = owner_username(entry)
            && let Some(features) = by_handle.get_mut(target)
        {
            features.stories_viewed += 1;
        }
    }

    for entry in inputs.saved_posts {
        if let Some(target) = owner_username(entry)
            && let Some(features) = by_handle.get_mut(target)
        {
            features.saved_their_content += 1;
        }
    }

    by_handle.into_values().collect()
}

/// Walk a relationship-flag entry to its outer-level `Username` value.
///
/// Distinct from [`owner_username`]: the seven `label_values` files
/// (`close_friends`, `profiles_you've_favorited`, etc.) carry the per-account
/// handle at the OUTER `label_values` level (flat `{label, value}` shape),
/// not nested under `Owner.dict[0].dict`. Empty `value` is treated as
/// "no handle" — mirrors the resolver's posture against the 21 empty-Name
/// entries in the real export.
fn flag_username(entry: &ShapeCEntry) -> Option<&str> {
    entry
        .label_values
        .iter()
        .find(|lv| lv.label.as_deref() == Some("Username"))?
        .value
        .as_deref()
        .filter(|s| !s.is_empty())
}

fn collect_handles(entries: &[ShapeCEntry]) -> HashSet<&str> {
    entries.iter().filter_map(flag_username).collect()
}

fn tenure_days(followed_at: Timestamp, now: Timestamp) -> Option<u32> {
    let secs = now.duration_since(followed_at).as_secs();
    if secs < 0 {
        return None;
    }
    u32::try_from(secs / 86_400).ok()
}

#[cfg(test)]
mod tests {
    //! Structural tests on synthetic parser outputs — no fixture I/O.
    //!
    //! Pins the four invariants the slice-7A spec calls out: filter-to-
    //! followings, blocked/recently_unfollowed input-set exclusion, boolean
    //! flag population from the seven `label_values` files, and activity-
    //! count summation across the five source-groups (`liked_posts` +
    //! `liked_comments`, three comment files, seven story shape-A files,
    //! `stories_viewed`, `saved_posts`). `follow_tenure_days` is pinned
    //! against a fixed `now` so the test never drifts against real time.
    use super::*;
    use crate::export::{ShapeCInnerEntry, ShapeCInnerGroup, ShapeCLabelValue};

    fn following(username: &str, followed_at: Option<Timestamp>) -> FollowingEntry {
        FollowingEntry {
            username: username.to_owned(),
            followed_at,
        }
    }

    fn flag(handle: &str) -> ShapeCEntry {
        ShapeCEntry {
            fbid: None,
            timestamp: None,
            label_values: vec![ShapeCLabelValue {
                label: Some("Username".to_owned()),
                value: Some(handle.to_owned()),
                title: None,
                dict: Vec::new(),
            }],
        }
    }

    fn owner_entry(handle: &str) -> ShapeCEntry {
        ShapeCEntry {
            fbid: None,
            timestamp: None,
            label_values: vec![ShapeCLabelValue {
                label: None,
                value: None,
                title: Some("Owner".to_owned()),
                dict: vec![ShapeCInnerGroup {
                    title: None,
                    dict: vec![ShapeCInnerEntry {
                        label: Some("Username".to_owned()),
                        value: Some(handle.to_owned()),
                    }],
                }],
            }],
        }
    }

    fn shape_a(username: &str) -> ShapeAEntry {
        ShapeAEntry {
            username: username.to_owned(),
            timestamp: None,
        }
    }

    fn comment(target: &str) -> CommentEntry {
        CommentEntry {
            target_username: target.to_owned(),
            timestamp: None,
        }
    }

    fn empty_inputs<'a>(
        followings: &'a [FollowingEntry],
        hide_story_from: &'a ShapeCEntry,
    ) -> AggregateInputs<'a> {
        AggregateInputs {
            followings,
            close_friends: &[],
            favorited: &[],
            blocked: &[],
            restricted: &[],
            hide_story_from,
            recently_unfollowed: &[],
            removed_suggestions: &[],
            liked_posts: &[],
            liked_comments: &[],
            stories_viewed: &[],
            saved_posts: &[],
            story_polls: &[],
            story_quizzes: &[],
            story_questions: &[],
            story_emoji_sliders: &[],
            story_emoji_reactions: &[],
            story_reaction_stickers: &[],
            story_countdowns: &[],
            post_comments: &[],
            reels_comments: &[],
            hype: &[],
        }
    }

    fn empty_hide_entry() -> ShapeCEntry {
        ShapeCEntry {
            fbid: None,
            timestamp: None,
            label_values: Vec::new(),
        }
    }

    fn by_username(features: Vec<AccountFeatures>) -> HashMap<String, AccountFeatures> {
        features
            .into_iter()
            .map(|f| (f.username.clone(), f))
            .collect()
    }

    fn fixed_now() -> Timestamp {
        // 2027-01-15T08:00:00Z — a stable reference point so `tenure_days`
        // arithmetic stays deterministic across time.
        Timestamp::from_second(1_800_000_000).expect("constant timestamp")
    }

    #[test]
    fn filters_output_to_followings_set() {
        let followings = vec![following("alice", None), following("bob", None)];
        let hide = empty_hide_entry();
        let mut inputs = empty_inputs(&followings, &hide);
        // close_friends includes a handle (`stranger`) that's NOT a following:
        // the aggregator must drop it from the output, not promote it.
        let close_friends = vec![flag("alice"), flag("stranger")];
        inputs.close_friends = &close_friends;

        let out = aggregate(&inputs, fixed_now());
        let handles: HashSet<&str> = out.iter().map(|f| f.username.as_str()).collect();
        assert_eq!(handles, HashSet::from(["alice", "bob"]));
    }

    #[test]
    fn excludes_blocked_and_recently_unfollowed_from_output() {
        let followings = vec![
            following("alice", None),
            following("bob", None),
            following("carol", None),
            following("dave", None),
        ];
        let hide = empty_hide_entry();
        let mut inputs = empty_inputs(&followings, &hide);
        let blocked = vec![flag("carol")];
        let recently_unfollowed = vec![flag("dave")];
        inputs.blocked = &blocked;
        inputs.recently_unfollowed = &recently_unfollowed;

        let out = aggregate(&inputs, fixed_now());
        let handles: HashSet<&str> = out.iter().map(|f| f.username.as_str()).collect();
        assert_eq!(
            handles,
            HashSet::from(["alice", "bob"]),
            "blocked + recently_unfollowed are hard input-set excludes, not features",
        );
    }

    #[test]
    fn populates_boolean_flags_from_outer_label_values() {
        let followings = vec![
            following("alice", None),
            following("bob", None),
            following("carol", None),
        ];
        let hide = flag("alice");
        let mut inputs = empty_inputs(&followings, &hide);
        let close_friends = vec![flag("alice")];
        let favorited = vec![flag("bob"), flag("carol")];
        let restricted = vec![flag("bob")];
        let removed_suggestions = vec![flag("carol")];
        inputs.close_friends = &close_friends;
        inputs.favorited = &favorited;
        inputs.restricted = &restricted;
        inputs.removed_suggestions = &removed_suggestions;

        let by = by_username(aggregate(&inputs, fixed_now()));
        let alice = &by["alice"];
        assert!(alice.is_close_friend);
        assert!(alice.is_hide_story_from);
        assert!(!alice.is_favorited);
        assert!(!alice.is_restricted);
        assert!(!alice.is_removed_suggestion);

        let bob = &by["bob"];
        assert!(bob.is_favorited);
        assert!(bob.is_restricted);
        assert!(!bob.is_close_friend);

        let carol = &by["carol"];
        assert!(carol.is_favorited);
        assert!(carol.is_removed_suggestion);
        assert!(!carol.is_hide_story_from);
    }

    #[test]
    fn sums_activity_counts_across_sources() {
        let followings = vec![following("alice", None), following("bob", None)];
        let hide = empty_hide_entry();
        let mut inputs = empty_inputs(&followings, &hide);

        // alice gets 2 liked_posts + 1 liked_comment → likes_given = 3.
        let liked_posts = vec![
            owner_entry("alice"),
            owner_entry("alice"),
            owner_entry("stranger"),
        ];
        let liked_comments = vec![shape_a("alice"), shape_a("stranger")];
        inputs.liked_posts = &liked_posts;
        inputs.liked_comments = &liked_comments;

        // bob gets 1 of each of post/reels/hype → comments_given = 3.
        let post_comments = vec![comment("bob"), comment("stranger")];
        let reels_comments = vec![comment("bob")];
        let hype = vec![comment("bob"), comment("stranger")];
        inputs.post_comments = &post_comments;
        inputs.reels_comments = &reels_comments;
        inputs.hype = &hype;

        // alice gets 1 entry across each of the 7 story shape-A files →
        // story_interactions_out = 7.
        let polls = vec![shape_a("alice")];
        let quizzes = vec![shape_a("alice")];
        let questions = vec![shape_a("alice")];
        let emoji_sliders = vec![shape_a("alice")];
        let emoji_reactions = vec![shape_a("alice")];
        let reaction_stickers = vec![shape_a("alice")];
        let countdowns = vec![shape_a("alice")];
        inputs.story_polls = &polls;
        inputs.story_quizzes = &quizzes;
        inputs.story_questions = &questions;
        inputs.story_emoji_sliders = &emoji_sliders;
        inputs.story_emoji_reactions = &emoji_reactions;
        inputs.story_reaction_stickers = &reaction_stickers;
        inputs.story_countdowns = &countdowns;

        let stories_viewed = vec![owner_entry("alice"), owner_entry("alice")];
        let saved_posts = vec![owner_entry("bob")];
        inputs.stories_viewed = &stories_viewed;
        inputs.saved_posts = &saved_posts;

        let by = by_username(aggregate(&inputs, fixed_now()));
        let alice = &by["alice"];
        let bob = &by["bob"];

        assert_eq!(alice.likes_given, 3);
        assert_eq!(alice.comments_given, 0);
        assert_eq!(alice.story_interactions_out, 7);
        assert_eq!(alice.stories_viewed, 2);
        assert_eq!(alice.saved_their_content, 0);

        assert_eq!(bob.likes_given, 0);
        assert_eq!(bob.comments_given, 3);
        assert_eq!(bob.story_interactions_out, 0);
        assert_eq!(bob.stories_viewed, 0);
        assert_eq!(bob.saved_their_content, 1);
    }

    #[test]
    fn follow_tenure_days_computed_from_now() {
        let now = fixed_now();
        let thirty_days = Timestamp::from_second(1_800_000_000 - 30 * 86_400).unwrap();
        let followings = vec![
            following("alice", Some(thirty_days)),
            following("bob", None),
        ];
        let hide = empty_hide_entry();
        let inputs = empty_inputs(&followings, &hide);

        let by = by_username(aggregate(&inputs, now));
        assert_eq!(by["alice"].follow_tenure_days, Some(30));
        assert_eq!(by["bob"].follow_tenure_days, None);
    }

    #[test]
    fn future_follow_timestamp_yields_none_tenure() {
        // Synthetic follow-timestamps in the export occasionally drift forward
        // of `now` (clock skew on the export job, manually edited fixtures).
        // The duration arithmetic must not panic or wrap — tenure stays None.
        let now = fixed_now();
        let future = Timestamp::from_second(1_800_000_000 + 86_400).unwrap();
        let followings = vec![following("alice", Some(future))];
        let hide = empty_hide_entry();
        let inputs = empty_inputs(&followings, &hide);

        let by = by_username(aggregate(&inputs, now));
        assert_eq!(by["alice"].follow_tenure_days, None);
    }

    #[test]
    fn empty_handle_following_does_not_anchor_output_row() {
        // Schema drift on `following.json`: an empty `title` deserializes to
        // an empty `FollowingEntry.username` (the field carries
        // `#[serde(default)] String`). That entry must NOT anchor an
        // output row — otherwise every activity file with an empty
        // `Owner.Username` and every flag file with an empty `Username`
        // would credit the phantom empty-handle followee.
        let followings = vec![following("", None), following("alice", None)];
        let hide = empty_hide_entry();
        let mut inputs = empty_inputs(&followings, &hide);
        // Activity entries with empty `Owner.Username` and shape-A username:
        // both would route to `get_mut("")` if the empty handle existed.
        let liked_posts = vec![owner_entry("")];
        let liked_comments = vec![shape_a("")];
        inputs.liked_posts = &liked_posts;
        inputs.liked_comments = &liked_comments;

        let out = aggregate(&inputs, fixed_now());
        let handles: HashSet<&str> = out.iter().map(|f| f.username.as_str()).collect();
        assert_eq!(handles, HashSet::from(["alice"]));
        let alice = out.iter().find(|f| f.username == "alice").unwrap();
        assert_eq!(
            alice.likes_given, 0,
            "empty-Owner activity must not credit alice either"
        );
    }

    #[test]
    fn empty_username_in_flag_files_does_not_create_phantom_match() {
        // Empty Username at the outer label_values level must NOT promote a
        // followee whose handle happens to be "" — same posture as the
        // resolver. Real-world failure mode if this ever broke: every
        // followee silently gets is_close_friend = true.
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let mut inputs = empty_inputs(&followings, &hide);
        let close_friends = vec![ShapeCEntry {
            fbid: None,
            timestamp: None,
            label_values: vec![ShapeCLabelValue {
                label: Some("Username".to_owned()),
                value: Some(String::new()),
                title: None,
                dict: Vec::new(),
            }],
        }];
        inputs.close_friends = &close_friends;

        let by = by_username(aggregate(&inputs, fixed_now()));
        assert!(!by["alice"].is_close_friend);
    }
}
