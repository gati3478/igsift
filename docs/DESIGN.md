# Design — `ig-mgr`

The full design for the Instagram following-cleanup CLI. Status, build, and the
short pitch live in the [README](../README.md); the task list in
[ROADMAP.md](../ROADMAP.md). Parser layer, feature aggregation, first-pass
scoring, CSV/Markdown writers, and the brand/public-figure account-class
heuristic (with user-maintained keep-allowlist override) are all landed today;
weight tuning against a labeled set and the run-on-real-export feedback loop
are the remaining ROADMAP items.

## Inputs

> **Schema validated 2026-05-26** against the 2026-05-11 personal export by
> walking every JSON file with [`scripts/walk_export_schema.sh`](../scripts/walk_export_schema.sh).
> The paths, shapes, and field names below match what Instagram actually
> ships today. Re-run the walker after every new export to detect drift.

The full Instagram "Download Your Information" export in **JSON** format,
unzipped and merged into one root. Instagram chunks large exports by ~2 GB
file-size budget across multiple zips; the same DM thread folder can appear
in multiple chunks with disjoint files inside (JSON metadata in one chunk,
media in others). Merge with `rsync -a chunk/ merged/` for each chunk —
files are unioned without conflict.

### Files we consume

| Path                                                                                                                                                        | Shape group                            | Used for                                                        |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | --------------------------------------------------------------- |
| `connections/followers_and_following/following.json`                                                                                                        | **A** (wrapped + flat)                 | the set we score, **plus** per-account follow-tenure timestamp  |
| `connections/followers_and_following/followers_*.json` †                                                                                                    | **B** (bare array + flat)              | follower set (mutual-follow detection)                          |
| `connections/followers_and_following/close_friends.json`                                                                                                    | **C** (`label_values`)                 | hard `keep` boost                                               |
| `connections/followers_and_following/profiles_you've_favorited.json`                                                                                        | **C**                                  | hard `keep` boost (distinct tier from close_friends)            |
| `connections/followers_and_following/blocked_profiles.json`                                                                                                 | **C**                                  | hard exclude from set                                           |
| `connections/followers_and_following/restricted_profiles.json`                                                                                              | **C**                                  | floor bucket to `review` minimum                                |
| `connections/followers_and_following/hide_story_from.json`                                                                                                  | object, single-entry                   | weak negative                                                   |
| `connections/followers_and_following/recently_unfollowed_profiles.json`                                                                                     | **C**                                  | exclude from set (already dropped)                              |
| `connections/followers_and_following/removed_suggestions.json`                                                                                              | **C**                                  | very weak negative (PYMK dismissals)                            |
| `your_instagram_activity/messages/inbox/<thread>/message_*.json` ‡                                                                                          | thread-specific (see below)            | DM volume, recency, direction, **reactions in both directions** |
| `your_instagram_activity/messages/message_requests/<thread>/...`                                                                                            | same as inbox                          | weak inbound interest (DM attempts from non-followings)         |
| `your_instagram_activity/likes/liked_posts.json`                                                                                                            | **C** with nested `Owner.dict[0].dict` | likes I gave (target = Owner.Username)                          |
| `your_instagram_activity/likes/liked_comments.json`                                                                                                         | **A**                                  | comment-likes I gave (target = `title`)                         |
| `your_instagram_activity/comments/post_comments_1.json` §                                                                                                   | **D** (`string_map_data`)              | comments I left on posts                                        |
| `your_instagram_activity/comments/reels_comments.json`                                                                                                      | **A** wrapping **D**-shaped entries    | comments I left on reels                                        |
| `your_instagram_activity/comments/hype.json`                                                                                                                | **A** wrapping **D**-shaped entries    | comments I left on stories                                      |
| `your_instagram_activity/story_interactions/{polls,quizzes,questions,emoji_sliders,emoji_story_reactions,story_reaction_sticker_reactions,countdowns}.json` | **A**                                  | outbound story engagement (target = `title`)                    |
| `your_instagram_activity/story_interactions/{story_likes,stories_viewed}.json`                                                                              | **C** with nested `Owner`              | outbound story likes / passive view tracking                    |
| `your_instagram_activity/saved/saved_posts.json`                                                                                                            | **C** with nested `Owner`              | saves of someone's content                                      |

