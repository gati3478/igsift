//! Per-account feature aggregation — handle-keyed half (slice 7A) plus
//! raw DM features (slice 7B-1).
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
//! Slice 7B-1 added:
//! - raw DM features per followee whose 1:1 inbox thread resolves through
//!   [`crate::features::name_resolution`]: `dm_messages_total`,
//!   `dm_recency_days`, `dm_balance`, `dm_reactions_given`,
//!   `dm_reactions_received`,
//! - the `inbound_dm_request` boolean from a resolved thread in
//!   `messages/message_requests/`.
//!
//! Group chats (≥ 3 participants) and abandoned threads (only `me` as
//! participant) are dropped entirely — DESIGN.md "DM display_name ↔ handle
//! bridge" makes both exclusions explicit. Threads whose resolver call
//! returns `None` (unknown display name OR colliding name) do not credit
//! any followee — misattribution is worse than missing attribution.
//!
//! Slice 7B-2 added (from `[decay]` in `config/scoring.toml`):
//! - `*_decayed: f64` versions of every count feature, where each entry
//!   contributes `exp(-Δt_days / τ_days)` to the running sum
//!   (`tau_content_days` for activity, `tau_dm_days` for DM signals).
//!   Entries with no timestamp, or with a future timestamp (clock skew),
//!   contribute `0.0` — same honest-counting posture as the raw counts.
//! - four `*_90d` / `*_180d` raw windowed counts matching DESIGN.md's CSV
//!   header (`likes_given_90d`, `comments_given_90d`,
//!   `dm_reactions_given_180d`, `dm_reactions_received_180d`). The
//!   window is half-open: `secs / 86_400 < days`. DESIGN.md is explicit
//!   these are *different aggregations* from the decay-weighted score
//!   inputs — the CSV emits both because they answer different questions.
//!
//! Reactions don't carry their own timestamps in the export, so the parent
//! message's timestamp drives both decay and the 180d window for the two
//! reaction counters.
//!
//! Honest-counting posture inherited from the parsers: an activity entry
//! whose target handle is not in the followings set is silently dropped at
//! aggregation time, mirroring "filter to followings is the scope" from
//! DESIGN.md. The structural unit tests below pin that semantic.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;

use crate::config::DecayConfig;
use crate::export::{
    CommentEntry, DmThread, FollowerEntry, FollowingEntry, MeIdentity, ShapeAEntry, ShapeCEntry,
    owner_username,
};
use crate::features::account_class::Classifier;
use crate::features::name_resolution::NameResolver;

/// Account class — DESIGN.md gates the `Unfollow` bucket on
/// `account_class == Personal`. The brand-detection heuristic lives in
/// [`crate::features::account_class::Classifier`] and stamps `Brand` onto
/// any followee whose handle or display name hits the lexicon. Surfaced in
/// the CSV `account_class` column for human triage.
///
/// `PublicFigure` is deliberately omitted from the variant set: the
/// username/display-name heuristic can't reliably tell brand from
/// public_figure from text alone, and the downstream gating is identical
/// (block Unfollow, surface as Review). Adding a variant we can't populate
/// would be a lie about what the aggregator knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountClass {
    #[default]
    Personal,
    Brand,
}

impl AccountClass {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountClass::Personal => "personal",
            AccountClass::Brand => "brand",
        }
    }
}

/// One row of per-account features. Slice 7A populates the handle-keyed
/// fields; DM-derived fields are defaulted to zero / `None` and filled in
/// slice 7B. See [`docs/DESIGN.md`](../../../docs/DESIGN.md) ("Per-account
/// features") for the feature semantics.
#[derive(Debug, Clone)]
pub struct AccountFeatures {
    pub username: String,
    /// Display name resolved via [`NameResolver::display_name_for`]. `None`
    /// when the handle doesn't appear in any `label_values` file or when
    /// it appears with multiple distinct names (collision policy mirrors
    /// the forward direction). CSV emits empty string for `None`.
    pub display_name: Option<String>,
    pub account_class: AccountClass,
    pub follow_tenure_days: Option<u32>,
    /// Days since the relationship became **mutual** — the later of {you
    /// followed them, they followed you back}. `None` when not mutual, or
    /// when either follow lacks a timestamp (an undatable relationship is
    /// not auto-kept by the deep-mutual floor in [`crate::scoring`]). Drawn
    /// from `following.json` + `followers_*.json` timestamps; distinct from
    /// `follow_tenure_days` (your-follow date only) — the two differ for the
    /// ~19% of mutuals whose follow-back lagged your follow.
    pub mutual_age_days: Option<u32>,

    pub is_close_friend: bool,
    pub is_favorited: bool,
    pub is_blocked: bool,
    pub is_restricted: bool,
    pub is_hide_story_from: bool,
    pub is_removed_suggestion: bool,
    pub recently_unfollowed: bool,
    /// `true` iff the followee also appears in `followers_*.json` —
    /// the relationship is reciprocal. Carried for decision support
    /// only (surfaces in CSV + MD card "one-sided?" hint); not used
    /// by scoring, which intentionally treats one-sided follows
    /// neutrally. The CLAUDE.md "one_sided_them_is_not_a_penalty"
    /// scoring test pins that policy.
    pub is_mutual: bool,
    /// Handle is in `config/keeplist.txt` — user-maintained
    /// never-unfollow override. Parallel signal to `is_close_friend` /
    /// `is_favorited`, gated at [`crate::scoring::assign_bucket`] to floor
    /// the bucket at Review. NOT classification — a personal close friend
    /// the user has keeplisted stays `account_class == Personal` so the
    /// column doesn't misrepresent their profile.
    pub is_keeplisted: bool,
    /// Handle is in `config/droplist.txt` — user-maintained always-
    /// unfollow override, the exact inverse of `is_keeplisted`.
    /// Forces [`crate::scoring::assign_bucket`] to Unfollow regardless of
    /// score or keep-signals (the one exception: `is_restricted` still
    /// floors at Review). Cannot co-occur with `is_keeplisted` —
    /// [`crate::lists::ensure_disjoint`] rejects a both-listed handle
    /// at load. Like the keeplist, NOT classification.
    pub is_droplisted: bool,

    pub likes_given: u32,
    pub comments_given: u32,
    pub story_interactions_out: u32,
    pub stories_viewed: u32,
    pub saved_their_content: u32,

