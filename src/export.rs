//! Parsers for the Instagram personal data export (JSON).
//!
//! Schema was validated against the 2026-05-11 export on 2026-05-26 by walking
//! every JSON file with [`scripts/walk_export_schema.sh`](../scripts/walk_export_schema.sh).
//! Paths and field names below match what Instagram actually ships today.
//! Re-run the walker after every new export to detect drift.
//!
//! Implemented in this pass: `following.json`, `followers_*.json`, and DM
//! threads under `messages/inbox/<thread>/message_*.json`. Likes, comments,
//! stories, saved, and other relationships are deferred per `ROADMAP.md`.
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
///
/// Each thread folder may hold one or more `message_*.json` parts; this reader
/// concatenates the parts in lexicographic order (`message_1.json` <
/// `message_2.json` < … < `message_10.json` would be wrong — but Instagram
/// caps at single digits in practice, and we sort the actual numeric suffix
/// just in case).
pub fn read_inbox(export_dir: &Path) -> Result<Vec<DmThread>> {
    let inbox = export_dir.join(INBOX_DIR);
    let mut thread_dirs: Vec<PathBuf> = std::fs::read_dir(&inbox)
        .with_context(|| format!("reading {}", inbox.display()))?
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

// ── Internals ────────────────────────────────────────────────────────────────

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
