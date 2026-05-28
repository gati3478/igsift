//! Parsers for the Instagram personal data export (JSON).
//!
//! Schema was validated against the 2026-05-11 export on 2026-05-26 by walking
//! every JSON file with [`scripts/walk_export_schema.sh`](../scripts/walk_export_schema.sh).
//! Paths and field names below match what Instagram actually ships today.
//! Re-run the walker after every new export to detect drift.
//!
//! Implemented in this pass: `following.json`, `followers_*.json`, DM
//! threads under `messages/inbox/<thread>/message_*.json` and the same shape
//! under `messages/message_requests/<thread>/`, the seven shape-C /
//! single-entry relationship-flag files (`close_friends`,
//! `profiles_you've_favorited`, `blocked_profiles`, `restricted_profiles`,
//! `hide_story_from`, `recently_unfollowed_profiles`, `removed_suggestions`),
//! the four shape-C-with-nested-`Owner` activity files (`liked_posts`,
//! `story_likes`, `stories_viewed`, `saved_posts`) plus the
//! [`owner_username`] helper, the eight shape-A activity files
//! (`liked_comments` and the seven `story_interactions/*` files) returning
//! [`ShapeAEntry`], the three shape-D comment files (`post_comments_*`,
//! `reels_comments`, `hype`) returning [`CommentEntry`], and the user's own
//! `(handle, name)` from `personal_information.json` returning
//! [`MeIdentity`].
//!
//! Robustness approach:
//!
//! - Per-file deserializer for each of the four JSON shape groups in
//!   `docs/DESIGN.md`. Sharing one struct across `following.json` and
//!   `followers_*.json` would silently misread one — they differ in where the
//!   username lives.
//! - `#[serde(default)]` + `Option<T>` on every leaf so a renamed key degrades
//!   to a `None` rather than aborting the run.
//! - Every `from_reader` is wrapped in `serde_path_to_error::deserialize` so a
//!   parse failure names the offending JSON path (e.g.
//!   `relationships_following[5].string_list_data[0].timestamp`) — the
//!   schema-drift survival mechanism per `CLAUDE.md`.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::Deserialize;

const FOLLOW_DIR: &str = "connections/followers_and_following";
const FOLLOWING_FILE: &str = "connections/followers_and_following/following.json";
const ACTIVITY_DIR: &str = "your_instagram_activity";
const INBOX_DIR: &str = "your_instagram_activity/messages/inbox";
const MESSAGE_REQUESTS_DIR: &str = "your_instagram_activity/messages/message_requests";
const LIKED_POSTS: &str = "your_instagram_activity/likes/liked_posts.json";
const STORY_LIKES: &str = "your_instagram_activity/story_interactions/story_likes.json";
const STORIES_VIEWED: &str = "your_instagram_activity/story_interactions/stories_viewed.json";
const SAVED_POSTS: &str = "your_instagram_activity/saved/saved_posts.json";

const LIKED_COMMENTS: &str = "your_instagram_activity/likes/liked_comments.json";
const STORY_POLLS: &str = "your_instagram_activity/story_interactions/polls.json";
const STORY_QUIZZES: &str = "your_instagram_activity/story_interactions/quizzes.json";
const STORY_QUESTIONS: &str = "your_instagram_activity/story_interactions/questions.json";
const STORY_EMOJI_SLIDERS: &str = "your_instagram_activity/story_interactions/emoji_sliders.json";
const STORY_EMOJI_REACTIONS: &str =
    "your_instagram_activity/story_interactions/emoji_story_reactions.json";
const STORY_REACTION_STICKERS: &str =
    "your_instagram_activity/story_interactions/story_reaction_sticker_reactions.json";
const STORY_COUNTDOWNS: &str = "your_instagram_activity/story_interactions/countdowns.json";

const POST_COMMENTS_DIR: &str = "your_instagram_activity/comments";
const REELS_COMMENTS: &str = "your_instagram_activity/comments/reels_comments.json";
const HYPE: &str = "your_instagram_activity/comments/hype.json";

const PERSONAL_INFO: &str = "personal_information/personal_information/personal_information.json";

// ── Public output types ──────────────────────────────────────────────────────

/// One account I follow.
#[derive(Debug, Clone)]
pub struct FollowingEntry {
    pub username: String,
    pub followed_at: Option<Timestamp>,
}

/// One account that follows me.
#[derive(Debug, Clone)]
pub struct FollowerEntry {
    pub username: String,
    pub followed_me_at: Option<Timestamp>,
}

/// One DM thread. `messages` is the concatenation of every `message_*.json`
/// part inside the thread folder, in part-number order — Instagram splits
/// large conversations across multiple files and dropping parts > 1 would
/// silently truncate the largest threads.
#[derive(Debug, Clone)]
pub struct DmThread {
    pub folder: String,
    pub title: Option<String>,
    pub participants: Vec<String>,
    pub messages: Vec<DmMessage>,
}

#[derive(Debug, Clone)]
pub struct DmMessage {
    pub sender: Option<String>,
    pub timestamp: Option<Timestamp>,
    pub content: Option<String>,
    pub reactions: Vec<DmReaction>,
}

#[derive(Debug, Clone)]
pub struct DmReaction {
    pub reaction: Option<String>,
    pub actor: Option<String>,
}