    pub dm_messages_total: u32,
    pub dm_recency_days: Option<u32>,
    /// Outbound / (outbound + inbound) over messages with a classifiable
    /// sender. `None` ⇔ no classifiable senders (zero-message thread, or
    /// every message had `sender_name = None`). `Some(1.0)` is fully
    /// one-sided me; `Some(0.0)` fully one-sided them; `Some(0.5)`
    /// balanced. The scoring layer's `dm_balance_penalty` should gate on
    /// volume (`dm_messages_total`) — `Some(0.5)` over 2 greetings is
    /// not the same relationship as `Some(0.5)` over 1000 messages.
    pub dm_balance: Option<f32>,
    pub dm_reactions_given: u32,
    pub dm_reactions_received: u32,
    pub inbound_dm_request: bool,

    pub likes_given_decayed: f64,
    pub comments_given_decayed: f64,
    pub story_interactions_out_decayed: f64,
    pub stories_viewed_decayed: f64,
    pub saved_their_content_decayed: f64,
    pub dm_messages_total_decayed: f64,
    pub dm_reactions_given_decayed: f64,
    pub dm_reactions_received_decayed: f64,

    pub likes_given_90d: u32,
    pub comments_given_90d: u32,
    pub dm_reactions_given_180d: u32,
    pub dm_reactions_received_180d: u32,
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
    /// Used only to populate [`AccountFeatures::is_mutual`] via a set
    /// intersection with `followings`. Not consumed by scoring.
    pub followers: &'a [FollowerEntry],

    pub close_friends: &'a [ShapeCEntry],
    pub favorited: &'a [ShapeCEntry],
    pub blocked: &'a [ShapeCEntry],
    pub restricted: &'a [ShapeCEntry],
    pub hide_story_from: &'a ShapeCEntry,
    pub recently_unfollowed: &'a [ShapeCEntry],
    pub removed_suggestions: &'a [ShapeCEntry],

    pub liked_posts: &'a [ShapeCEntry],
    pub liked_comments: &'a [ShapeAEntry],
    /// Story likes — shape-C-with-`Owner`, folded into
    /// `story_interactions_out` per DESIGN.md ("all `story_interactions/*`
    /// aggregated"). Keyed via `owner_username`, unlike the seven shape-A
    /// `story_*` files keyed on `entry.username`.
    pub story_likes: &'a [ShapeCEntry],
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

    pub inbox_threads: &'a [DmThread],
    pub message_request_threads: &'a [DmThread],
    pub me: &'a MeIdentity,
    pub resolver: &'a NameResolver,
    pub classifier: &'a Classifier,

    pub decay: &'a DecayConfig,
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
    let follower_handles: HashSet<&str> = inputs
        .followers
        .iter()
        .map(|f| f.username.as_str())
        .collect();
    // When-they-followed-back, keyed by handle — feeds `mutual_age_days`.
    // Followers without a timestamp simply don't appear, so the relationship
    // stays undatable (and the deep-mutual floor stays off) for them.
    let follower_since: HashMap<&str, Timestamp> = inputs
        .followers
        .iter()
        .filter_map(|f| f.followed_me_at.map(|ts| (f.username.as_str(), ts)))
        .collect();

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
        let display_name = inputs.resolver.display_name_for(handle);
        let features = AccountFeatures {
            username: f.username.clone(),
            display_name: display_name.map(|s| s.to_owned()),
            account_class: inputs.classifier.classify(handle, display_name),
            follow_tenure_days: f.followed_at.and_then(|ts| days_since(ts, now)),
            mutual_age_days: mutual_age_days(
                follower_handles.contains(handle),
                f.followed_at,
                follower_since.get(handle).copied(),
                now,
            ),
            is_close_friend: close_friend.contains(handle),
            is_favorited: favorited.contains(handle),
            is_blocked: false,
            is_restricted: restricted.contains(handle),
            is_hide_story_from: hide_story_target == Some(handle),
            is_removed_suggestion: removed_suggestion.contains(handle),
            recently_unfollowed: false,
            is_mutual: follower_handles.contains(handle),
            is_keeplisted: inputs.classifier.is_keeplisted(handle),
            is_droplisted: inputs.classifier.is_droplisted(handle),
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
            likes_given_decayed: 0.0,
            comments_given_decayed: 0.0,
            story_interactions_out_decayed: 0.0,
            stories_viewed_decayed: 0.0,
            saved_their_content_decayed: 0.0,
            dm_messages_total_decayed: 0.0,
            dm_reactions_given_decayed: 0.0,
            dm_reactions_received_decayed: 0.0,
            likes_given_90d: 0,
            comments_given_90d: 0,
            dm_reactions_given_180d: 0,
            dm_reactions_received_180d: 0,
        };
        by_handle.insert(handle, features);
    }

    let tau_content = inputs.decay.tau_content_days;
    let tau_dm = inputs.decay.tau_dm_days;

    for entry in inputs.liked_posts {
        if let Some(target) = owner_username(entry)
            && let Some(features) = by_handle.get_mut(target)
        {
            let ts = shape_c_timestamp(entry);
            features.likes_given += 1;
            features.likes_given_decayed += decay_weight(ts, now, tau_content);
            if within_window(ts, now, 90) {
                features.likes_given_90d += 1;
            }
        }
    }
    for entry in inputs.liked_comments {
        if let Some(features) = by_handle.get_mut(entry.username.as_str()) {
            features.likes_given += 1;
            features.likes_given_decayed += decay_weight(entry.timestamp, now, tau_content);
            if within_window(entry.timestamp, now, 90) {
                features.likes_given_90d += 1;
            }
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
            features.comments_given_decayed += decay_weight(entry.timestamp, now, tau_content);
            if within_window(entry.timestamp, now, 90) {
                features.comments_given_90d += 1;
            }
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
            features.story_interactions_out_decayed +=
                decay_weight(entry.timestamp, now, tau_content);
        }
    }

    // Story likes feed the same `story_interactions_out` feature as the
    // seven shape-A files above, but ship shape-C-with-`Owner` (same as
    // stories_viewed / saved_posts) so the target handle comes from
    // `owner_username`, not `entry.username`.
    for entry in inputs.story_likes {
        if let Some(target) = owner_username(entry)
            && let Some(features) = by_handle.get_mut(target)
        {
            features.story_interactions_out += 1;
            features.story_interactions_out_decayed +=
                decay_weight(shape_c_timestamp(entry), now, tau_content);
        }
    }

    for entry in inputs.stories_viewed {
        if let Some(target) = owner_username(entry)
            && let Some(features) = by_handle.get_mut(target)
        {
            features.stories_viewed += 1;
            features.stories_viewed_decayed +=
                decay_weight(shape_c_timestamp(entry), now, tau_content);
        }
    }

    for entry in inputs.saved_posts {
        if let Some(target) = owner_username(entry)
            && let Some(features) = by_handle.get_mut(target)
        {
            features.saved_their_content += 1;
            features.saved_their_content_decayed +=
                decay_weight(shape_c_timestamp(entry), now, tau_content);
        }
    }

    apply_dm_features(&mut by_handle, inputs, now, tau_dm);

    by_handle.into_values().collect()
}

