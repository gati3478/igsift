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
//! [`owner_username`] helper, and the eight shape-A activity files
//! (`liked_comments` and the seven `story_interactions/*` files) returning
//! [`ShapeAEntry`]. Shape-D comment files (`post_comments_*`,
//! `reels_comments`, `hype`) are deferred per `ROADMAP.md`.
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

/// One outbound shape-**A** activity entry — a like I gave on a comment, a
/// poll I voted on, a quiz I answered, an emoji slider I dragged, etc. The
/// target account (whose content I engaged with) is `username`; the activity
/// timestamp comes from `string_list_data[0].timestamp` on the raw entry.
///
/// Backs all eight shape-A activity files
/// (`likes/liked_comments.json` and the seven
/// `story_interactions/*.json`). The raw entry shape is identical across
/// the eight files — only the JSON wrapper key differs (see the
/// per-file private structs).
#[derive(Debug, Clone)]
pub struct ShapeAEntry {
    pub username: String,
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

// ── Shape-A wrapper structs ──────────────────────────────────────────────────
//
// Each shape-A file is `{<wrapper_key>: [{title, string_list_data}, ...]}`.
// The interior entry shape is identical across all eight (it's the same as
// `following.json` — the wrapper-key naming is the only delta), so the
// existing `RelationshipEntryRaw` deserializer is reused for the entry body.
// One struct per file keeps the wrapper key as a compile-checked field name:
// if Instagram renames the key on a future export, the parse fails loudly at
// the offending JSON path via `serde_path_to_error` instead of silently
// returning an empty Vec.

#[derive(Debug, Deserialize)]
struct LikedCommentsFileRaw {
    #[serde(default)]
    likes_comment_likes: Vec<RelationshipEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct StoryPollsFileRaw {
    #[serde(default)]
    story_activities_polls: Vec<RelationshipEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct StoryQuizzesFileRaw {
    #[serde(default)]
    story_activities_quizzes: Vec<RelationshipEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct StoryQuestionsFileRaw {
    #[serde(default)]
    story_activities_questions: Vec<RelationshipEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct StoryEmojiSlidersFileRaw {
    #[serde(default)]
    story_activities_emoji_sliders: Vec<RelationshipEntryRaw>,
}

// Note the IG inconsistency: the file is `emoji_story_reactions.json` but the
// internal wrapper key is `story_activities_emoji_quick_reactions` — not the
// symmetric "emoji_story_reactions" name. Codified explicitly so a future
// reader does not assume a 1:1 file-name ↔ wrapper-key mapping.
#[derive(Debug, Deserialize)]
struct StoryEmojiReactionsFileRaw {
    #[serde(default)]
    story_activities_emoji_quick_reactions: Vec<RelationshipEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct StoryReactionStickersFileRaw {
    #[serde(default)]
    story_activities_reaction_sticker_reactions: Vec<RelationshipEntryRaw>,
}

#[derive(Debug, Deserialize)]
struct StoryCountdownsFileRaw {
    #[serde(default)]
    story_activities_countdowns: Vec<RelationshipEntryRaw>,
}

// ── Public readers ───────────────────────────────────────────────────────────

/// Parse `connections/followers_and_following/following.json` (shape A).
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

/// Parse `your_instagram_activity/story_interactions/polls.json` — shape
/// **A**, wrapper key `story_activities_polls`. Each entry is a poll vote I
/// cast on someone's story.
pub fn read_story_polls(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let raw: StoryPollsFileRaw = parse_json(&export_dir.join(STORY_POLLS))?;
    Ok(shape_a_entries(raw.story_activities_polls))
}

/// Parse `your_instagram_activity/story_interactions/quizzes.json` — shape
/// **A**, wrapper key `story_activities_quizzes`.
pub fn read_story_quizzes(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let raw: StoryQuizzesFileRaw = parse_json(&export_dir.join(STORY_QUIZZES))?;
    Ok(shape_a_entries(raw.story_activities_quizzes))
}

/// Parse `your_instagram_activity/story_interactions/questions.json` — shape
/// **A**, wrapper key `story_activities_questions`.
pub fn read_story_questions(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let raw: StoryQuestionsFileRaw = parse_json(&export_dir.join(STORY_QUESTIONS))?;
    Ok(shape_a_entries(raw.story_activities_questions))
}

/// Parse `your_instagram_activity/story_interactions/emoji_sliders.json` —
/// shape **A**, wrapper key `story_activities_emoji_sliders`.
pub fn read_story_emoji_sliders(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let raw: StoryEmojiSlidersFileRaw = parse_json(&export_dir.join(STORY_EMOJI_SLIDERS))?;
    Ok(shape_a_entries(raw.story_activities_emoji_sliders))
}

/// Parse `your_instagram_activity/story_interactions/emoji_story_reactions.json`
/// — shape **A**, wrapper key `story_activities_emoji_quick_reactions`
/// (file name and wrapper key are NOT symmetric — see the wrapper struct
/// comment).
pub fn read_story_emoji_reactions(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let raw: StoryEmojiReactionsFileRaw = parse_json(&export_dir.join(STORY_EMOJI_REACTIONS))?;
    Ok(shape_a_entries(raw.story_activities_emoji_quick_reactions))
}

/// Parse
/// `your_instagram_activity/story_interactions/story_reaction_sticker_reactions.json`
/// — shape **A**, wrapper key
/// `story_activities_reaction_sticker_reactions`.
pub fn read_story_reaction_stickers(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let raw: StoryReactionStickersFileRaw = parse_json(&export_dir.join(STORY_REACTION_STICKERS))?;
    Ok(shape_a_entries(
        raw.story_activities_reaction_sticker_reactions,
    ))
}

/// Parse `your_instagram_activity/story_interactions/countdowns.json` —
/// shape **A**, wrapper key `story_activities_countdowns`.
pub fn read_story_countdowns(export_dir: &Path) -> Result<Vec<ShapeAEntry>> {
    let raw: StoryCountdownsFileRaw = parse_json(&export_dir.join(STORY_COUNTDOWNS))?;
    Ok(shape_a_entries(raw.story_activities_countdowns))
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

        let parts = thread_part_paths(&thread_dir)?;
        if parts.is_empty() {
            continue;
        }

        let mut messages = Vec::new();
        let mut title: Option<String> = None;
        let mut participants: Vec<String> = Vec::new();

        for path in &parts {
            let raw: ThreadFileRaw = parse_json(path)?;
            if title.is_none() {
                title = raw.title;
                participants = raw.participants.into_iter().map(|p| p.name).collect();
            }
            messages.extend(raw.messages.into_iter().map(|m| {
                DmMessage {
                    sender: m.sender_name,
                    timestamp: m.timestamp_ms.and_then(milliseconds_to_timestamp),
                    content: m.content,
                    reactions: m
                        .reactions
                        .into_iter()
                        .map(|r| DmReaction {
                            reaction: r.reaction,
                            actor: r.actor,
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

/// Read `message_1.json`, `message_2.json`, … sorted by numeric suffix.
/// Falling back to lexicographic sort would put `message_10.json` before
/// `message_2.json` — relevant for the `validoli` thread (10 parts).
fn thread_part_paths(thread_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut parts: Vec<(u32, PathBuf)> = std::fs::read_dir(thread_dir)
        .with_context(|| format!("reading {}", thread_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter_map(|path| {
            let stem = path.file_stem().and_then(|s| s.to_str())?;
            let ext = path.extension().and_then(|s| s.to_str())?;
            if ext != "json" {
                return None;
            }
            let suffix = stem.strip_prefix("message_")?;
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
    //! the relationship-flag files carry.
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
}