/// One row from a "label-values" relationship or activity file (shape **C**
/// in DESIGN).
///
/// Backs the seven relationship-flag files (`close_friends`,
/// `profiles_you've_favorited`, `blocked_profiles`, `restricted_profiles`,
/// `recently_unfollowed_profiles`, `removed_suggestions`, single-entry
/// `hide_story_from`) plus the four nested-`Owner` activity files
/// (`liked_posts`, `story_likes`, `stories_viewed`, `saved_posts`).
///
/// Two coexisting shapes appear inside `label_values`:
///
/// - flat: `{label, value}` — URL, Caption, Username on the relationship
///   files, etc. The username on `close_friends.json` lives here.
/// - nested: `{title, dict}` — used for `Owner` (the post author on activity
///   files) and `Hashtags`. The activity-file username lives **inside**
///   `dict[0].dict[label == "Username"].value`, which is what
///   [`owner_username`] extracts.
///
/// One struct fits both because the field set is disjoint and every leaf
/// carries `#[serde(default)]`; a missing field defaults to `None`/empty
/// rather than aborting the parse — the schema-drift posture per `CLAUDE.md`.
#[derive(Debug, Clone, Deserialize)]
pub struct ShapeCEntry {
    #[serde(default)]
    pub fbid: Option<String>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub label_values: Vec<ShapeCLabelValue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShapeCLabelValue {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub dict: Vec<ShapeCInnerGroup>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShapeCInnerGroup {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub dict: Vec<ShapeCInnerEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShapeCInnerEntry {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

/// One outbound activity entry — a like I gave on a comment, a poll I
/// voted on, a quiz I answered, an emoji slider I dragged, etc. The
/// target account (whose content I engaged with) is `username`;
/// `timestamp` is when the action happened.
///
/// Logical abstraction shared by all eight outbound-activity readers
/// (`likes/liked_comments.json` and the seven `story_interactions/*.json`),
/// even though only `liked_comments.json` still ships in the original
/// shape-A wrapper format. The seven story files drifted to shape-C
/// records in 2026-05; their readers project back to this type via
/// [`shape_c_owner_to_shape_a_entries`] so downstream consumers
/// don't see the wire-format split.
#[derive(Debug, Clone)]
pub struct ShapeAEntry {
    pub username: String,
    pub timestamp: Option<Timestamp>,
}

/// The user's own identity — handle and display name from the export.
///
/// `name` is what appears in `participants[].name` and `messages[].sender_name`
/// on every DM thread (Instagram ships display names there, never handles),
/// so it's the load-bearing key for classifying DM message direction (`me`
/// vs. partner) and DM-reaction direction (`given` vs. `received`) in the
/// feature aggregator. `handle` is the user's Instagram username.
///
/// Source: `personal_information/personal_information/personal_information.json`,
/// extracted from `profile_user[0].string_map_data["Username"|"Name"].value`.
#[derive(Debug, Clone)]
pub struct MeIdentity {
    pub handle: String,
    pub name: String,
}

/// One outbound comment I left — on a post, a reel, or a story.
///
/// Backs all three shape-**D** comment files: `comments/post_comments_*.json`
/// (bare array), `comments/reels_comments.json` (wrapped under
/// `comments_reels_comments`), and `comments/hype.json` (wrapped under
/// `comments_story_comments`). The target account (whose post/reel/story I
/// commented on) is `target_username`; the comment timestamp is in Unix
/// seconds at the source.
///
/// Comment text and media URI from `string_map_data` are deliberately not
/// surfaced — DESIGN.md's `comments_given` feature consumes count, recency,
/// and target, not the body. Adding the text would put export content on
/// the public type without a consumer.
#[derive(Debug, Clone)]
pub struct CommentEntry {
    pub target_username: String,
    pub timestamp: Option<Timestamp>,
}

// ── Raw deserialization shapes ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct FollowingFileRaw {
    #[serde(default)]
    relationships_following: Vec<RelationshipEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct RelationshipEntryRaw {
    #[serde(default)]
    title: String,
    #[serde(default)]
    string_list_data: Vec<StringListItemRaw>,
}

#[derive(Debug, Deserialize)]
struct StringListItemRaw {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ThreadFileRaw {
    #[serde(default)]
    messages: Vec<MessageRaw>,
    #[serde(default)]
    participants: Vec<ParticipantRaw>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageRaw {
    #[serde(default)]
    sender_name: Option<String>,
    #[serde(default)]
    timestamp_ms: Option<i64>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reactions: Vec<ReactionRaw>,
}

#[derive(Debug, Deserialize)]
struct ReactionRaw {
    #[serde(default)]
    reaction: Option<String>,
    #[serde(default)]
    actor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParticipantRaw {
    #[serde(default)]
    name: String,
}

// ── Shape-A wrapper struct ───────────────────────────────────────────────────
//
// `liked_comments.json` is the last surviving shape-A activity file:
// `{likes_comment_likes: [{title, string_list_data}, ...]}`. The seven
// `story_interactions/*.json` files used to share this shape but IG
// dropped the wrapper key around 2026-05 — they now ship as bare arrays
// of shape-C records (six) or a single top-level record (countdowns).
// The seven story readers project shape-C back to `ShapeAEntry` so the
// downstream feature aggregator keeps its `(username, timestamp)`
// contract unchanged.
//
// Rename drift on the surviving wrapper key still produces a **silent
// empty Vec**: `#[serde(default)]` plus serde's ignore-unknown-fields
// posture means a renamed key falls back to `Vec::new()`. The drift is
// caught downstream by the locked-in fixture counts in `tests/cli.rs`
// dropping to 0.

#[derive(Debug, Deserialize)]
struct LikedCommentsFileRaw {
    #[serde(default)]
    likes_comment_likes: Vec<RelationshipEntryRaw>,
}

// ── Shape-D wrapper + entry structs ──────────────────────────────────────────
//
// Shape D entry is `{string_map_data: {<FieldName>: {value?, timestamp?}}}`,
// where each value object carries either a string `value` or a numeric
// `timestamp` (Unix seconds) — sparsely populated, never both. The two
// fields the scoring layer needs are codified as compile-checked struct
// fields with `#[serde(rename)]` so the human-readable IG spellings
// ("Media Owner", "Time") live in code as a single source of truth, not
// only in `docs/DESIGN.md`. Same rationale as the shape-A wrapper-key
// structs above: rename drift on a sub-key produces a None at extraction
// time rather than silently passing through, and the honest-count filter
// in `shape_d_entries` surfaces it as a missing count.
//
// `post_comments_*.json` is a bare array of these entries (no wrapper);
// `reels_comments.json` and `hype.json` wrap the array under their
// respective IG wrapper keys.

#[derive(Debug, Deserialize)]
struct ReelsCommentsFileRaw {
    #[serde(default)]
    comments_reels_comments: Vec<ShapeDEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct HypeFileRaw {
    #[serde(default)]
    comments_story_comments: Vec<ShapeDEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct ShapeDEntryRaw {
    #[serde(default)]
    string_map_data: ShapeDStringMap,
}

#[derive(Debug, Default, Deserialize)]
struct ShapeDStringMap {
    #[serde(default, rename = "Media Owner")]
    media_owner: ShapeDValue,
    #[serde(default, rename = "Time")]
    time: ShapeDValue,
}

#[derive(Debug, Default, Deserialize)]
struct ShapeDValue {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

// ── personal_information.json structs ────────────────────────────────────────
//
// The file ships `{profile_user: [{string_map_data: {Username|Name|Email|...:
// {href, value, timestamp}, ...}}]}`. Only `Username` and `Name` are read;
// every other sub-key (Email, Phone Number, Bio, …) is PII and ignored by
// serde's default ignore-unknown-fields posture. Same named-fields-with-
// `#[serde(rename)]` approach as shape D: the IG-specific spellings are
// compile-checked in code, drift to None on rename.

#[derive(Debug, Deserialize)]
struct PersonalInfoFileRaw {
    #[serde(default)]
    profile_user: Vec<ProfileUserRaw>,
}

#[derive(Debug, Deserialize)]
struct ProfileUserRaw {
    #[serde(default)]
    string_map_data: ProfileUserStringMap,
}

#[derive(Debug, Default, Deserialize)]
struct ProfileUserStringMap {
    #[serde(default, rename = "Username")]
    username: ShapeDValue,
    #[serde(default, rename = "Name")]
    name: ShapeDValue,
}

// ── Public readers ───────────────────────────────────────────────────────────

/// Parse `connections/followers_and_following/following.json` (shape A).
/// Verify `export_dir` looks like an unzipped Instagram "Download Your
/// Information" folder before the parser layer runs.
///
/// Pointing the binary at the wrong folder is the most common
/// non-schema-drift failure mode: the export's parent directory, a
/// freshly-created empty folder, or the still-zipped archive root. The
/// per-file parsers would each fail in turn with a different error,
/// burning startup time on each. This shallow check surfaces a single
/// readable error naming every expected top-level marker that is
/// missing.
///
/// What it checks: the three load-bearing top-level paths
/// (`connections/followers_and_following/following.json` — the
/// followings list; `personal_information/personal_information/personal_information.json`
/// — the `me` identity; `your_instagram_activity/` — the activity tree
/// root). Activity sub-files are not checked individually — IG drops
/// optional files (no countdowns last quarter? no `countdowns.json`),
/// and each parser already tolerates a missing file by returning an
/// empty Vec.
pub fn validate_shape(export_dir: &Path) -> Result<()> {
    let mut missing: Vec<&str> = Vec::new();
    for path in [FOLLOWING_FILE, PERSONAL_INFO] {
        if !export_dir.join(path).is_file() {
            missing.push(path);
        }
    }
    if !export_dir.join(ACTIVITY_DIR).is_dir() {
        missing.push(ACTIVITY_DIR);
    }
    if missing.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{} does not look like an unzipped Instagram \"Download Your Information\" export — \
         missing expected paths:\n  {}\nIf the path points at a parent directory or a still-zipped \
         archive root, descend into the unzipped folder containing `connections/` and \
         `your_instagram_activity/`.",
        export_dir.display(),
        missing.join("\n  ")
    )
}

///
/// Shape A wraps a single array under the `relationships_following` key.
/// The username lives in `title`; the follow timestamp is the (only)
/// `string_list_data[0].timestamp` in Unix seconds.
pub fn read_following(export_dir: &Path) -> Result<Vec<FollowingEntry>> {
    let path = export_dir.join(FOLLOW_DIR).join("following.json");
    let raw: FollowingFileRaw = parse_json(&path)?;

    Ok(raw
        .relationships_following
        .into_iter()
        .map(|entry| FollowingEntry {
            username: entry.title,
            followed_at: entry
                .string_list_data
                .first()
                .and_then(|item| item.timestamp)
                .and_then(seconds_to_timestamp),
        })
        .collect())
}

/// Parse every `connections/followers_and_following/followers_*.json` (shape B).
///
/// Shape B is a bare top-level array (no wrapper key) and — crucially — leaves
/// `title` empty, placing the username in `string_list_data[0].value`.
/// Instagram chunks the followers list across `followers_1.json`,
/// `followers_2.json`, … for accounts with many followers; concatenate in
/// numeric order.
pub fn read_followers(export_dir: &Path) -> Result<Vec<FollowerEntry>> {
    let dir = export_dir.join(FOLLOW_DIR);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("followers_") && name.ends_with(".json"))
        })
        .collect();
    paths.sort();

    let mut out = Vec::new();
    for path in paths {
        let raw: Vec<RelationshipEntryRaw> = parse_json(&path)?;
        for entry in raw {
            let Some(item) = entry.string_list_data.into_iter().next() else {
                continue;
            };
            let Some(username) = item.value else {
                continue;
            };
            out.push(FollowerEntry {
                username,
                followed_me_at: item.timestamp.and_then(seconds_to_timestamp),
            });
        }
    }
    Ok(out)
}

/// Parse every DM thread under `your_instagram_activity/messages/inbox/`.
pub fn read_inbox(export_dir: &Path) -> Result<Vec<DmThread>> {
    read_thread_dir(&export_dir.join(INBOX_DIR))
}

/// Parse every thread under `your_instagram_activity/messages/message_requests/`.
///
/// Schema is identical to `inbox/` — same `message_*.json` parts, same
/// multi-part concat rules; only the base directory differs. Surfaced as a
/// separate signal because the relationship semantics differ:
/// `message_requests/` is inbound DMs from accounts the user never accepted,
/// not a held conversation. Schema-extra keys (`is_pending`, `magic_words`,
/// …) ride along harmlessly via serde's default ignore-unknown-fields.
pub fn read_message_requests(export_dir: &Path) -> Result<Vec<DmThread>> {
    read_thread_dir(&export_dir.join(MESSAGE_REQUESTS_DIR))
}

/// Parse `close_friends.json` (shape **C** — bare array of label-values entries).
pub fn read_close_friends(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_array(export_dir, "close_friends.json")
}

/// Parse `profiles_you've_favorited.json` (shape **C**).
pub fn read_favorited(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    // Apostrophe in the filename is preserved by `Path::join` — no shell
    // escaping needed.
    read_shape_c_array(export_dir, "profiles_you've_favorited.json")
}

/// Parse `blocked_profiles.json` (shape **C**).
pub fn read_blocked(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_array(export_dir, "blocked_profiles.json")
}

/// Parse `restricted_profiles.json` (shape **C**).
pub fn read_restricted(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_array(export_dir, "restricted_profiles.json")
}

/// Parse `recently_unfollowed_profiles.json` (shape **C**).
pub fn read_recently_unfollowed(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_array(export_dir, "recently_unfollowed_profiles.json")
}

/// Parse `removed_suggestions.json` (shape **C**).
pub fn read_removed_suggestions(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_array(export_dir, "removed_suggestions.json")
}

/// Parse `hide_story_from.json`.
///
/// Unlike the other relationship-flag files this one is a single shape-C
/// entry at the top level — not an array of them. The 2026-05-11 export
/// validated by `scripts/walk_export_schema.sh` ships a lone object with the
/// same `{fbid, timestamp, label_values, media}` keys. Returning
/// `ShapeCEntry` (not `Vec<ShapeCEntry>`) keeps the structural difference
/// visible in the API.
pub fn read_hide_story_from(export_dir: &Path) -> Result<ShapeCEntry> {
    let path = export_dir.join(FOLLOW_DIR).join("hide_story_from.json");
    parse_json(&path)
}

/// Parse `your_instagram_activity/likes/liked_posts.json` — shape **C** with
/// nested `Owner`. Each entry is a like I gave; the target account is
/// reachable via [`owner_username`].
pub fn read_liked_posts(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_at(export_dir, LIKED_POSTS)
}

/// Parse `your_instagram_activity/story_interactions/story_likes.json` —
/// shape **C** with nested `Owner`. Each entry is a story like I gave.
pub fn read_story_likes(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_at(export_dir, STORY_LIKES)
}

/// Parse `your_instagram_activity/story_interactions/stories_viewed.json` —
/// shape **C** with nested `Owner`. Passive views (low-intent), but the
/// entries carry the same structure as story_likes.
pub fn read_stories_viewed(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_at(export_dir, STORIES_VIEWED)
}

/// Parse `your_instagram_activity/saved/saved_posts.json` — shape **C** with
/// nested `Owner`. Each entry is a post I saved; the target account is the
/// post owner.
pub fn read_saved_posts(export_dir: &Path) -> Result<Vec<ShapeCEntry>> {
    read_shape_c_at(export_dir, SAVED_POSTS)
}

/// Parse `your_instagram_activity/likes/liked_comments.json` — shape **A**,
/// wrapper key `likes_comment_likes`. Each entry is a comment-like I gave;
/// the target account is `title`.
pub fn read_liked_comments(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let raw: LikedCommentsFileRaw = parse_json(&export_dir.join(LIKED_COMMENTS))?;
    Ok(shape_a_entries(raw.likes_comment_likes))
}

/// Parse `your_instagram_activity/story_interactions/polls.json`. IG dropped
/// the shape-A wrapper around 2026-05 — the file now ships as a bare array
/// of shape-C records, target handle nested under `Owner.dict[].dict[label ==
/// "Username"]`. Projected back to [`ShapeAEntry`] so the aggregator's
/// `(username, timestamp)` contract is unchanged.
pub fn read_story_polls(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    Ok(shape_c_owner_to_shape_a_entries(read_shape_c_at(
        export_dir,
        STORY_POLLS,
    )?))
}

/// Parse `your_instagram_activity/story_interactions/quizzes.json`. Same
/// shape-C-array drift as [`read_story_polls`].
pub fn read_story_quizzes(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    Ok(shape_c_owner_to_shape_a_entries(read_shape_c_at(
        export_dir,
        STORY_QUIZZES,
    )?))
}

/// Parse `your_instagram_activity/story_interactions/questions.json`. Same
/// shape-C-array drift as [`read_story_polls`].
pub fn read_story_questions(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    Ok(shape_c_owner_to_shape_a_entries(read_shape_c_at(
        export_dir,
        STORY_QUESTIONS,
    )?))
}

/// Parse `your_instagram_activity/story_interactions/emoji_sliders.json`.
/// Same shape-C-array drift as [`read_story_polls`].
pub fn read_story_emoji_sliders(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    Ok(shape_c_owner_to_shape_a_entries(read_shape_c_at(
        export_dir,
        STORY_EMOJI_SLIDERS,
    )?))
}

/// Parse `your_instagram_activity/story_interactions/emoji_story_reactions.json`.
/// Same shape-C-array drift as [`read_story_polls`].
pub fn read_story_emoji_reactions(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    Ok(shape_c_owner_to_shape_a_entries(read_shape_c_at(
        export_dir,
        STORY_EMOJI_REACTIONS,
    )?))
}

/// Parse
/// `your_instagram_activity/story_interactions/story_reaction_sticker_reactions.json`.
/// Same shape-C-array drift as [`read_story_polls`].
pub fn read_story_reaction_stickers(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    Ok(shape_c_owner_to_shape_a_entries(read_shape_c_at(
        export_dir,
        STORY_REACTION_STICKERS,
    )?))
}

/// Parse `your_instagram_activity/story_interactions/countdowns.json`.
/// Unlike its six siblings this one ships as a **single** top-level
/// shape-C record (no array wrapper) — IG quirk, mirrored from
/// [`read_hide_story_from`]. Wrapped in a `Vec` so the public return
/// type matches the rest of the story readers.
pub fn read_story_countdowns(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let entry: ShapeCEntry = parse_json(&export_dir.join(STORY_COUNTDOWNS))?;
    Ok(shape_c_owner_to_shape_a_entries(vec![entry]))
}

/// Parse every `your_instagram_activity/comments/post_comments_*.json` —
/// shape **D**, bare array (no wrapper key). Instagram chunks high-volume
/// comment lists across `post_comments_1.json`, `post_comments_2.json`, …
/// — numeric-suffix sort (not lexicographic) so `_10` follows `_9`, not
/// `_2`. Each entry is one comment I left on a post; the target account
/// (post owner) lives at `string_map_data["Media Owner"].value`.
pub fn read_post_comments(export_dir: &Path) -> Result<Vec<CommentEntry>> {
    let dir = export_dir.join(POST_COMMENTS_DIR);
    let parts = numeric_suffix_parts(&dir, "post_comments_")?;

    let mut raw_all: Vec<ShapeDEntryRaw> = Vec::new();
    for path in parts {
        let raw: Vec<ShapeDEntryRaw> = parse_json(&path)?;
        raw_all.extend(raw);
    }
    Ok(shape_d_entries(raw_all))
}

/// Parse `your_instagram_activity/comments/reels_comments.json` — shape
/// **A** wrapper around shape-**D** entries; wrapper key
/// `comments_reels_comments`. Each entry is one comment I left on a reel.
pub fn read_reels_comments(export_dir: &Path) -> Result<Vec<CommentEntry>> {
    let raw: ReelsCommentsFileRaw = parse_json(&export_dir.join(REELS_COMMENTS))?;
    Ok(shape_d_entries(raw.comments_reels_comments))
}

/// Parse `your_instagram_activity/comments/hype.json` — shape **A** wrapper
/// around shape-**D** entries; wrapper key `comments_story_comments`. File
/// name and wrapper key are NOT symmetric (the file is `hype.json` but the
/// key references `story_comments`) — codified so a future reader does not
/// assume a 1:1 file-name ↔ wrapper-key mapping.
pub fn read_hype(export_dir: &Path) -> Result<Vec<CommentEntry>> {
    let raw: HypeFileRaw = parse_json(&export_dir.join(HYPE))?;
    Ok(shape_d_entries(raw.comments_story_comments))
}

/// Read the user's own `(handle, name)` from
/// `personal_information/personal_information/personal_information.json`.
///
/// Unlike the activity-parsers, missing or empty fields here are a HARD
/// ERROR rather than a soft None: every downstream DM-direction
/// classification in the feature aggregator depends on these values. If
/// IG renames `"Username"` or `"Name"` the run cannot meaningfully
/// proceed — fail loud at parse time with the offending path (via
/// `serde_path_to_error`) instead of producing garbage attributions.
pub fn read_me_identity(export_dir: &Path) -> Result<MeIdentity> {
    let raw: PersonalInfoFileRaw = parse_json(&export_dir.join(PERSONAL_INFO))?;
    me_identity_from_raw(raw)
}

/// Field-extraction logic for [`read_me_identity`], split out so the three
/// hard-error paths (empty `profile_user`, missing `Username`, missing
/// `Name`) can be unit-tested without staging a broken JSON file on disk.
fn me_identity_from_raw(raw: PersonalInfoFileRaw) -> Result<MeIdentity> {
    let profile = raw.profile_user.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!(
            "personal_information.json: `profile_user` is empty — cannot determine `me` identity"
        )
    })?;
    let handle = profile
        .string_map_data
        .username
        .value
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "personal_information.json: `profile_user[0].string_map_data[\"Username\"].value` missing or empty"
            )
        })?;
    let name = profile
        .string_map_data
        .name
        .value
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "personal_information.json: `profile_user[0].string_map_data[\"Name\"].value` missing or empty"
            )
        })?;
    let name = crate::text::fix_mojibake(&name).into_owned();
    Ok(MeIdentity { handle, name })
}

