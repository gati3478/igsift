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
- [ ] **Feature aggregation** — `features::aggregate(...)` produces
      `Vec<AccountFeatures>` keyed by handle, filtered to followings. Boolean
      flags (close_friend/favorited/blocked/restricted/etc.), activity
      counts (likes_given/comments_given/story_interactions_out/etc.),
      DM features (dm_messages_total/dm_recency_days/dm_balance/
      dm_reactions_given/dm_reactions_received/inbound_dm_request) gated
      on resolvable threads via [`features::name_resolution`], and
      `follow_tenure_days` from `FollowingEntry.followed_at`. Decay-weighted
      counts per `config/scoring.toml [decay]` plus raw 90d/180d windowed
      counts for the CSV columns (DESIGN.md is explicit these are different
      aggregations).
- [ ] **First-pass scoring** with hand-set weights; eyeball top/bottom 50.
- [ ] **Tune weights and decay constants** — consider a small labeled set of
      ~30 accounts I already know I want to keep/drop, fit weights to match.
- [ ] **Brand / public-figure heuristic** + user-maintained allowlist.
- [ ] **CSV + Markdown output writers.**
- [ ] **Run on the real export**, do the cleanup, evaluate regret a few weeks
      later, iterate.