† Followers are numbered (`followers_1.json`, `_2`, `_3` for larger accounts) — glob.
‡ DM thread schema: `{messages:[{sender_name, timestamp_ms, content?, reactions:[{reaction, actor}], photos?, videos?, share?, ...}], participants:[{name}], thread_path, title, ...}`.
§ Post comments are numbered (`post_comments_1.json`, `_2`, …) for high-volume accounts — glob.

### Files we deliberately skip

- `messages/broadcast/<channel>/*` — pub/sub channels (other user publishes, I subscribe); not a 1:1 relationship.
- `messages/ai_conversations.json` — chats with the AI assistant.
- `messages/secret_conversations.json` — E2EE chats; only `armadillo_devices` / `calls` metadata, no content.
- `your_instagram_activity/threads/*` — the _Threads_ social network app, not IG.
- `your_instagram_activity/{events,monetization,other_activity,shopping,subscriptions}/` — no per-account signal.
- `personal_information/personal_information/note_and_repost_interactions.json` — ships exactly one entry with no content, no timestamp, ambiguous direction. Too sparse to be useful.
- `connections/contacts/synced_contacts.json` — phone contacts (244 entries). Possible v2 "IRL overlap" join; not in v1.
- `media/posts/<YYYYMM>/*.jpg` — my own uploaded media. No metadata JSON ships alongside — so no caption, no tagged users.

### Signals the export does NOT ship (dropped from spec)

Schema validation killed four features that earlier drafts of this doc assumed
existed. Their source files simply aren't in the personal export:

- **`searched_for_them`** — no `profile_searches.json` exists anywhere.
- **`tagged_them`** — no post/reel/story metadata; `media/posts/` has only raw JPGs.
- **`they_tagged_me`** — no `archived_posts/` either.
- **`story_interactions_in`** — every `story_interactions/*.json` file has me as
  the actor; `title` and nested `Owner` are always the story owner (them).

Inbound follow requests are also absent: there is **no** `follow_requests_you've_received.json`. Instagram exports `pending_follow_requests.json` and `recent_follow_requests.json` but both are **outbound** (mine, awaiting target approval).

### Partial observability — narrower than it looks

Instagram doesn't ship who liked _my_ posts, who commented on _my_ posts, or
who reacted to _my_ stories. Reciprocity has to be inferred from indirect
evidence — but it's not zero. The export **does** ship one true inbound
channel:

**DM `reactions[].actor`**. Each message in `messages/inbox/<thread>/message_*.json`
carries a `reactions` array; when `actor != me`, that record is _them reacting
to one of my messages_. Both directions are visible. This is the single most
valuable bidirectional signal in the export and underpins `dm_reactions_received`.

`messages/message_requests/` (10 threads in this export) is also inbound — DM
attempts from accounts I never accepted. Weak signal but real.

### DM `display_name ↔ handle` bridge — 37% coverage by design

DM threads ship `participants[].name` and `messages[].sender_name` as **display
names**, never handles, while every other source keys by handle. The
aggregator joins them via the seven `label_values` files (`close_friends`,
`profiles_you've_favorited`, `blocked_profiles`, `restricted_profiles`,
`recently_unfollowed_profiles`, `removed_suggestions`, `hide_story_from`) —
each entry carries both a `Name` and a `Username` label at the outer
`label_values` level. Recon on the 2026-05-11 export:

- **281 unique `(Name, Username)` pairs** across the seven files.
- **12 name collisions** — `"Mike"` → {`bermudalckt`, `hairycub81`,
  `leahcim333`} and similar. The resolver returns `None` on collision
  rather than guessing; misattribution is worse than missing attribution.
- **217/581 (37%) of 1:1 DM threads** resolve to a followee handle under
  this strict policy. The remaining 63% of followings score from
  activity-side features (handle-keyed at source) only — `dm_*` features
  are sparse by design, not by bug.