/// Slice 7B-1 DM aggregation pass.
///
/// Walks `inbox_threads` to populate `dm_messages_total`,
/// `dm_recency_days`, `dm_balance`, `dm_reactions_given`,
/// `dm_reactions_received` on each followee with a resolvable 1:1 thread;
/// walks `message_request_threads` to flip `inbound_dm_request`.
///
/// `outbound`/`inbound`/`latest` accumulate in a sidecar [`HashMap`] (not
/// on `AccountFeatures`) so multiple threads attributing to the same handle
/// — which can happen when display-name aliases in the seven `label_values`
/// files all resolve to one handle — compose to one correct `dm_balance`
/// and one correct `dm_recency_days` at finalization, rather than the
/// last-thread-wins behaviour a per-thread-finalize loop would produce.
fn apply_dm_features<'a>(
    by_handle: &mut HashMap<&'a str, AccountFeatures>,
    inputs: &AggregateInputs<'a>,
    now: Timestamp,
    tau_dm: u32,
) {
    for thread in inputs.message_request_threads {
        if let Some(handle) = attributable_handle(thread, &inputs.me.name, inputs.resolver)
            && let Some(features) = by_handle.get_mut(handle)
        {
            features.inbound_dm_request = true;
        }
    }

    let mut accum: HashMap<&str, DmAccum> = HashMap::new();
    for thread in inputs.inbox_threads {
        let Some(handle) = attributable_handle(thread, &inputs.me.name, inputs.resolver) else {
            continue;
        };
        let Some(features) = by_handle.get_mut(handle) else {
            continue;
        };
        let acc = accum.entry(handle).or_default();
        walk_inbox_thread(thread, features, acc, &inputs.me.name, now, tau_dm);
    }

    for (handle, acc) in &accum {
        let Some(features) = by_handle.get_mut(*handle) else {
            continue;
        };
        let total = acc.outbound + acc.inbound;
        if total > 0 {
            features.dm_balance = Some(acc.outbound as f32 / total as f32);
        }
        if let Some(latest) = acc.latest {
            features.dm_recency_days = days_since(latest, now);
        }
    }
}

/// Tracks the per-handle running totals that don't compose by addition.
///
/// `dm_messages_total` and the two reaction counters compose by addition
/// (we write straight to `AccountFeatures`), but `dm_balance` is a ratio
/// and `dm_recency_days` is a max — both need access to the cross-thread
/// pre-image, not the running result.
#[derive(Debug, Default)]
struct DmAccum {
    outbound: u32,
    inbound: u32,
    latest: Option<Timestamp>,
}

/// Resolve a thread to its (single, non-me, mapped-by-resolver) handle.
///
/// Returns `None` for group chats (≥ 2 others), abandoned threads (0
/// others), and any thread whose other-party display name is unknown to
/// the resolver or maps to multiple handles (collision). Matches DESIGN.md
/// "DM display_name ↔ handle bridge" exclusions.
///
/// `pub(crate)` so [`crate::run`]'s `resolvable DM threads` sanity count
/// can use the same predicate as the aggregator — single source of truth
/// for the 1:1 / resolved / non-collision filter.
pub(crate) fn attributable_handle<'r>(
    thread: &DmThread,
    me_name: &str,
    resolver: &'r NameResolver,
) -> Option<&'r str> {
    let mut others = thread.participants.iter().filter(|p| p.as_str() != me_name);
    let first = others.next()?;
    if others.next().is_some() {
        return None;
    }
    resolver.resolve(first)
}