/// Extract the `Owner.Username` from a shape-C entry that carries a nested
/// `Owner` section (the four activity files above). Walks
/// `label_values → title == "Owner" → dict[0].dict → label == "Username" →
/// value`. Returns `None` if any step of the walk is missing — schema drift
/// surfaces as a parse path mismatch on the next walker re-run, not as a
/// runtime panic.
///
/// This helper is **not** the username accessor for the relationship-flag
/// files (`close_friends.json`, etc.). Those carry the username at the outer
/// `label_values` level with `label == "Username"`, not nested under
/// `Owner`; they need a different accessor when `features.rs` lands.
pub fn owner_username(entry: &ShapeCEntry) -> Option<&str> {
    entry
        .label_values
        .iter()
        .find(|lv| lv.title.as_deref() == Some("Owner"))?
        .dict
        .first()?
        .dict
        .iter()
        .find(|d| d.label.as_deref() == Some("Username"))?
        .value
        .as_deref()
}

// ── Internals ────────────────────────────────────────────────────────────────

/// Shared between `read_inbox` and `read_message_requests`. Walks one base
/// directory of thread folders, concatenating multi-part `message_*.json`
/// files per thread in numeric-suffix order. See `read_inbox` doc for the
/// part-ordering rationale.
fn read_thread_dir(base: &Path) -> Result<Vec<DmThread>> {
    let mut thread_dirs: Vec<PathBuf> = std::fs::read_dir(base)
        .with_context(|| format!("reading {}", base.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    thread_dirs.sort();

    let mut threads = Vec::with_capacity(thread_dirs.len());
    for thread_dir in thread_dirs {
        let folder = thread_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();

        let parts = numeric_suffix_parts(&thread_dir, "message_")?;
        if parts.is_empty() {
            continue;
        }

        let mut messages = Vec::new();
        let mut title: Option<String> = None;
        let mut participants: Vec<String> = Vec::new();

        for path in &parts {
            let raw: ThreadFileRaw = parse_json(path)?;
            if title.is_none() {
                // Title is an IG-rendered display string (often the
                // group name, or the partner's display name for
                // 1:1s) — same UTF-8-as-Latin-1 mojibake as the
                // participants / sender_name fields, so fixed at
                // parse time to keep every DmThread surface
                // consistent.
                title = raw
                    .title
                    .map(|t| crate::text::fix_mojibake(&t).into_owned());
                participants = raw
                    .participants
                    .into_iter()
                    .map(|p| crate::text::fix_mojibake(&p.name).into_owned())
                    .collect();
            }
            messages.extend(raw.messages.into_iter().map(|m| {
                DmMessage {
                    sender: m
                        .sender_name
                        .map(|s| crate::text::fix_mojibake(&s).into_owned()),
                    timestamp: m.timestamp_ms.and_then(milliseconds_to_timestamp),
                    // Message body is user-typed and can carry
                    // non-ASCII naturally; same exporter bug
                    // afflicts it, fix consistently with the rest.
                    content: m
                        .content
                        .map(|c| crate::text::fix_mojibake(&c).into_owned()),
                    reactions: m
                        .reactions
                        .into_iter()
                        .map(|r| DmReaction {
                            reaction: r.reaction,
                            // `actor` is a display name compared against the
                            // mojibake-fixed `me.name` to classify a reaction
                            // as given vs received — the same cross-side join
                            // as `sender_name`, so it needs the same repair or
                            // the user's own reactions misclassify when their
                            // display name carries non-ASCII bytes.
                            actor: r.actor.map(|a| crate::text::fix_mojibake(&a).into_owned()),
                        })
                        .collect(),
                }
            }));
        }

        threads.push(DmThread {
            folder,
            title,
            participants,
            messages,
        });
    }
    Ok(threads)
}

/// Deserialize a shape-C bare-array file from `connections/followers_and_following/`.
///
/// All six shape-C relationship-flag files share the same top-level shape;
/// only the filename differs.
fn read_shape_c_array(export_dir: &Path, file_name: &str) -> Result<Vec<ShapeCEntry>> {
    let path = export_dir.join(FOLLOW_DIR).join(file_name);
    parse_json(&path)
}

/// Deserialize a shape-C bare-array file at an arbitrary path relative to the
/// export root. Sibling of [`read_shape_c_array`] for activity files that
/// don't live under `connections/followers_and_following/` (likes,
/// story_interactions, saved). Same shape, different parent directory.
fn read_shape_c_at(export_dir: &Path, rel_path: &str) -> Result<Vec<ShapeCEntry>> {
    parse_json(&export_dir.join(rel_path))
}

/// Convert the raw shape-A entry list (after wrapper-key extraction) into
/// the public [`ShapeAEntry`] list. Drops entries with an empty `title` —
/// an entry without an extractable target username is schema drift, not a
/// usable activity signal. The matching posture from prior slices
/// (`hide_story_from_count`, `owner_username`-derived counts) — surface
/// dropped data as a missing count rather than silently passing through.
fn shape_a_entries(raw: Vec<RelationshipEntryRaw>) -> Vec<ShapeAEntry> {
    raw.into_iter()
        .filter_map(|entry| {
            if entry.title.is_empty() {
                return None;
            }
            Some(ShapeAEntry {
                username: entry.title,
                timestamp: entry
                    .string_list_data
                    .first()
                    .and_then(|item| item.timestamp)
                    .and_then(seconds_to_timestamp),
            })
        })
        .collect()
}

/// Project shape-C-with-`Owner` records onto [`ShapeAEntry`] — the
/// projection used by every `read_story_*` reader after IG's 2026-05
/// schema drift dropped the shape-A wrappers. Username comes from
/// [`owner_username`] (nested `Owner.dict[].dict[label == "Username"]`),
/// timestamp from the entry's top-level `timestamp`. Entries without
/// an extractable Owner username are dropped — same honest-count
/// posture as [`shape_a_entries`] and [`shape_d_entries`].
fn shape_c_owner_to_shape_a_entries(raw: Vec<ShapeCEntry>) -> Vec<ShapeAEntry> {
    raw.into_iter()
        .filter_map(|entry| {
            let username = owner_username(&entry)?.to_owned();
            let timestamp = entry.timestamp.and_then(seconds_to_timestamp);
            Some(ShapeAEntry {
                username,
                timestamp,
            })
        })
        .collect()
}

/// Convert the raw shape-D entry list into the public [`CommentEntry`]
/// list. Drops entries without an extractable `Media Owner` username —
/// schema drift or empty fields, not a usable comment signal. Same
/// honest-count posture as [`shape_a_entries`] and the
/// `owner_username`-derived counts: surface dropped data as a missing
/// count rather than silently inflating the total.
fn shape_d_entries(raw: Vec<ShapeDEntryRaw>) -> Vec<CommentEntry> {
    raw.into_iter()
        .filter_map(|entry| {
            let username = entry.string_map_data.media_owner.value?;
            if username.is_empty() {
                return None;
            }
            Some(CommentEntry {
                target_username: username,
                timestamp: entry
                    .string_map_data
                    .time
                    .timestamp
                    .and_then(seconds_to_timestamp),
            })
        })
        .collect()
}

/// Collect `<prefix>1.json`, `<prefix>2.json`, … under `dir`, sorted by
/// numeric suffix. Lexicographic sort would put `…_10.json` before
/// `…_2.json` — wrong for the `validoli` DM thread (10 `message_*` parts)
/// and any chunked `post_comments_*` set. Shared by [`read_thread_dir`]
/// and [`read_post_comments`]; non-matching filenames are skipped.
fn numeric_suffix_parts(dir: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let mut parts: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter_map(|path| {
            let stem = path.file_stem().and_then(|s| s.to_str())?;
            let ext = path.extension().and_then(|s| s.to_str())?;
            if ext != "json" {
                return None;
            }
            let suffix = stem.strip_prefix(prefix)?;
            let n: u32 = suffix.parse().ok()?;
            Some((n, path))
        })
        .collect();
    parts.sort_by_key(|(n, _)| *n);
    Ok(parts.into_iter().map(|(_, p)| p).collect())
}

fn parse_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);
    let de = &mut serde_json::Deserializer::from_reader(reader);
    serde_path_to_error::deserialize(de).map_err(|err| {
        let json_path = err.path().to_string();
        anyhow::anyhow!(
            "parsing {} failed at `{}`: {}",
            path.display(),
            json_path,
            err.into_inner(),
        )
    })
}