The DM thread folder name `<inbox>/<thread>/` looks like a bridge but
isn't: validation showed the prefix is the participant's display name
sanitized (lowercased, spaces stripped), not the handle. Only 3 of 30
sampled followings appeared as folder prefixes — those were coincidences
where display name happens to equal handle. Don't read the folder.

Group chats (≥ 3 participants — 9 threads in this export) and abandoned
threads (only `me` as participant — 3 threads) are dropped from `dm_*`
aggregation: per-participant attribution in groups doesn't fit the 1:1
feature model and group counts are weak signal in v1.

`me.name` (`"Gati Petriashvili"`) comes from
`personal_information/personal_information/personal_information.json`
→ `profile_user[0].string_map_data["Name"].value`. Missing or empty here
is a HARD ERROR — every DM-direction classification depends on it.

### Parsing notes

These bit the validation pass and will bite the parser too — call them out
explicitly so `src/export.rs` is designed for them:

- **Four distinct JSON entry shapes** across activity files. Per-file
  deserializer, not a single generic struct:
    - **A** — wrapped + flat: `{wrapper_key: [{title, string_list_data:[{href, timestamp}]}]}`
    - **B** — bare array + flat: `[{title, media_list_data, string_list_data:[{href, value, timestamp}]}]`
    - **C** — bare array + `label_values` with optional nested `dict.dict` (used for `Owner`): `[{fbid, timestamp, label_values:[{label, value, …}], media:[]}]`
    - **D** — `string_map_data` dict: `{string_map_data: {<FieldName>: {value, timestamp, href}}}`
- **Wrapper keys are inconsistent** — codified file-by-file as a private
  struct in `src/export.rs`. The full known set (validated 2026-05-27):

    | File                                                       | Wrapper key                                   |
    | ---------------------------------------------------------- | --------------------------------------------- |
    | `following.json`                                           | `relationships_following`                     |
    | `likes/liked_comments.json`                                | `likes_comment_likes`                         |
    | `comments/reels_comments.json`                             | `comments_reels_comments` †                   |
    | `comments/hype.json`                                       | `comments_story_comments` †                   |
    | `story_interactions/polls.json`                            | `story_activities_polls`                      |
    | `story_interactions/quizzes.json`                          | `story_activities_quizzes`                    |
    | `story_interactions/questions.json`                        | `story_activities_questions`                  |
    | `story_interactions/emoji_sliders.json`                    | `story_activities_emoji_sliders`              |
    | `story_interactions/emoji_story_reactions.json`            | `story_activities_emoji_quick_reactions` ‡    |
    | `story_interactions/story_reaction_sticker_reactions.json` | `story_activities_reaction_sticker_reactions` |
    | `story_interactions/countdowns.json`                       | `story_activities_countdowns`                 |

    † Shape D entries wrapped in shape A — parsed in a separate slice.
    ‡ File name and wrapper key are NOT symmetric. The file says
    `emoji_story_reactions` but the wrapper says `emoji_quick_reactions`
    — keep the constant explicit so this asymmetry does not surprise
    future maintenance.

- **`Owner` extraction** is three levels deep on shape **C**:
  `label_values[?.title == "Owner"].dict[0].dict[?.label == "Username"].value`.
- **Timestamps are inconsistent**: most files use Unix **seconds** in
  `timestamp`; DM messages use **milliseconds** in `timestamp_ms`; a few use
  `timestamp_value`. Normalize at the parse boundary into `jiff::Timestamp`.
- **Multi-part DM threads**: 14 of 593 threads span 2–10 `message_*.json` files
  (e.g., `validoli` has 10 parts). Concatenate all parts per thread before
  computing features, or the largest conversations silently truncate.

## Algorithm sketch

Score each followed account on `keep_probability ∈ [0, 1]`. **Scope:** the
ranking covers accounts **you follow** (643 in the 2026-05-11 export).
"Remove from followers" is the manual companion action — when you unfollow a
mutual you also drop them as a follower — not a separate ranked set. One list,
one decision per account.

