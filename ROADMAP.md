# Roadmap — `ig-mgr`

Implementation order. Each parser/feature step is verified against a real
export before moving on — see [`docs/DESIGN.md`](docs/DESIGN.md) for the design
behind each item.

- [x] **Download a fresh export** (2026-05-17) — pulled; nothing inspected or
      parsed yet, schema notes in DESIGN are presumed stale until re-verified.
- [x] **Scaffold the repository** (2026-05-20) — Rust CLI skeleton: module
      boundaries, dependencies, config files, CI, and smoke tests. No pipeline
      logic.
- [x] **Re-validate the file layout and JSON keys** (2026-05-26) — walked the
      2026-05-11 export with `scripts/walk_export_schema.sh`; DESIGN.md and
      scoring.toml rewritten against the actual schema. Re-run the walker
      against every fresh export to catch drift.
- [x] **Minimal parser** (2026-05-26) — `export::read_following`,
      `read_followers`, `read_inbox`; validated against the 2026-05-11 export:
      643 followings, 695 followers, 593 DM threads, 706,095 total messages
      (multi-part threads concatenated). Fixture-driven integration test in
      `tests/cli.rs` exercises shape A, shape B, and a multi-part thread.
- [ ] **Remaining feature extractors**, one at a time, verifying counts against
      spot-checks (likes, comments, stories, tags, saved, searches).
    - [x] **Relationship-flag parsers** (2026-05-26) — `read_close_friends`,
          `read_favorited`, `read_blocked`, `read_restricted`,
          `read_recently_unfollowed`, `read_removed_suggestions` (shape C
          arrays), `read_hide_story_from` (single-entry object), and
          `read_message_requests` (reuses the inbox thread parser via a shared
          `read_thread_dir` helper). Validated against the 2026-05-11 export:
          267 close friends, 49 favorited, 4 blocked, 5 restricted, 1 hidden,
          5 recently unfollowed, 24 removed suggestions, 10 message request
          threads. Owner extraction from `label_values.dict.dict` deferred to
          the next slice (lands with `liked_posts.json`).
    - [x] **Nested-`Owner` activity parsers** (2026-05-27) — extended
          `ShapeCLabelValue` with the `{title, dict}` nesting and added
          `read_liked_posts`, `read_story_likes`, `read_stories_viewed`,
          `read_saved_posts` (shape C with nested `Owner`) plus the
          `owner_username` helper that walks
          `label_values → title == "Owner" → dict[0].dict → label ==
"Username" → value`. Count lines in `lib::run` derive from
          `filter_map(owner_username).count()` so a silent serde drop of the
          nested fields surfaces as a zero count rather than a confidently
          wrong number. Validated against the 2026-05-11 export: 46,398
          liked posts, 28,357 story likes, 2,247 stories viewed, 205 saved
          posts.
    - [x] **Shape-A activity parsers** (2026-05-27) — added
          `read_liked_comments` and seven `read_story_*` readers for the
          `story_interactions/*` files (polls, quizzes, questions,
          emoji_sliders, emoji_story_reactions, story_reaction_sticker_reactions,
          countdowns). Each file gets its own private wrapper struct so the
          IG-specific wrapper key (e.g. `story_activities_emoji_quick_reactions`
          inside `emoji_story_reactions.json` — file name and wrapper key
          are _not_ symmetric) is compile-checked. New public
          `ShapeAEntry { username, timestamp }` returned by all eight; the
          private `shape_a_entries` helper drops entries with empty `title`
          so counts answer "real targets" not "deserialized objects".
          DESIGN.md wrapper-key table expanded to enumerate all 11 known
          keys. Validated against the 2026-05-11 export: 538 liked comments,
          1,045 polls, 111 quizzes, 63 questions, 102 emoji sliders, 13
          emoji story reactions, 20 reaction sticker reactions, 1 countdown.
    - [x] **Shape-D comment parsers** (2026-05-27) — added
          `read_post_comments` (bare-array, numbered with numeric-suffix
          sort for forward-compat with `post_comments_2.json`+),
          `read_reels_comments` (wrapper key `comments_reels_comments`),
          and `read_hype` (wrapper key `comments_story_comments` —
          file/wrapper-key asymmetry codified explicitly). New public
          `CommentEntry { target_username, timestamp }` returned by all
          three; the private `shape_d_entries` helper walks
          `string_map_data → "Media Owner" → value` and drops entries
          without an extractable owner. The "Media Owner" and "Time"
          spellings are codified as compile-checked `#[serde(rename)]`
          fields so a sub-key rename in IG's export degrades to a None at
          extraction time rather than passing through silently. Validated
          against the 2026-05-11 export: 631 post comments, 63 reels
          comments, 48 hype.
    - [x] **Resolver infrastructure** (2026-05-27) — added
          `read_me_identity` returning `MeIdentity { handle, name }` from
          `personal_information/personal_information/personal_information.json`
          (load-bearing for DM direction classification — missing or empty
          `Username`/`Name` is a HARD ERROR, not a silent default).
          Promoted `features.rs` → `features/mod.rs` and added
          `features::name_resolution::NameResolver` that builds a
          `display_name → handle` map from the seven `label_values` files
          (close_friends, favorited, blocked, restricted,
          recently_unfollowed, removed_suggestions, hide_story_from) —
          the only export-internal bridge since `following.json` ships
          handle-only. Collisions return `None` (no guessing); empty-string
          Name or Username entries are dropped so the empty key cannot
          become a phantom resolution path. Validated against the
          2026-05-11 export: handle `gati3478`, name `Gati Petriashvili`,
          281 unique names, 12 collisions, 217 (37%) of 1:1 DM threads
          resolve.
    - [x] **Handle-keyed feature aggregator (slice 7A)** (2026-05-27) —
          added `features::aggregate(inputs, now) -> Vec<AccountFeatures>`
          and `AggregateInputs<'a>` (borrowed bundle so the function stays
          callable without a 20-positional-arg signature). Output is keyed
          by handle, filtered to followings, and hard-excludes `is_blocked`
          / `recently_unfollowed` handles per DESIGN.md ("excludes from
          input set"). Populates: boolean flags (close_friend / favorited /
          blocked / restricted / hide_story_from / removed_suggestion /
          recently_unfollowed) from the OUTER-level `Username` on the seven
          `label_values` files (distinct from the nested-`Owner` walk in
          `owner_username`); `follow_tenure_days` from
          `FollowingEntry.followed_at` with `now` parameterized so tests
          pin a stable reference point; raw activity counts —
          `likes_given` (`liked_posts` via `owner_username` +
          `liked_comments` via `ShapeAEntry.username`), `comments_given`
          (`post_comments` + `reels_comments` + `hype` via
          `CommentEntry.target_username`), `story_interactions_out` (the
          seven shape-A `story_interactions/*` files), `stories_viewed`
          and `saved_their_content` (both via `owner_username`). DM
          features (`dm_messages_total`, `dm_recency_days`, `dm_balance`,
          `dm_reactions_given`, `dm_reactions_received`,
          `inbound_dm_request`) are defaulted to zero / `None` and
          populated in slice 7B alongside decay weighting and the
          90d/180d windowed counts. Validated against the 2026-05-11
          export: `aggregated accounts: 643` (blocked ∩ following = ∅
          and recently_unfollowed ∩ following = ∅ — IG auto-unfollows on
          block, so both hard-exclude filters are no-ops on this export
          but still pin the semantic), `aggregated close friends: 267`
          (= |close_friends ∩ following|), `aggregated favorited: 48`
          (= |favorited ∩ following|; one favorited handle is no longer
          followed), `aggregated with likes_given > 0: 561`, `aggregated
          with comments_given > 0: 139`. Intersections cross-checked with
          `comm -12` against `jq`-extracted handle sets.