fn seconds_to_timestamp(secs: i64) -> Option<Timestamp> {
    Timestamp::from_second(secs).ok()
}

fn milliseconds_to_timestamp(ms: i64) -> Option<Timestamp> {
    Timestamp::from_millisecond(ms).ok()
}

#[cfg(test)]
mod tests {
    //! Unit tests that pin the *structural fidelity* of parser output, not
    //! just `len()`. The integration test in `tests/cli.rs` already asserts
    //! the count line printed by the binary; without these tests a parser
    //! regression that returns N empty-default `ShapeCEntry`s would
    //! print the right count and pass.
    //!
    //! Coverage spans both shape-C variants. The flat shape is pinned by
    //! `close_friends_parses_label_values` (URL/Name/Username at the outer
    //! `label_values` level) and by `hide_story_from_parses_as_single_entry`.
    //! The nested-`Owner` shape is pinned by
    //! `liked_posts_owner_username_extracts`, which exercises the
    //! three-level `label_values → title == "Owner" → dict[0].dict → label
    //! == "Username" → value` walk on two distinct entries.
    //! `owner_username_returns_none_without_owner_section` guards the
    //! accessor against silently promoting the outer-level `Username` that
    //! the relationship-flag files carry. Shape A is pinned by
    //! `liked_comments_extracts_username_and_timestamp` and the honest-
    //! count filter by `shape_a_entries_drops_empty_title`. Shape D is
    //! pinned by `post_comments_extracts_media_owner_and_time` (the
    //! `string_map_data → "Media Owner" → value` walk plus the `"Time" →
    //! timestamp` extraction) and `shape_d_entries_drops_empty_owner` (the
    //! honest-count filter). The `personal_information.json` parser is
    //! pinned by `me_identity_extracts_handle_and_name`, which catches a
    //! rename of either sub-key.
    use super::*;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_export")
    }

    #[test]
    fn close_friends_parses_label_values() {
        let entries = read_close_friends(&fixture_root()).expect("fixture parse");
        assert_eq!(entries.len(), 1, "fixture has one close friend");

        let entry = &entries[0];
        assert_eq!(entry.fbid.as_deref(), Some("1000000000000001"));
        assert_eq!(entry.timestamp, Some(1_700_300_000));

        // Three labels: URL, Name, Username. If serde silently skips
        // `label_values` (rename, type change), this drops to zero.
        let labels: Vec<&str> = entry
            .label_values
            .iter()
            .filter_map(|lv| lv.label.as_deref())
            .collect();
        assert_eq!(labels, ["URL", "Name", "Username"]);
    }

    #[test]
    fn hide_story_from_parses_as_single_entry() {
        let entry = read_hide_story_from(&fixture_root()).expect("fixture parse");
        assert_eq!(entry.fbid.as_deref(), Some("1000000000000006"));
        assert!(
            !entry.label_values.is_empty(),
            "fixture entry must carry label_values so the count line in \
             lib::run derives `1`, not `0`",
        );
    }

    #[test]
    fn liked_posts_owner_username_extracts() {
        let entries = read_liked_posts(&fixture_root()).expect("fixture parse");
        assert_eq!(entries.len(), 2, "fixture has two liked posts");

        // Pin the full three-level walk on the first entry. If serde silently
        // drops `dict` (rename, shape change), this drops to None.
        let owner = owner_username(&entries[0]);
        assert_eq!(owner, Some("jeremy_synth"));

        // Second entry exercises a distinct owner — guards against the helper
        // picking up a hard-coded match on a single label_values element.
        let other = owner_username(&entries[1]);
        assert_eq!(other, Some("maria_synth"));
    }

    #[test]
    fn owner_username_returns_none_without_owner_section() {
        // close_friends.json entries carry Username at the outer label_values
        // level, not nested under Owner. owner_username must NOT silently
        // promote that — it is the accessor for the nested-Owner shape only.
        let entries = read_close_friends(&fixture_root()).expect("fixture parse");
        assert!(owner_username(&entries[0]).is_none());
    }

    #[test]
    fn liked_comments_extracts_username_and_timestamp() {
        let entries = read_liked_comments(&fixture_root()).expect("fixture parse");
        assert_eq!(entries.len(), 2, "fixture has two liked comments");

        // Pin both `title → username` and `string_list_data[0].timestamp →
        // timestamp` extraction. If the wrapper-key field gets defaulted
        // (rename drift) the Vec drops to zero. If `RelationshipEntryRaw`
        // changes shape, these field reads fail.
        assert_eq!(entries[0].username, "first_target_synth");
        assert!(entries[0].timestamp.is_some());
        assert_eq!(entries[1].username, "second_target_synth");
    }

    #[test]
    fn shape_a_entries_drops_empty_title() {
        // Synthetic raw list: one valid entry, one with empty title (schema
        // drift signal). Honest-count posture: the empty-title entry must be
        // filtered out so `lib::run` count lines answer "how many real
        // signals" rather than "how many objects deserialized".
        let raw = vec![
            RelationshipEntryRaw {
                title: "ok_synth".to_owned(),
                string_list_data: vec![],
            },
            RelationshipEntryRaw {
                title: String::new(),
                string_list_data: vec![],
            },
        ];
        let out = shape_a_entries(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].username, "ok_synth");
    }

    #[test]
    fn post_comments_extracts_media_owner_and_time() {
        let entries = read_post_comments(&fixture_root()).expect("fixture parse");
        assert_eq!(entries.len(), 2, "fixture has two post comments");

        // Pin both `string_map_data["Media Owner"].value → target_username`
        // and `string_map_data["Time"].timestamp → timestamp` extraction.
        // If the rename on either sub-key drifts, the corresponding leaf
        // defaults to None and the honest-count filter in shape_d_entries
        // drops the entry — this test catches both regressions because the
        // expected len() is 2, not zero.
        assert_eq!(entries[0].target_username, "first_post_target_synth");
        assert!(entries[0].timestamp.is_some());
        assert_eq!(entries[1].target_username, "second_post_target_synth");
    }

    #[test]
    fn validate_shape_accepts_fixture() {
        validate_shape(&fixture_root()).expect("fixture must validate as a real export");
    }

    #[test]
    fn validate_shape_lists_every_missing_path() {
        // Empty tempdir — nothing IG-shaped exists. The error must
        // name all three top-level markers so the user sees the full
        // diagnosis in one message, not three sequential parser
        // failures.
        let empty = std::env::temp_dir().join(format!(
            "ig-mgr-validate-empty-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond(),
        ));
        std::fs::create_dir_all(&empty).expect("mktemp");
        let err = validate_shape(&empty).expect_err("empty dir must fail validation");
        let msg = err.to_string();
        std::fs::remove_dir(&empty).ok();
        assert!(
            msg.contains(FOLLOWING_FILE),
            "missing FOLLOWING_FILE: {msg}"
        );
        assert!(msg.contains(PERSONAL_INFO), "missing PERSONAL_INFO: {msg}");
        assert!(msg.contains(ACTIVITY_DIR), "missing ACTIVITY_DIR: {msg}");
    }

    #[test]
    fn validate_shape_partial_presence_names_only_the_missing() {
        // Dir contains the connections marker but neither PERSONAL_INFO
        // nor ACTIVITY_DIR. The error must NOT name FOLLOWING_FILE
        // (present) and MUST name both missing markers. A short-circuit
        // regression that bailed at the first missing path would still
        // pass `validate_shape_lists_every_missing_path` but fail here.
        let dir = std::env::temp_dir().join(format!(
            "ig-mgr-validate-partial-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond(),
        ));
        let following = dir.join(FOLLOWING_FILE);
        std::fs::create_dir_all(following.parent().unwrap()).expect("mktemp");
        std::fs::write(&following, "[]").expect("touch following");

        let err = validate_shape(&dir).expect_err("partial dir must fail");
        let msg = err.to_string();
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            !msg.contains(FOLLOWING_FILE),
            "present file must NOT be named in error: {msg}"
        );
        assert!(
            msg.contains(PERSONAL_INFO),
            "missing PERSONAL_INFO must be named: {msg}"
        );
        assert!(
            msg.contains(ACTIVITY_DIR),
            "missing ACTIVITY_DIR must be named: {msg}"
        );
    }

    #[test]
    fn me_identity_extracts_handle_and_name() {
        let me = read_me_identity(&fixture_root()).expect("fixture parse");
        // If "Username" or "Name" gets renamed in IG's export the
        // ProfileUserStringMap field defaults to ShapeDValue::default()
        // (value=None) and read_me_identity returns the hard-error
        // anyhow message — this test pins that the rename-checked field
        // walk currently succeeds against the synthetic fixture.
        assert_eq!(me.handle, "me_synth");
        assert_eq!(me.name, "Test User");
    }

    fn raw_profile(username: Option<&str>, name: Option<&str>) -> PersonalInfoFileRaw {
        PersonalInfoFileRaw {
            profile_user: vec![ProfileUserRaw {
                string_map_data: ProfileUserStringMap {
                    username: ShapeDValue {
                        value: username.map(str::to_owned),
                        timestamp: None,
                    },
                    name: ShapeDValue {
                        value: name.map(str::to_owned),
                        timestamp: None,
                    },
                },
            }],
        }
    }

    #[test]
    fn me_identity_errors_on_empty_profile_user() {
        // If IG ever ships personal_information.json with an empty
        // profile_user array (account in a strange state, export bug),
        // the run must fail loud — DM-direction classification has no
        // anchor without a `me` identity.
        let raw = PersonalInfoFileRaw {
            profile_user: Vec::new(),
        };
        let err = me_identity_from_raw(raw).expect_err("must hard-error");
        assert!(
            err.to_string().contains("`profile_user` is empty"),
            "error message must name the field: {err}",
        );
    }

    #[test]
    fn me_identity_errors_on_missing_or_empty_username() {
        for username in [None, Some("")] {
            let err = me_identity_from_raw(raw_profile(username, Some("Test User")))
                .expect_err("missing/empty Username must hard-error");
            assert!(
                err.to_string().contains("Username"),
                "error message must name `Username`: {err}",
            );
        }
    }

    #[test]
    fn me_identity_errors_on_missing_or_empty_name() {
        for name in [None, Some("")] {
            let err = me_identity_from_raw(raw_profile(Some("me_handle"), name))
                .expect_err("missing/empty Name must hard-error");
            assert!(
                err.to_string().contains("Name"),
                "error message must name `Name`: {err}",
            );
        }
    }

    #[test]
    fn me_identity_repairs_mojibake_on_name() {
        // IG's exporter ships display names as UTF-8 bytes mis-read as
        // Latin-1 codepoints (see `src/text.rs`). 'ü' UTF-8 = c3 bc →
        // mojibake reads as Ã (U+00C3) + ¼ (U+00BC). The mojibake form
        // is constructed byte-exactly here so the test is invariant to
        // whatever encoding the source file is saved as.
        let mojibake: String = [b'H', 0xc3, 0xbc, b's', b'e', b'y', b'i', b'n']
            .iter()
            .map(|&b| b as char)
            .collect();
        let me = me_identity_from_raw(raw_profile(Some("me_handle"), Some(&mojibake)))
            .expect("repair must not error");
        assert_eq!(me.name, "Hüseyin", "mojibake must be repaired on Me.name");
        assert_eq!(me.handle, "me_handle", "handle is ASCII — no transform");
    }

    #[test]
    fn read_inbox_repairs_mojibake_on_reaction_actor() {
        // The reaction `actor` is a display name joined against the
        // mojibake-fixed `me.name` to classify given-vs-received. If the
        // parser leaves `actor` un-repaired, a user whose own display name
        // carries non-ASCII bytes has their own reactions misclassified as
        // received. "HÃ¼seyin" is the Latin-1-misread form of "Hüseyin".
        let base = std::env::temp_dir().join(format!(
            "ig-mgr-actor-mojibake-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond(),
        ));
        let thread_dir = base.join("your_instagram_activity/messages/inbox/h_thread");
        std::fs::create_dir_all(&thread_dir).expect("mktemp thread dir");
        std::fs::write(
            thread_dir.join("message_1.json"),
            r#"{
                "participants": [{"name": "Partner"}, {"name": "HÃ¼seyin"}],
                "messages": [
                    {"sender_name": "Partner", "timestamp_ms": 1700000000000,
                     "reactions": [{"reaction": "love", "actor": "HÃ¼seyin"}]}
                ],
                "title": "Partner"
            }"#,
        )
        .expect("write thread part");

        let threads = read_inbox(&base).expect("read inbox");
        std::fs::remove_dir_all(&base).ok();

        let actor = threads[0].messages[0].reactions[0].actor.as_deref();
        assert_eq!(
            actor,
            Some("Hüseyin"),
            "reaction actor must be mojibake-repaired like sender_name",
        );
    }

    #[test]
    fn shape_d_entries_drops_empty_owner() {
        // Synthetic raw list: one valid entry, one with missing Media Owner
        // (schema drift), one with empty-string Media Owner. Both invalid
        // entries must be filtered so the count line answers "how many
        // real comments" rather than "how many objects deserialized" —
        // same posture as shape_a_entries_drops_empty_title.
        let raw = vec![
            ShapeDEntryRaw {
                string_map_data: ShapeDStringMap {
                    media_owner: ShapeDValue {
                        value: Some("ok_synth".to_owned()),
                        timestamp: None,
                    },
                    time: ShapeDValue {
                        value: None,
                        timestamp: Some(1_700_000_000),
                    },
                },
            },
            ShapeDEntryRaw {
                string_map_data: ShapeDStringMap::default(),
            },
            ShapeDEntryRaw {
                string_map_data: ShapeDStringMap {
                    media_owner: ShapeDValue {
                        value: Some(String::new()),
                        timestamp: None,
                    },
                    time: ShapeDValue::default(),
                },
            },
        ];
        let out = shape_d_entries(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target_username, "ok_synth");
        assert!(out[0].timestamp.is_some());
    }
}