### Per-account features

| Feature                  | Source                                              | Direction / handling                                           | Weight (initial)                        |
| ------------------------ | --------------------------------------------------- | -------------------------------------------------------------- | --------------------------------------- |
| `dm_messages_total`      | inbox `<thread>` messages                           | outbound + inbound, log-scaled                                 | high                                    |
| `dm_recency_days`        | last `timestamp_ms` in thread                       | recency enters via decayed counts; field surfaces in CSV only  | (no separate weight)                    |
| `dm_balance`             | outbound / (outbound + inbound) message count       | penalize one-sided threads                                     | medium                                  |
| `dm_reactions_given`     | `reactions[?actor == me]`                           | log-scaled, recency-weighted                                   | medium                                  |
| `dm_reactions_received`  | `reactions[?actor != me]` — **inbound** reciprocity | log-scaled, recency-weighted                                   | medium-high                             |
| `dm_reaction_balance`    | given / (given + received)                          | penalize one-sided reactions                                   | low-medium                              |
| `inbound_dm_request`     | thread present in `message_requests/`               | boolean                                                        | low keep-bias                           |
| `likes_given`            | `liked_posts` + `liked_comments`                    | log-scaled, recency-weighted                                   | medium                                  |
| `comments_given`         | `post_comments_*` + `reels_comments` + `hype`       | log-scaled, recency-weighted                                   | medium                                  |
| `story_interactions_out` | all `story_interactions/*` aggregated               | log-scaled, recency-weighted                                   | medium                                  |
| `stories_viewed`         | `stories_viewed.json`                               | log-scaled, recency-weighted                                   | low                                     |
| `saved_their_content`    | `saved_posts.json`                                  | log-scaled                                                     | low                                     |
| `follow_tenure_days`     | `following.json` per-account `timestamp`            | `log(days_since_follow + 1)`                                   | low                                     |
| `is_close_friend`        | `close_friends.json`                                | boolean                                                        | hard boost                              |
| `is_favorited`           | `profiles_you've_favorited.json`                    | boolean                                                        | hard boost (separate from close_friend) |
| `is_blocked`             | `blocked_profiles.json`                             | boolean                                                        | **excludes from input set**             |
| `is_restricted`          | `restricted_profiles.json`                          | boolean                                                        | floor bucket to `review`                |
| `is_hide_story_from`     | `hide_story_from.json`                              | boolean                                                        | weak negative                           |
| `is_removed_suggestion`  | `removed_suggestions.json`                          | boolean                                                        | very weak negative                      |
| `recently_unfollowed`    | `recently_unfollowed_profiles.json`                 | boolean                                                        | **excludes from input set**             |
| `account_class`          | username/name heuristic (below)                     | personal / brand (PublicFigure deferred — see heuristic below) | gates the `unfollow` recommendation     |
| `is_keep_allowlisted`    | `config/keep_allowlist.txt`                         | boolean                                                        | parallel Unfollow→Review override       |

**Decay.** Every interaction count is recency-weighted with exponential decay
so a 2019 like is worth far less than a 2026 like. τ is configurable; start
≈ 180 days for DMs, 365 for content interactions.

### Account-class heuristic (gates "suggest unfollow")

Lives in [`src/features/account_class.rs`](../src/features/account_class.rs);
`Classifier` is built once per run from the user-maintained allowlist and
threaded into the aggregator via `AggregateInputs`.

- **Lexicon match.** Username AND display name are case-insensitively
  substring-matched against `BRAND_LEXICON` — a curated 8-token list
  (`official`, `studio`, `magazine`, `records`, `gallery`, `news`, `media`,
  `agency`) using **`aho-corasick`** (single automaton, one pass over each
  input). Tokens shorter than 5 chars (`inc`, `co`) are deliberately omitted
  because they false-positive on personal handles like `incognito_jay` and
  `cooking_anna`, and false positives are costlier here than false negatives —
  a missed brand stays Personal and remains eligible for the close_friend /
  favorited / allowlist gates, whereas a falsely-flagged brand silently
  suppresses a real Unfollow recommendation.