- [x] **DM features (slice 7B-1)** (2026-05-27) — extended
      `AggregateInputs` with `inbox_threads`, `message_request_threads`,
      `me`, `resolver`. The aggregator now resolves each 1:1 inbox thread
      via `attributable_handle` (display name → handle through the
      `NameResolver`, after stripping `me.name` from participants and
      rejecting both group chats with ≥ 2 other participants and
      abandoned threads with 0 others — both exclusions explicit in
      DESIGN.md). Threads that resolve and whose handle is in the
      followings set credit `dm_messages_total`, direction-classified
      counters, reactions (`actor == me.name` → given, otherwise
      received, `None` skipped), and a max-timestamp `dm_recency_days`.
      `dm_balance` and `dm_recency_days` finalize from a sidecar
      `DmAccum` map so multi-thread aliasing (a followee surfaced under
      two `(Name, Username)` pairs in different `label_values` files)
      composes by union, not last-thread-wins. A separate pass over
      `message_request_threads` flips `inbound_dm_request` (the boolean
      is the only signal DESIGN sources from `message_requests/`;
      counts and reactions stay sourced from inbox-only). Validated
      against the 2026-05-11 export: `DM-attributed accounts: 214`
      (217 resolvable threads minus 3 whose resolved handle is no
      longer a followee), `DM reactions given total: 36027`, `DM
      reactions received total: 39724` (the true inbound signal —
      received > given, consistent with DESIGN's "DM reactions are the
      single most valuable bidirectional signal" framing), `inbound
      DM requests: 1`.
- [x] **Decay + windowed counts + config (slice 7B-2)** (2026-05-27) —
      replaced the `src/config.rs` scaffold with `ScoringConfig` +
      `DecayConfig` (`tau_dm_days`, `tau_content_days`) loaded from
      `config/scoring.toml`. Path resolution: `--config <PATH>` → cwd
      `./config/scoring.toml` → compiled-in default via `include_str!`
      so a fresh install runs zero-config. `weights` and `scoring`
      sections are tolerated (serde ignore-unknown) and surface in the
      scoring slice. Aggregator gained eight `*_decayed: f64` fields
      (one per count feature) where each entry contributes
      `exp(-Δt_days / τ_days)` to the sum — `tau_content_days` for
      activity, `tau_dm_days` for DM signals. Missing or future
      timestamps contribute `0.0` (honest-counting parity with the raw
      counts). Plus the four DESIGN.md CSV-header windowed counts —
      `likes_given_90d`, `comments_given_90d`,
      `dm_reactions_given_180d`, `dm_reactions_received_180d` —
      computed under a half-open `secs / 86_400 < window_days`
      predicate. Reactions inherit their parent message's timestamp
      for both decay and the 180d window (the export doesn't ship
      per-reaction timestamps; reactions are approximately
      contemporaneous with the message). Validated against the
      2026-05-11 export: `decayed DM messages sum: 24235.25` (raw
      706,095 — heavy decay since most of the 706k messages are old),
      `decayed reactions received sum: 3485.06` (raw 39,724), `90d
      likes total: 1489`, `90d comments total: 8`, `180d reactions
      given total: 3361`, `180d reactions received total: 3022`.
- [x] **First-pass scoring** (2026-05-27) — extended `ScoringConfig`
      with `WeightsConfig` (16 fields, 1:1 with `[weights]` in
      `config/scoring.toml`) and `ScoringParams` (`threshold`, `scale`,
      `keep_min`, `unfollow_max`). `validate()` now rejects non-finite
      weights, `scale <= 0`, `keep_min <= unfollow_max`, and
      keep_min/unfollow_max outside `[0, 1]` — same NaN/poison posture
      that gated zero τ in slice 7B-2. `src/scoring.rs` composes
      decayed counts + log-tenure + boolean boosts − derived penalties
      (`dm_balance_penalty` and `reaction_balance_penalty` computed in
      scoring with volume gates of 5 messages / 5 reactions; asymmetric
      so one-sided-them is reciprocity not over-extension), runs the
      sum through `keep_prob = sigmoid((score_raw - threshold) / scale)`,
      and assigns a bucket per DESIGN.md (restricted floors at review;
      unfollow requires `!is_close_friend && !is_favorited` for now —
      the `account_class == personal` gate lands with the brand /
      public-figure slice). Each `ScoredAccount` carries a
      `dominant_feature: &'static str` for the Markdown summary,
      chosen as the largest-magnitude contribution with penalties
      signed negative so a penalty-driven account surfaces as
      "hide_story_penalty" rather than the smaller positive term.
      Validated against the 2026-05-11 export: 643 accounts scored,
      top 10 keep_prob = 1.000 dominated by `dm` / `likes`, bottom 10
      keep_prob 0.72–0.86 dominated by `tenure` (passive longevity, no
      interaction). With the first-pass weights the distribution skews
      heavily Keep (641 / 2 / 0); weight tuning is the next bullet.
- [x] **Tune weights and decay constants** (2026-05-27) — first calibration
      pass: 481 / 159 / 3 (Keep / Review / Unfollow) on 643 followings, down
      from 641 / 2 / 0 baseline. Two edits — `threshold 0.0 → 1.5`,
      `tenure 0.3 → 0.15` — both attributable through the new histogram +
      `--trace` surface in `lib::run`. Decay constants unchanged. Hybrid
      methodology per DESIGN.md "Open questions": iterate on the live
      ranking, with optional `config/labels.txt` (loader + confusion matrix
      in `src/labels.rs`) as the held-out accuracy floor when laid down.
      Further Unfollow widening deferred until the brand / public-figure
      heuristic ships. Full notes: [`docs/TUNING.md`](docs/TUNING.md).
- [ ] **Brand / public-figure heuristic** + user-maintained allowlist.
- [x] **CSV + Markdown output writers** (2026-05-27) — `src/output/` with
      `csv` and `markdown` submodules. CSV columns pin
      [`docs/DESIGN.md`](docs/DESIGN.md) "Output" verbatim; rows emit
      ascending by `keep_prob` so actionable accounts surface at the top
      of the file. Markdown is the skim artifact: bucket counts plus
      bottom-20 / top-20 tables with `dominant_feature` per row. Default
      stem `<export-parent>/recommendations_<YYYY-MM-DD>`, overridable
      via `--out`. Display names resolve via `NameResolver::display_name_for`
      (the reverse direction added in this slice; same "no guessing on
      collision" posture as the forward direction). `account_class`
      stub-materialized as a single `Personal` variant — the brand /
      public-figure slice upgrades that field in place rather than
      adding a new one.
- [ ] **Run on the real export**, do the cleanup, evaluate regret a few weeks
      later, iterate.
