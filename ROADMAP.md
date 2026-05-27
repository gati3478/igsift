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
- [ ] **First-pass scoring** with hand-set weights; eyeball top/bottom 50.
- [ ] **Tune weights and decay constants** — consider a small labeled set of
      ~30 accounts I already know I want to keep/drop, fit weights to match.
- [ ] **Brand / public-figure heuristic** + user-maintained allowlist.
- [ ] **CSV + Markdown output writers.**
- [ ] **Run on the real export**, do the cleanup, evaluate regret a few weeks
      later, iterate.
