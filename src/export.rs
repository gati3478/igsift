//! Parsers for the Instagram personal data export (JSON).
//!
//! Schema was validated against the 2026-05-11 export on 2026-05-26 by walking
//! every JSON file with [`scripts/walk_export_schema.sh`](../scripts/walk_export_schema.sh).
//! Paths and field names below match what Instagram actually ships today.
//! Re-run the walker after every new export to detect drift.
//!
//! Implemented in this pass: `following.json`, `followers_*.json`, DM
//! threads under `messages/inbox/<thread>/message_*.json` and the same shape
//! under `messages/message_requests/<thread>/`, and the seven shape-C /
//! single-entry relationship-flag files (`close_friends`,
//! `profiles_you've_favorited`, `blocked_profiles`, `restricted_profiles`,
//! `hide_story_from`, `recently_unfollowed_profiles`, `removed_suggestions`).
//! Likes, comments, stories, saved, and shape-C `Owner` extraction are
//! deferred per `ROADMAP.md`.
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

/// One row from a "label-values" relationship file (shape **C** in DESIGN).
///
/// Backs `close_friends.json`, `profiles_you've_favorited.json`,
/// `blocked_profiles.json`, `restricted_profiles.json`,
/// `recently_unfollowed_profiles.json`, `removed_suggestions.json`, and the
/// single-entry `hide_story_from.json`. The username sits three levels deep
/// inside `label_values` (`label == "Username"`) — this slice keeps the raw
/// labels so the next slice (which adds `Owner` extraction for
/// `liked_posts.json` and friends) can reuse the same struct.
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
    //! Scope of this slice: assert the parser walks into nested arrays and
    //! preserves leaf values. `Owner` extraction (the three-level
    //! `label_values → dict → dict → Username` walk) is deferred to the
    //! slice that adds `liked_posts.json`.
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
}