- **`AccountClass` variants.** `Personal` (default) and `Brand` (lexicon hit
  on either surface). `PublicFigure` is **deliberately omitted** from the
  variant set: the text-only heuristic can't reliably distinguish brand from
  public_figure, and the downstream gating is identical (block Unfollow,
  surface as Review). Adding a variant the aggregator can't populate would
  be a lie about what it knows; if a future labeled set proves the distinction
  matters, add the variant then.
- **Keep-allowlist override.** [`config/keep_allowlist.txt`](../config/keep_allowlist.txt)
  is a separate user-maintained list of handles that must never bucket as
  Unfollow — brands the lexicon misses, public figures, and personal accounts
  the export under-represents (sparse signal, out-of-band relationship). The
  allowlist is **NOT** classification: a personal close-friend the user
  allowlists stays `Personal` at the `AccountClass` level so the CSV column
  doesn't misrepresent the profile. The override surfaces as a separate
  `is_keep_allowlisted: bool` on `AccountFeatures`, parallel to
  `is_close_friend` / `is_favorited`. Scoring's `assign_bucket` folds both
  signals into the Unfollow gate: `account_class != Personal ||
is_keep_allowlisted` downgrades to Review.
- **Honest uncertainty.** Follower count cannot be inferred from the export,
  so the heuristic is limited to name patterns + allowlist. The text-only
  surface is inherently lossy. **If uncertain, never auto-suggest unfollow —
  the gate fires conservatively (Brand OR allowlisted → Review).**

### Scoring composition (initial form, iterate empirically)

```
engagement     = w_dm·dm_messages + w_likes·likes_given + w_comments·comments_given
               + w_story_out·story_interactions_out + w_stories_viewed·stories_viewed
               + w_saved·saves_of_their_content + w_reactions_given·dm_reactions_given
reciprocity    = w_reactions_received·dm_reactions_received
tenure         = w_tenure·log(follow_tenure_days + 1)
boosts         = (is_close_friend ? w_close_friend_boost : 0)
               + (is_favorited   ? w_favorite_boost     : 0)
               + (inbound_dm_request ? w_inbound_request : 0)
penalties      = w_dm_balance_penalty·dm_balance_penalty
               + w_reaction_balance_penalty·reaction_balance_penalty
               + (is_hide_story_from   ? w_hide_story_penalty   : 0)
               + (is_removed_suggestion ? w_removed_suggestion_penalty : 0)
score_raw      = engagement + reciprocity + tenure + boosts - penalties
keep_prob      = sigmoid((score_raw - threshold) / scale)
```

Every key in [`config/scoring.toml`](../config/scoring.toml) `[weights]` appears
exactly once above. Recency enters through the exponential decay applied to
each count (`[decay]` τ constants), not as an additive term — `dm_recency_days`
is materialized on `AccountFeatures` for the CSV (`last_dm_days`) but never
fed into `score_raw`. Weights and constants live in TOML so they can be
tuned without a rebuild.

#### Balance-penalty form (volume-gated, asymmetric)

`dm_balance_penalty` and `reaction_balance_penalty` are not stored on
`AccountFeatures` — they're derived in `src/scoring.rs` from the raw
`dm_balance` ratio (and from the raw `dm_reactions_given` /
`dm_reactions_received` counts for the reaction variant) so the volume-
gating policy lives next to the weight rather than baked into the
aggregator:

```
volume_gate(count, k)  = (count >= k)         # k = 5 today, tunable later
balance_penalty(ratio) = max(0, ratio - 0.5) * 2   ∈ [0, 1]
```

Asymmetric on purpose: `balance = 1.0` (fully one-sided me) returns `1.0`,
the full penalty; `balance = 0.0` (fully one-sided them) returns `0.0`.
One-sided-them is reciprocity in the inbound direction, not over-extension
— `reactions_received` scores it through its own weight. The volume gate
prevents 1-message threads with `balance = 1.0` from dominating: a fresh
thread isn't a relationship signal yet.