fn walk_inbox_thread(
    thread: &DmThread,
    features: &mut AccountFeatures,
    acc: &mut DmAccum,
    me_name: &str,
    now: Timestamp,
    tau_dm: u32,
) {
    for msg in &thread.messages {
        // Reactions don't carry their own timestamps in the export — the
        // parent message's timestamp drives both the decay weight and the
        // 180d window predicate. A reaction is approximately contemporaneous
        // with the message it's on, so the message's decay weight is reused
        // for the message total AND every reaction on it (one `exp()` per
        // message, not one per message plus one per reaction-loop entry).
        let decayed = decay_weight(msg.timestamp, now, tau_dm);
        features.dm_messages_total += 1;
        features.dm_messages_total_decayed += decayed;

        match msg.sender.as_deref() {
            Some(s) if s == me_name => acc.outbound += 1,
            Some(_) => acc.inbound += 1,
            None => {}
        }

        if let Some(ts) = msg.timestamp {
            acc.latest = Some(match acc.latest {
                Some(prev) if prev >= ts => prev,
                _ => ts,
            });
        }

        let in_180d = within_window(msg.timestamp, now, 180);
        for r in &msg.reactions {
            match r.actor.as_deref() {
                Some(s) if s == me_name => {
                    features.dm_reactions_given += 1;
                    features.dm_reactions_given_decayed += decayed;
                    if in_180d {
                        features.dm_reactions_given_180d += 1;
                    }
                }
                Some(_) => {
                    features.dm_reactions_received += 1;
                    features.dm_reactions_received_decayed += decayed;
                    if in_180d {
                        features.dm_reactions_received_180d += 1;
                    }
                }
                None => {}
            }
        }
    }
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

/// Whole days between `earlier` and `now` as a non-negative `u32`.
///
/// `None` when `earlier > now` (clock skew on the export job, hand-edited
/// fixtures) — the duration arithmetic must not panic or wrap. Backs
/// `follow_tenure_days` and `dm_recency_days`.
fn days_since(earlier: Timestamp, now: Timestamp) -> Option<u32> {
    let secs = now.duration_since(earlier).as_secs();
    if secs < 0 {
        return None;
    }
    u32::try_from(secs / 86_400).ok()
}

/// Days since a reciprocal follow became mutual — the later of {you followed
/// them, they followed you back}.
///
/// `None` when the account isn't mutual, or when either follow lacks a
/// timestamp: an undatable relationship must not satisfy the deep-mutual
/// keep-floor in [`crate::scoring`]. Taking the **later** (`max`) of the two
/// follows is the load-bearing choice — the relationship has only been
/// reciprocal since both follows were in place, so a years-old one-way follow
/// that was reciprocated last month is a one-month-old *mutual* relationship.
fn mutual_age_days(
    is_mutual: bool,
    you_followed: Option<Timestamp>,
    they_followed: Option<Timestamp>,
    now: Timestamp,
) -> Option<u32> {
    if !is_mutual {
        return None;
    }
    days_since(you_followed?.max(they_followed?), now)
}

/// Exponential decay weight `exp(-Δt_days / τ_days)` ∈ (0, 1].
///
/// Missing timestamps and future timestamps both yield `0.0` — same posture
/// as `days_since`'s `None` return: an entry without a usable timestamp
/// contributes no weight to the decayed sum rather than full weight.
fn decay_weight(timestamp: Option<Timestamp>, now: Timestamp, tau_days: u32) -> f64 {
    let Some(ts) = timestamp else {
        return 0.0;
    };
    let secs = now.duration_since(ts).as_secs();
    if secs < 0 {
        return 0.0;
    }
    let dt_days = secs as f64 / 86_400.0;
    (-dt_days / f64::from(tau_days)).exp()
}

/// Half-open day window: `true` iff `0 ≤ Δt_days < window_days`.
///
/// Missing or future timestamps return `false`. Matches the DESIGN.md CSV
/// columns `*_90d` / `*_180d`: "in the last N days, exclusive at the
/// boundary".
fn within_window(timestamp: Option<Timestamp>, now: Timestamp, window_days: u32) -> bool {
    let Some(ts) = timestamp else {
        return false;
    };
    let secs = now.duration_since(ts).as_secs();
    if secs < 0 {
        return false;
    }
    secs / 86_400 < i64::from(window_days)
}

/// Shape-C entries carry the raw Unix-seconds `timestamp: Option<i64>` at
/// the parser boundary; shape-A and shape-D have already lifted to
/// `Option<Timestamp>`. This helper lifts shape-C so the decay/window
/// helpers see a uniform input type.
fn shape_c_timestamp(entry: &ShapeCEntry) -> Option<Timestamp> {
    entry.timestamp.and_then(|s| Timestamp::from_second(s).ok())
}

#[cfg(test)]
mod tests {
    //! Structural tests on synthetic parser outputs — no fixture I/O.
    //!
    //! Pins the four invariants the slice-7A spec calls out (filter-to-
    //! followings, blocked/recently_unfollowed input-set exclusion, boolean
    //! flag population from the seven `label_values` files, activity-count
    //! summation across the five source-groups) plus the slice-7B-1 DM
    //! semantics: 1:1 resolver gating, group/abandoned-thread exclusion,
    //! direction classification, reaction direction, `dm_balance` /
    //! `dm_recency_days` finalization across multiple threads to the same
    //! handle, and `inbound_dm_request` from `message_requests/`.
    //! `follow_tenure_days` and `dm_recency_days` are pinned against a
    //! fixed `now` so the tests never drift against real time.
    use super::*;
    use crate::export::{
        DmMessage, DmReaction, ShapeCInnerEntry, ShapeCInnerGroup, ShapeCLabelValue,
    };

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
        me: &'a MeIdentity,
        resolver: &'a NameResolver,
        classifier: &'a Classifier,
        decay: &'a DecayConfig,
    ) -> AggregateInputs<'a> {
        AggregateInputs {
            followings,
            followers: &[],
            close_friends: &[],
            favorited: &[],
            blocked: &[],
            restricted: &[],
            hide_story_from,
            recently_unfollowed: &[],
            removed_suggestions: &[],
            liked_posts: &[],
            liked_comments: &[],
            story_likes: &[],
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
            inbox_threads: &[],
            message_request_threads: &[],
            me,
            resolver,
            classifier,
            decay,
        }
    }

    /// Build a synthetic [`Classifier`] with no keeplist entries. Tests
    /// that only exercise the existing flag/activity paths use this; tests
    /// that exercise keeplist behaviour build their own.
    fn synth_classifier() -> Classifier {
        Classifier::new(HashSet::new(), HashSet::new())
    }

    fn synth_decay() -> DecayConfig {
        DecayConfig {
            tau_dm_days: 180,
            tau_content_days: 365,
        }
    }

    fn synth_me() -> MeIdentity {
        MeIdentity {
            handle: "me_handle".to_owned(),
            name: "Me Real".to_owned(),
        }
    }

    fn name_pair(name: &str, handle: &str) -> ShapeCEntry {
        ShapeCEntry {
            fbid: None,
            timestamp: None,
            label_values: vec![
                ShapeCLabelValue {
                    label: Some("Name".to_owned()),
                    value: Some(name.to_owned()),
                    title: None,
                    dict: Vec::new(),
                },
                ShapeCLabelValue {
                    label: Some("Username".to_owned()),
                    value: Some(handle.to_owned()),
                    title: None,
                    dict: Vec::new(),
                },
            ],
        }
    }

    fn resolver_from(pairs: &[(&str, &str)]) -> NameResolver {
        let entries: Vec<ShapeCEntry> = pairs.iter().map(|(n, h)| name_pair(n, h)).collect();
        NameResolver::build(&[&entries])
    }

    fn dm_thread(participants: &[&str], messages: Vec<DmMessage>) -> DmThread {
        DmThread {
            folder: String::new(),
            title: None,
            participants: participants.iter().map(|s| (*s).to_owned()).collect(),
            messages,
        }
    }

    fn msg(sender: Option<&str>, ts_secs: Option<i64>, reactions: Vec<DmReaction>) -> DmMessage {
        DmMessage {
            sender: sender.map(str::to_owned),
            timestamp: ts_secs.and_then(|s| Timestamp::from_second(s).ok()),
            content: None,
            reactions,
        }
    }

    fn react(actor: Option<&str>) -> DmReaction {
        DmReaction {
            reaction: Some("heart".to_owned()),
            actor: actor.map(str::to_owned),
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
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
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
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
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
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
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
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

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

        // alice gets 1 entry across each of the 7 story shape-A files,
        // plus 1 shape-C-with-Owner story_like → story_interactions_out = 8.
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

        // Story likes are shape-C-with-Owner and must fold into the same
        // story_interactions_out feature as the shape-A files above.
        let story_likes = vec![owner_entry("alice")];
        inputs.story_likes = &story_likes;

        let stories_viewed = vec![owner_entry("alice"), owner_entry("alice")];
        let saved_posts = vec![owner_entry("bob")];
        inputs.stories_viewed = &stories_viewed;
        inputs.saved_posts = &saved_posts;

        let by = by_username(aggregate(&inputs, fixed_now()));
        let alice = &by["alice"];
        let bob = &by["bob"];

        assert_eq!(alice.likes_given, 3);
        assert_eq!(alice.comments_given, 0);
        assert_eq!(
            alice.story_interactions_out, 8,
            "7 shape-A story interactions + 1 shape-C story_like",
        );
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
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

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
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

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
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
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
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
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

    // ── slice 7B-1: DM features ───────────────────────────────────────────

    #[test]
    fn group_chat_is_dropped_from_dm_aggregation() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        // Three participants including me — `attributable_handle` must
        // refuse to single out Alice as the partner.
        let threads = vec![dm_thread(
            &["Alice Real", "Bob Real", "Me Real"],
            vec![msg(Some("Alice Real"), Some(1_700_000_000), vec![])],
        )];
        inputs.inbox_threads = &threads;

        let by = by_username(aggregate(&inputs, fixed_now()));
        assert_eq!(by["alice"].dm_messages_total, 0);
        assert_eq!(by["alice"].dm_balance, None);
        assert_eq!(by["alice"].dm_recency_days, None);
    }

    #[test]
    fn abandoned_thread_is_dropped_from_dm_aggregation() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        // Only me in participants — `attributable_handle` returns None
        // (no `first` other-participant to resolve).
        let threads = vec![dm_thread(
            &["Me Real"],
            vec![msg(Some("Me Real"), Some(1_700_000_000), vec![])],
        )];
        inputs.inbox_threads = &threads;

        let by = by_username(aggregate(&inputs, fixed_now()));
        assert_eq!(by["alice"].dm_messages_total, 0);
    }

    #[test]
    fn unresolvable_display_name_credits_no_followee() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        // Resolver has Alice mapped, but the thread uses a display name
        // (`Stranger`) that the resolver has never seen.
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let threads = vec![dm_thread(
            &["Stranger", "Me Real"],
            vec![msg(Some("Stranger"), Some(1_700_000_000), vec![])],
        )];
        inputs.inbox_threads = &threads;

        let by = by_username(aggregate(&inputs, fixed_now()));
        assert_eq!(by["alice"].dm_messages_total, 0);
    }

    #[test]
    fn colliding_display_name_credits_no_followee() {
        // Resolver collisions (one Name maps to ≥ 2 distinct handles)
        // resolve to None, mirroring the resolver's "misattribution >
        // missing attribution" policy. Defends the DM aggregation path
        // explicitly even though the resolver already enforces it.
        let followings = vec![following("alice", None), following("alice2", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Mike", "alice"), ("Mike", "alice2")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let threads = vec![dm_thread(
            &["Mike", "Me Real"],
            vec![msg(Some("Mike"), Some(1_700_000_000), vec![])],
        )];
        inputs.inbox_threads = &threads;

        let by = by_username(aggregate(&inputs, fixed_now()));
        assert_eq!(by["alice"].dm_messages_total, 0);
        assert_eq!(by["alice2"].dm_messages_total, 0);
    }

    #[test]
    fn resolved_handle_outside_followings_does_not_create_row() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Stranger", "stranger")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let threads = vec![dm_thread(
            &["Stranger", "Me Real"],
            vec![msg(Some("Stranger"), Some(1_700_000_000), vec![])],
        )];
        inputs.inbox_threads = &threads;

        let out = aggregate(&inputs, fixed_now());
        let handles: HashSet<&str> = out.iter().map(|f| f.username.as_str()).collect();
        assert_eq!(
            handles,
            HashSet::from(["alice"]),
            "resolved-but-non-followee must not anchor a phantom output row",
        );
    }

    #[test]
    fn classifies_message_direction_and_skips_missing_sender() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let threads = vec![dm_thread(
            &["Alice Real", "Me Real"],
            vec![
                msg(Some("Me Real"), Some(1_700_000_000), vec![]),
                msg(Some("Me Real"), Some(1_700_000_010), vec![]),
                msg(Some("Me Real"), Some(1_700_000_020), vec![]),
                msg(Some("Alice Real"), Some(1_700_000_030), vec![]),
                msg(Some("Alice Real"), Some(1_700_000_040), vec![]),
                // Missing sender — counts toward dm_messages_total but
                // not toward direction-balance denom (cannot classify).
                msg(None, Some(1_700_000_050), vec![]),
            ],
        )];
        inputs.inbox_threads = &threads;

        let alice = &by_username(aggregate(&inputs, fixed_now()))["alice"];
        assert_eq!(alice.dm_messages_total, 6);
        // 3 outbound, 2 inbound, 1 unclassifiable → 3 / 5 = 0.6
        assert_eq!(alice.dm_balance, Some(0.6));
    }

    #[test]
    fn dm_balance_is_none_when_no_classifiable_senders() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let threads = vec![dm_thread(
            &["Alice Real", "Me Real"],
            vec![msg(None, Some(1_700_000_000), vec![])],
        )];
        inputs.inbox_threads = &threads;

        let alice = &by_username(aggregate(&inputs, fixed_now()))["alice"];
        assert_eq!(alice.dm_messages_total, 1);
        assert_eq!(alice.dm_balance, None);
    }

    #[test]
    fn classifies_reaction_direction() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let threads = vec![dm_thread(
            &["Alice Real", "Me Real"],
            vec![msg(
                Some("Me Real"),
                Some(1_700_000_000),
                vec![
                    react(Some("Me Real")),
                    react(Some("Alice Real")),
                    react(Some("Stranger")),
                    react(None),
                ],
            )],
        )];
        inputs.inbox_threads = &threads;

        let alice = &by_username(aggregate(&inputs, fixed_now()))["alice"];
        // Me → given, Alice → received, Stranger → received (anything
        // not-me counts as received in 1:1 threads), None → skipped.
        assert_eq!(alice.dm_reactions_given, 1);
        assert_eq!(alice.dm_reactions_received, 2);
    }

    #[test]
    fn dm_recency_days_uses_max_timestamp() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        // Out-of-order timestamps: the recency value must be derived
        // from the maximum, not the last entry encountered.
        let now_secs = 1_800_000_000_i64;
        let latest_secs = now_secs - 7 * 86_400;
        let threads = vec![dm_thread(
            &["Alice Real", "Me Real"],
            vec![
                msg(Some("Me Real"), Some(latest_secs - 86_400), vec![]),
                msg(Some("Alice Real"), Some(latest_secs), vec![]),
                msg(Some("Me Real"), Some(latest_secs - 2 * 86_400), vec![]),
            ],
        )];
        inputs.inbox_threads = &threads;

        let alice = &by_username(aggregate(&inputs, fixed_now()))["alice"];
        assert_eq!(alice.dm_recency_days, Some(7));
    }

    #[test]
    fn future_message_timestamp_yields_none_recency() {
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let future_secs = 1_800_000_000_i64 + 86_400;
        let threads = vec![dm_thread(
            &["Alice Real", "Me Real"],
            vec![msg(Some("Alice Real"), Some(future_secs), vec![])],
        )];
        inputs.inbox_threads = &threads;

        let alice = &by_username(aggregate(&inputs, fixed_now()))["alice"];
        assert_eq!(alice.dm_recency_days, None);
    }

    #[test]
    fn cross_thread_balance_and_recency_finalize_correctly() {
        // Display-name aliases for the same handle exist in the real
        // export: a followee may appear under two `(Name, Username)` pairs
        // in different `label_values` files. The aggregator must treat
        // every inbox thread that resolves to that handle as part of one
        // aggregation — balance/recency reduce across threads, not
        // last-thread-wins.
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice"), ("Alice Alias", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let now_secs = 1_800_000_000_i64;
        let threads = vec![
            dm_thread(
                &["Alice Real", "Me Real"],
                vec![
                    msg(Some("Me Real"), Some(now_secs - 31 * 86_400), vec![]),
                    msg(Some("Me Real"), Some(now_secs - 30 * 86_400), vec![]),
                ],
            ),
            dm_thread(
                &["Alice Alias", "Me Real"],
                vec![
                    msg(Some("Alice Alias"), Some(now_secs - 11 * 86_400), vec![]),
                    msg(Some("Alice Alias"), Some(now_secs - 10 * 86_400), vec![]),
                ],
            ),
        ];
        inputs.inbox_threads = &threads;

        let alice = &by_username(aggregate(&inputs, fixed_now()))["alice"];
        assert_eq!(alice.dm_messages_total, 4);
        // 2 outbound + 2 inbound across the two threads — balance must
        // reflect the union, not the second thread's local 0/2 = 0.0.
        assert_eq!(alice.dm_balance, Some(0.5));
        assert_eq!(alice.dm_recency_days, Some(10));
    }

    #[test]
    fn inbound_dm_request_flips_via_message_requests_only() {
        let followings = vec![following("alice", None), following("bob", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice"), ("Bob Real", "bob")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        // Alice has only an inbox thread → inbound_dm_request stays
        // false. Bob has only a message_requests thread → flips.
        let inbox = vec![dm_thread(
            &["Alice Real", "Me Real"],
            vec![msg(Some("Alice Real"), Some(1_700_000_000), vec![])],
        )];
        let requests = vec![dm_thread(
            &["Bob Real", "Me Real"],
            vec![msg(Some("Bob Real"), Some(1_700_000_000), vec![])],
        )];
        inputs.inbox_threads = &inbox;
        inputs.message_request_threads = &requests;

        let by = by_username(aggregate(&inputs, fixed_now()));
        assert!(!by["alice"].inbound_dm_request);
        assert!(by["bob"].inbound_dm_request);
        // message_requests must NOT credit bob's message-counts or
        // reactions — that path sources from `inbox_threads` only.
        assert_eq!(by["bob"].dm_messages_total, 0);
        assert_eq!(by["bob"].dm_reactions_received, 0);
    }

    // ── slice 7B-2: decay weighting + windowed counts ────────────────────

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected ~{b}, got {a}");
    }

    #[test]
    fn decay_weight_at_zero_age_is_one() {
        let now = fixed_now();
        approx(decay_weight(Some(now), now, 365), 1.0);
    }

    #[test]
    fn decay_weight_at_tau_age_is_one_over_e() {
        let now = fixed_now();
        let tau = 365u32;
        // Exactly τ days back — the inflection point of `exp(-Δt/τ)`.
        let earlier = Timestamp::from_second(1_800_000_000 - i64::from(tau) * 86_400).unwrap();
        approx(decay_weight(Some(earlier), now, tau), (-1.0_f64).exp());
    }

    #[test]
    fn decay_weight_at_two_tau_is_one_over_e_squared() {
        let now = fixed_now();
        let tau = 180u32;
        let earlier = Timestamp::from_second(1_800_000_000 - 2 * i64::from(tau) * 86_400).unwrap();
        approx(decay_weight(Some(earlier), now, tau), (-2.0_f64).exp());
    }

    #[test]
    fn decay_weight_missing_or_future_timestamp_is_zero() {
        let now = fixed_now();
        approx(decay_weight(None, now, 365), 0.0);
        let future = Timestamp::from_second(1_800_000_000 + 86_400).unwrap();
        approx(decay_weight(Some(future), now, 365), 0.0);
    }

    #[test]
    fn within_window_boundary_is_half_open() {
        let now = fixed_now();
        // 0 days back → in
        assert!(within_window(Some(now), now, 90));
        // 89.99 days back → in
        let almost_90 = Timestamp::from_second(1_800_000_000 - 90 * 86_400 + 1).unwrap();
        assert!(within_window(Some(almost_90), now, 90));
        // Exactly 90 days back → out (half-open at the upper bound)
        let exactly_90 = Timestamp::from_second(1_800_000_000 - 90 * 86_400).unwrap();
        assert!(!within_window(Some(exactly_90), now, 90));
    }

    #[test]
    fn within_window_missing_or_future_timestamp_is_false() {
        let now = fixed_now();
        assert!(!within_window(None, now, 90));
        let future = Timestamp::from_second(1_800_000_000 + 1).unwrap();
        assert!(!within_window(Some(future), now, 90));
    }

    #[test]
    fn activity_decay_and_window_compose_end_to_end() {
        // Three liked_comments toward alice, at 0d / 90d / 365d back.
        // tau_content_days = 365, so decayed contributions are
        // exp(0) + exp(-90/365) + exp(-1) = 1.0 + 0.7821... + 0.3679...
        // The 90d window is half-open: only the 0d entry counts as in.
        let now_secs = 1_800_000_000_i64;
        let zero_days_ts = Timestamp::from_second(now_secs).unwrap();
        let ninety_days_ts = Timestamp::from_second(now_secs - 90 * 86_400).unwrap();
        let one_year_ts = Timestamp::from_second(now_secs - 365 * 86_400).unwrap();

        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let liked = vec![
            ShapeAEntry {
                username: "alice".to_owned(),
                timestamp: Some(zero_days_ts),
            },
            ShapeAEntry {
                username: "alice".to_owned(),
                timestamp: Some(ninety_days_ts),
            },
            ShapeAEntry {
                username: "alice".to_owned(),
                timestamp: Some(one_year_ts),
            },
        ];
        inputs.liked_comments = &liked;

        let by = by_username(aggregate(&inputs, fixed_now()));
        let alice = &by["alice"];
        assert_eq!(alice.likes_given, 3);
        let expected = 1.0 + (-90.0_f64 / 365.0).exp() + (-1.0_f64).exp();
        approx(alice.likes_given_decayed, expected);
        // 0d in, 90d out (half-open), 365d out → exactly 1 in the window.
        assert_eq!(alice.likes_given_90d, 1);
    }

    #[test]
    fn dm_decay_and_window_compose_end_to_end() {
        // tau_dm_days = 180. One message at 0d (in 180d window), one at
        // 180d (exactly at boundary — out of half-open window). Both
        // carry a received reaction.
        let now_secs = 1_800_000_000_i64;
        let zero_days = now_secs;
        let one_eighty = now_secs - 180 * 86_400;

        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        let threads = vec![dm_thread(
            &["Alice Real", "Me Real"],
            vec![
                msg(
                    Some("Me Real"),
                    Some(zero_days),
                    vec![react(Some("Alice Real"))],
                ),
                msg(
                    Some("Me Real"),
                    Some(one_eighty),
                    vec![react(Some("Alice Real"))],
                ),
            ],
        )];
        inputs.inbox_threads = &threads;

        let alice = &by_username(aggregate(&inputs, fixed_now()))["alice"];
        assert_eq!(alice.dm_messages_total, 2);
        approx(alice.dm_messages_total_decayed, 1.0 + (-1.0_f64).exp());
        assert_eq!(alice.dm_reactions_received, 2);
        approx(alice.dm_reactions_received_decayed, 1.0 + (-1.0_f64).exp());
        // 180d window: 0d in, 180d out (half-open) → 1
        assert_eq!(alice.dm_reactions_received_180d, 1);
    }

    // ── account-class heuristic: classifier propagates through aggregator ─

    #[test]
    fn classifier_stamps_brand_for_lexicon_hit() {
        // Pin the classifier→aggregator wire: a brand-suffixed handle in
        // `followings` must materialize as `account_class = Brand` on its
        // `AccountFeatures`. Personal-handled followees stay `Personal`.
        let followings = vec![
            following("alice", None),
            following("nytimes_official", None),
        ];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

        let by = by_username(aggregate(&inputs, fixed_now()));
        assert_eq!(by["alice"].account_class, AccountClass::Personal);
        assert_eq!(by["nytimes_official"].account_class, AccountClass::Brand);
    }

    #[test]
    fn classifier_stamps_keeplisted_independently_of_class() {
        // The keeplist is a parallel signal — a personal-handled close
        // friend on the keeplist stays `Personal` but flips
        // `is_keeplisted = true`. Scoring will gate Unfollow on
        // BOTH, but classification stays honest.
        let mut keeplist = HashSet::new();
        keeplist.insert("alice".to_owned());
        let classifier = Classifier::new(keeplist, HashSet::new());

        let followings = vec![following("alice", None), following("bob", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

        let by = by_username(aggregate(&inputs, fixed_now()));
        assert!(by["alice"].is_keeplisted);
        assert_eq!(by["alice"].account_class, AccountClass::Personal);
        assert!(!by["bob"].is_keeplisted);
    }

    #[test]
    fn classifier_stamps_droplisted_and_ignores_non_followees() {
        // Mirror of the keeplist case for the inverse signal. A droplisted
        // followee surfaces `is_droplisted = true`; a non-followee handle
        // in the droplist creates no output row (acceptance criterion 8).
        let mut drop = HashSet::new();
        drop.insert("alice".to_owned());
        drop.insert("ghost_not_followed".to_owned());
        let classifier = Classifier::new(HashSet::new(), drop);

        let followings = vec![following("alice", None), following("bob", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

        let out = aggregate(&inputs, fixed_now());
        let by = by_username(out);
        assert!(by["alice"].is_droplisted);
        assert_eq!(by["alice"].account_class, AccountClass::Personal);
        assert!(!by["bob"].is_droplisted);
        assert!(
            !by.contains_key("ghost_not_followed"),
            "a droplist handle that isn't a followee must not anchor a row",
        );
    }

    // ── decay-weighted + windowed accumulation ────────────────────────────
    //
    // Every other aggregate test feeds `timestamp: None`, which makes
    // `decay_weight` return 0.0 and `within_window` false — so the `*_decayed`
    // sums and the `*_90d` / `*_180d` counters (the values scoring actually
    // consumes) are never exercised. These helpers + tests feed real recent
    // timestamps and pin both.

    fn owner_entry_ts(handle: &str, ts: i64) -> ShapeCEntry {
        let mut e = owner_entry(handle);
        e.timestamp = Some(ts);
        e
    }

    fn shape_a_ts(username: &str, ts: i64) -> ShapeAEntry {
        ShapeAEntry {
            username: username.to_owned(),
            timestamp: Timestamp::from_second(ts).ok(),
        }
    }

    fn comment_ts(target: &str, ts: i64) -> CommentEntry {
        CommentEntry {
            target_username: target.to_owned(),
            timestamp: Timestamp::from_second(ts).ok(),
        }
    }

    #[test]
    fn content_decay_and_window_accumulate_with_recent_timestamps() {
        let now = fixed_now();
        let ten_days_ago = now.as_second() - 10 * 86_400; // inside the 90d window
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

        let liked_posts = vec![owner_entry_ts("alice", ten_days_ago)];
        let liked_comments = vec![shape_a_ts("alice", ten_days_ago)];
        let post_comments = vec![comment_ts("alice", ten_days_ago)];
        let polls = vec![shape_a_ts("alice", ten_days_ago)];
        let story_likes = vec![owner_entry_ts("alice", ten_days_ago)];
        let stories_viewed = vec![owner_entry_ts("alice", ten_days_ago)];
        let saved_posts = vec![owner_entry_ts("alice", ten_days_ago)];
        inputs.liked_posts = &liked_posts;
        inputs.liked_comments = &liked_comments;
        inputs.post_comments = &post_comments;
        inputs.story_polls = &polls;
        inputs.story_likes = &story_likes;
        inputs.stories_viewed = &stories_viewed;
        inputs.saved_posts = &saved_posts;

        let alice = &by_username(aggregate(&inputs, now))["alice"];

        // Exact decayed values, not just `> 0`: each accumulator fed by two
        // sources (likes = posts + comments; story_out = polls + story_likes)
        // would mask a `+=`→`*=` mutation on ONE source behind the other's
        // contribution under a loose `> 0` check. `w` is the weight of a
        // single 10-day-old signal under τ_content = 365d.
        let w = (-10.0_f64 / 365.0).exp();
        let approx = |got: f64, want: f64, label: &str| {
            assert!((got - want).abs() < 1e-9, "{label}: got {got}, want {want}");
        };
        approx(
            alice.likes_given_decayed,
            2.0 * w,
            "likes decayed (post + comment)",
        );
        approx(alice.comments_given_decayed, w, "comments decayed");
        approx(
            alice.story_interactions_out_decayed,
            2.0 * w,
            "story_out decayed (shape-A poll + shape-C story_like)",
        );
        approx(alice.stories_viewed_decayed, w, "stories_viewed decayed");
        approx(alice.saved_their_content_decayed, w, "saved decayed");

        // Windowed counts: likes = post(1) + comment(1) = 2; comments = 1.
        assert_eq!(alice.likes_given_90d, 2, "likes_given_90d");
        assert_eq!(alice.comments_given_90d, 1, "comments_given_90d");
    }

    #[test]
    fn dm_reaction_decay_and_180d_window_accumulate() {
        let now = fixed_now();
        let recent = now.as_second() - 5 * 86_400; // inside the 180d window
        let followings = vec![following("alice", None)];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = resolver_from(&[("Alice Real", "alice")]);
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

        let threads = vec![dm_thread(
            &["Alice Real", "Me Real"],
            vec![msg(
                Some("Me Real"),
                Some(recent),
                vec![react(Some("Me Real")), react(Some("Alice Real"))],
            )],
        )];
        inputs.inbox_threads = &threads;

        let alice = &by_username(aggregate(&inputs, now))["alice"];
        assert!(
            alice.dm_reactions_given_decayed > 0.0,
            "given reaction decayed must be > 0",
        );
        assert!(
            alice.dm_reactions_received_decayed > 0.0,
            "received reaction decayed must be > 0",
        );
        assert_eq!(alice.dm_reactions_given_180d, 1, "given within 180d");
        assert_eq!(alice.dm_reactions_received_180d, 1, "received within 180d");
    }

    #[test]
    fn followed_exactly_now_yields_zero_tenure_not_none() {
        // Boundary: a follow timestamp equal to `now` is 0 days, not a
        // future-skew `None`. Pins `days_since`'s `secs < 0` guard as a
        // strict `<` (a `<=`/`==` mutation would map exactly-now to None).
        let now = fixed_now();
        let followings = vec![following("alice", Some(now))];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

        let by = by_username(aggregate(&inputs, now));
        assert_eq!(by["alice"].follow_tenure_days, Some(0));
    }

    // ── mutual-follow duration (reciprocal age) ───────────────────────────

    fn follower(username: &str, followed_me_at: Option<Timestamp>) -> FollowerEntry {
        FollowerEntry {
            username: username.to_owned(),
            followed_me_at,
        }
    }

    #[test]
    fn mutual_age_days_from_later_follow() {
        // You followed alice 1000 days ago; she followed back 800 days ago.
        // The relationship has been mutual since the LATER follow → 800 days.
        let now = fixed_now();
        let you = Timestamp::from_second(now.as_second() - 1000 * 86_400).unwrap();
        let them = Timestamp::from_second(now.as_second() - 800 * 86_400).unwrap();
        let followings = vec![following("alice", Some(you))];
        let followers = vec![follower("alice", Some(them))];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        inputs.followers = &followers;

        let alice = &by_username(aggregate(&inputs, now))["alice"];
        assert!(alice.is_mutual);
        assert_eq!(alice.mutual_age_days, Some(800));
    }

    #[test]
    fn mutual_age_days_uses_later_of_the_two_follows() {
        // They followed you long ago (1000d); you followed back recently
        // (300d). Mutual only since YOUR follow → 300 days. Pins `max`, not
        // `min` — a `min` would wrongly report the relationship as 1000d old.
        let now = fixed_now();
        let them = Timestamp::from_second(now.as_second() - 1000 * 86_400).unwrap();
        let you = Timestamp::from_second(now.as_second() - 300 * 86_400).unwrap();
        let followings = vec![following("alice", Some(you))];
        let followers = vec![follower("alice", Some(them))];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        inputs.followers = &followers;

        let alice = &by_username(aggregate(&inputs, now))["alice"];
        assert_eq!(alice.mutual_age_days, Some(300));
    }

    #[test]
    fn mutual_age_days_none_when_not_mutual() {
        // In followings but not followers → not mutual → no reciprocal age.
        let now = fixed_now();
        let you = Timestamp::from_second(now.as_second() - 1000 * 86_400).unwrap();
        let followings = vec![following("alice", Some(you))];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);

        let alice = &by_username(aggregate(&inputs, now))["alice"];
        assert!(!alice.is_mutual);
        assert_eq!(alice.mutual_age_days, None);
    }

    #[test]
    fn mutual_age_days_none_when_a_follow_timestamp_missing() {
        // Mutual, but your follow lacks a timestamp → relationship can't be
        // dated → None (conservative: the deep-mutual floor won't fire on an
        // undatable relationship).
        let now = fixed_now();
        let them = Timestamp::from_second(now.as_second() - 800 * 86_400).unwrap();
        let followings = vec![following("alice", None)];
        let followers = vec![follower("alice", Some(them))];
        let hide = empty_hide_entry();
        let me = synth_me();
        let resolver = NameResolver::default();
        let decay = synth_decay();
        let classifier = synth_classifier();
        let mut inputs = empty_inputs(&followings, &hide, &me, &resolver, &classifier, &decay);
        inputs.followers = &followers;

        let alice = &by_username(aggregate(&inputs, now))["alice"];
        assert!(alice.is_mutual);
        assert_eq!(alice.mutual_age_days, None);
    }
}
