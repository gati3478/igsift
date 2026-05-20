# Design — `ig-mgr`

The full design for the Instagram following-cleanup CLI. Status, build, and the
short pitch live in the [README](../README.md); the task list in
[ROADMAP.md](../ROADMAP.md). Nothing here is implemented yet.

## Inputs

> ⚠️ **Schema reference is stale.** The layout below is from a personal data
> export pulled a few months ago. Instagram silently rotates the export schema
> (paths, filenames, JSON keys). **Before any implementation work, download a
> fresh export and re-verify the structure** — every parser path here is
> suspect until confirmed against current data.

The full Instagram "Download Your Information" export in **JSON** format.
Relevant subsets the scoring should consume — not just `followers` / `following`:

- `connections/followers_and_following/` — followers, following, close friends,
  pending requests, recent follow requests, recently unfollowed.
- `your_instagram_activity/messages/inbox/<thread>/message_*.json` — DM threads
  (volume, recency, who initiated, message count by direction).
- `your_instagram_activity/likes/liked_posts.json` and `liked_comments.json` —
  likes I gave.
- `your_instagram_activity/comments/post_comments_*.json` and
  `reels_comments.json` — comments I made.
- `your_instagram_activity/story_interactions/` — story likes, polls, quizzes,
  reactions, replies, question responses.
- `your_instagram_activity/saved/saved_posts.json` — accounts whose content I saved.
- `your_instagram_activity/content/posts_*.json`, `reels.json`, `stories.json` —
  tags / mentions I made.
- `your_instagram_activity/profile/profile_searches.json` — accounts I looked up
  repeatedly (intent signal).
- `connections/follow_requests_you've_received.json` — inbound interest.

**Partial observability.** Reverse-direction signals (likes/comments _on_ my
posts) are **not** in the personal export — Instagram doesn't ship them. The
algorithm has to infer reciprocity from indirect evidence (DM direction
balance, story interaction balance, tag-back behavior) and accept that "they
engaged back" is a partially observable variable.

## Algorithm sketch

Score each followed account on `keep_probability ∈ [0, 1]`.

### Per-account features

| Feature                     | Source                                            | Direction / handling                           | Weight (initial)                  |
| --------------------------- | ------------------------------------------------- | ---------------------------------------------- | --------------------------------- |
| `dm_messages_total`         | inbox/`<thread>`                                  | outbound + inbound, log-scaled                 | high                              |
| `dm_recency_days`           | last message timestamp                            | exponential decay (τ ≈ 180d)                   | high                              |
| `dm_balance`                | outbound / (outbound + inbound)                   | penalize one-sided threads                     | medium                            |
| `likes_given`               | liked_posts / liked_comments                      | log-scaled, recency-weighted                   | medium                            |
| `comments_given`            | post / reels comments                             | log-scaled, recency-weighted                   | medium                            |
| `story_interactions_out`    | story_interactions/\*                             | log-scaled, recency-weighted                   | medium                            |
| `story_interactions_in`     | story_interactions/\*                             | inbound reactions/replies/poll-votes from them | medium-high (reciprocity proxy)   |
| `tagged_them`               | posts/reels/stories I made                        | count                                          | low                               |
| `they_tagged_me`            | rare in export; check `archived_posts` if present | count                                          | medium                            |
| `saved_their_content`       | saved_posts.json                                  | count                                          | low                               |
| `searched_for_them`         | profile_searches.json                             | count                                          | low (latent interest)             |
| `is_close_friend`           | close_friends.json                                | boolean                                        | hard boost                        |
| `recently_unfollowed_by_me` | recently_unfollowed_accounts.json                 | boolean                                        | exclude from set                  |
| `account_class`             | heuristic (below)                                 | public_figure / brand / personal               | gates the unfollow recommendation |

**Decay.** Every interaction count is recency-weighted with exponential decay so
a 2019 like is worth far less than a 2026 like. τ is configurable; start ≈ 180
days for DMs, 365 for content interactions.

### Account-class heuristic (gates "suggest unfollow")

- Username / display-name regex against a small lexicon (`official`, `studio`,
  `magazine`, `records`, `inc`, `co`, `gallery`, …).
- Known-brand allowlist the user maintains: [`config/keep_allowlist.txt`](../config/keep_allowlist.txt).
- Follower count cannot be inferred from the export, so the heuristic relies on
  name patterns + allowlist. **If uncertain, never auto-suggest unfollow — flag
  as `review`.**

### Scoring composition (initial form, iterate empirically)

```
engagement_raw = w1·dm + w2·likes + w3·comments + w4·story_out
reciprocity    = w5·story_in + w6·dm_balance_penalty
score_raw      = engagement_raw + reciprocity + close_friend_boost
keep_prob      = sigmoid((score_raw - threshold) / scale)
```

Weights and constants live in [`config/scoring.toml`](../config/scoring.toml)
so they can be tuned without a rebuild.

### Buckets

- `keep` (`keep_prob ≥ 0.7`) — solid two-way relationship or hard-boost account.
- `review` (`0.3 ≤ keep_prob < 0.7`) — ambiguous; needs my eyes.
- `unfollow` (`keep_prob < 0.3` **and** `account_class = personal` **and** not
  `close_friend`) — confident recommendation.

Public figures / brands with low `keep_prob` get `review`, never `unfollow` —
the decision criterion there ("do I still care about their content?") is
different and out of scope for the algorithm.

## Output

**Primary: CSV** — sortable, filterable, easy to diff between runs.

```
username,display_name,bucket,keep_prob,dm_msgs,last_dm_days,likes_given_90d,comments_given_90d,story_in_180d,account_class,notes
```

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

- **Story-interaction inbound robustness** — does the personal export reliably
  contain reactions/replies others sent **to my** stories, or only mine to
  theirs? Validate on a real export before leaning on it as the main
  reciprocity signal.
- **Recently-unfollowed as a negative signal** — useful (never re-suggest
  someone I already dropped) or too churny to bother?
- **Threshold tuning** — calibrate against personal judgment on a small labeled
  sample (~30 accounts I already know I want to keep/drop), or just iterate on
  the top/bottom of the ranking?