#### Dominant feature label (Markdown summary)

The Markdown summary's "dominant feature" column is the term with the
largest **signed** contribution to `score_raw`: positive engagement terms
compete in their natural sign, while penalty terms enter the comparison
as **negative** of their `weight * value` product. This surfaces
"hide_story_penalty" or "dm_balance_penalty" by name when a penalty
dominates, rather than burying the negative driver under a smaller
positive term. Labels match the corresponding `WeightsConfig` field
(and `[weights]` TOML key) verbatim so a call-out traces back to one
line of code and one line of config.

### Buckets

- `keep` (`keep_prob ≥ 0.7`) — solid two-way relationship or boosted account.
- `review` (`0.3 ≤ keep_prob < 0.7`) — ambiguous; needs my eyes.
- `unfollow` (`keep_prob < 0.3` **and** `account_class == personal` **and** not
  `is_close_friend`/`is_favorited`/`is_restricted`) — confident recommendation.

`is_blocked` and `recently_unfollowed` filter the input set **entirely** — they
never appear in output. `is_restricted` floors the bucket at `review`, even if
`keep_prob` is below `unfollow_max`. Public figures / brands with low
`keep_prob` get `review`, never `unfollow` — that decision criterion ("do I
still care about their content?") is different and out of scope for v1.

## Output

**Primary: CSV** — sortable, filterable, easy to diff between runs.

```
username,display_name,bucket,keep_prob,dm_msgs,last_dm_days,reactions_given_180d,reactions_received_180d,likes_given_90d,comments_given_90d,follow_tenure_days,account_class,notes
```

> **Two aggregations — don't conflate them.** `keep_prob` is computed from
> exponential-decay-weighted signals (continuous τ). The `*_90d` / `*_180d`
> columns are **raw counts in a fixed window**, emitted only as human-readable
> sanity context for skim-review — they are _not_ the values that feed scoring.
> `features.rs` computes both: the decay-weighted score inputs and these
> plain windowed counts.

> **`display_name` is the inverse of the `NameResolver` join.** The seven
> `label_values` files ship `(Name, Username)` pairs — `NameResolver`
> already uses them to map display name → handle for DM attribution; the
> CSV writer needs the reverse direction (handle → display name), with
> the same collision policy: collisions emit empty string, never a
> guess. The CSV slice should either materialize `display_name:
Option<String>` onto `AccountFeatures` from those same pairs (single
> source of truth, single pass) or extend `NameResolver` with a
> `display_name_for(handle)` accessor.

**Secondary: Markdown summary** alongside the CSV — top 20 unfollow candidates
and top 20 keepers, with the dominant feature behind each call, for skim-review
before opening the CSV.

Filenames: `recommendations_YYYY-MM-DD.csv` + `.md`, written next to the export
folder by default, overridable via `--out`.

## Implementation notes (tech choices)

**Format: CLI, not TUI.** One-shot batch job; the CSV + Markdown output is the
interface. A `ratatui` review subcommand (`ig-mgr review`) is an optional v2
direction — out of scope for v1.

**Language: Rust** (edition 2024, stable). The workload is its sweet spot: file
I/O + JSON deserialization + numeric scoring, no async, no networking. Same
shape as `ripgrep` / `fd`; ships a single static binary.

### Core crates

| Concern        | Pick                                           | Why over the obvious alternative                                                        |
| -------------- | ---------------------------------------------- | --------------------------------------------------------------------------------------- |
| CLI parsing    | `clap` v4 (derive)                             | Universal standard; `bpaf` more elegant but smaller ecosystem.                          |
| JSON parsing   | `serde` + `serde_json` + `serde_path_to_error` | Schema-drift survival: `#[serde(default)]` + `Option<T>` + named path on parse failure. |
| Config         | `toml` (`config/scoring.toml`)                 | Weights/τ tunable without rebuilds — neutralizes Rust's data-tuning ergonomic gap.      |
| Date/time      | `jiff`                                         | Correct-by-default vs. `chrono`'s timezone footguns.                                    |
| CSV output     | `csv`                                          | Serializes directly from `#[derive(Serialize)]` structs.                                |
| Errors         | `anyhow` + `thiserror`                         | `anyhow` in `main`/orchestration; `thiserror` enums in parser modules.                  |
| Parallelism    | `rayon`                                        | Scoring is embarrassingly parallel — `par_iter()`, no async needed.                     |
| Logging        | `tracing` + `tracing-subscriber`               | Structured logs; `--verbose` wiring trivial.                                            |
| Progress UX    | `indicatif` + `owo-colors`                     | Parse-phase progress bar + colored summary table.                                       |
| Snapshot tests | `insta` (`json` + `redactions`)                | Commit a fixture export; parser changes become reviewable diffs; drift fails loudly.    |
| E2E tests      | `assert_cmd` + `predicates`                    | Run the binary against fixtures, assert on stdout + emitted CSV.                        |
| Test runner    | `cargo-nextest`                                | Faster + better output than `cargo test`.                                               |

### Schema-drift survival

`serde_path_to_error` wraps every `from_reader` so a missing/renamed key fails
with the exact JSON path. The four shape groups (A/B/C/D in "Parsing notes")
get distinct deserializer types; `#[serde(default)]` + `Option<T>` cover
optional fields. Every release of Instagram's export schema is re-validated by
running [`scripts/walk_export_schema.sh`](../scripts/walk_export_schema.sh)
against the fresh dump — diff its output against the last-known-good snapshot.

### Cutting-edge calls (flagged deliberately)

- **`jiff` over `chrono`** — newer, smaller community, better foundation. Worth
  the bet for a personal project.
- **`tracing` over `log` + `env_logger`** — slight ceremony tax, modern direction.
- **`miette`** considered for fancy parse-error diagnostics with source spans;
  defer unless parse errors get noisy.
- **Skipped for now:** `simd-json` / `sonic-rs` — 2–5× faster JSON, premature
  until a real bottleneck is measured. Drop-in upgrade path stays open.

### Deliberately not using

- **No `tokio` / async** — no concurrent I/O; async-coloring would be dead weight.
- **No DB layer** (`sqlx` / `diesel`) — the export is the source of truth.
- **No `reqwest`** — no network surface.
- **No workspace split** — single package (lib + bin) until the parser earns
  its own crate.
- **No `pyo3` / Python FFI** — pure Rust; Python notebooks for _post-hoc_ CSV
  analysis if needed, no language coupling in the codebase.

### Release profile

```toml
[profile.release]
lto = "thin"        # 2026 sweet spot — `fat` compiles much slower for marginal gain
codegen-units = 1
strip = true
```

### Fallback (not chosen)

**Python** with `pydantic` + `pandas` is the escape hatch _only_ if
scoring-weight iteration becomes the bottleneck and the TOML-config approach
above doesn't relieve it. Faster to ship, slower to align with the Rust goal.

## Open questions

- **`stories_viewed` weight** — 2,247 entries in the validated export is high,
  but viewing a story is a low-intent action (you scroll through everyone's).
  Pure noise, or a real "I'm interested in their content" signal? Worth A/B
  with `w_stories_viewed = 0` vs. `> 0`.
- **DM reactions weighting** — same scale as message volume, or distinct? A
  thread with 100 messages and 0 reactions vs. 10 messages and 10 reactions
  represents different relationship shapes; the model should reflect that.
- **`recently_unfollowed` as input exclusion vs. negative feature** — currently
  set to exclude entirely. Useful (never re-suggest someone I already dropped)
  or too churny? Empirically only 5 entries in this export — likely a non-issue.
- **`synced_contacts.json` join (v2)** — match 244 phone contacts against the
  follower set to identify IRL connections (strong implicit `keep`). Defer
  unless v1 misclassifies real friends.
- **Threshold tuning** — calibrate against personal judgment on a small labeled
  sample (~30 accounts I already know I want to keep/drop), or just iterate on
  the top/bottom of the ranking?
