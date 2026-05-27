# Instagram Manager (`ig-mgr`)

Local-first CLI that reads my Instagram personal data export and produces a
ranked recommendation file: who to unfollow (and remove from followers) versus
who to keep, with a `keep_probability` per account derived from the full
breadth of exported interactions. No UI, no API automation — I act on the
output manually inside Instagram.

```
┌──────────────────────────┐     ┌─────────────────┐     ┌────────────────────┐
│ IG personal data export  │ ──▶ │  CLI: ig-mgr    │ ──▶ │ following-audit.*  │
│ (unzipped export folder) │     │  score + rank   │     │ (CSV + MD summary) │
└──────────────────────────┘     └─────────────────┘     └────────────────────┘
```

One invocation, one input folder, two output files. I review the output and do
the unfollows by hand.

## Status

**Active — end-to-end pipeline landed.** Every functional ROADMAP slice is in:
the parser layer, per-account feature aggregation (raw + decay-weighted +
90d/180d windowed counts), scoring (`keep_prob` plus a
`keep` / `review` / `unfollow` bucket per account), CSV + Markdown writers,
brand / public-figure account-class heuristic (16-token lexicon with
user-maintained keep-allowlist override), and the held-out
labeled-set confusion-matrix report (`config/labels.txt`) used for weight
tuning. Four tuning rounds landed: threshold + tenure calibration,
`unfollow_max` widening against the labeled set, brand-lexicon expansion.
The binary prints per-source counts plus top-10 / bottom-10 candidates with
their dominant feature, and writes `following-audit_<DATE>.csv` + `.md` next
to the export directory. Remaining: the operational "run, clean up, evaluate
regret" feedback loop. See [`ROADMAP.md`](ROADMAP.md) for the task list,
[`docs/DESIGN.md`](docs/DESIGN.md) for the full design, and
[`docs/TUNING.md`](docs/TUNING.md) for the weight-tuning journal.

> A previous SvelteKit web-app prototype (card-deck review UI, SQLite/Drizzle)
> was retired — the interactive direction is friction I don't need for a
> one-shot periodic cleanup. This repo is a clean restart as a Rust CLI.

## Build & run

```bash
cargo build --release             # binary at target/release/ig-mgr
cargo run -- /path/to/export      # run against an unzipped export folder
cargo run -- /path/to/export.zip  # single .zip — extracted + cached
cargo run -- /path/to/parts/      # directory of multipart .zip parts
cargo run -- /path/to/export --out ~/cleanup --verbose
```

Three input shapes are accepted transparently: an already-extracted
directory, a single `.zip` file, or a directory of multipart `*.zip`
parts that IG ships for large exports. Archives extract to
`.ig-mgr-extracted*/` next to the input and are cached on re-runs.
Use `--rebuild-cache` to force a fresh extract.

Options:

- `--out <PATH>` — output stem; defaults to `following-audit_<DATE>.{csv,md}`
  next to the input.
- `--preset <NAME>` — pick a shipped scoring shape (`balanced`,
  `engagement`, `tenure`). Mutually exclusive with `--config`. See
  Quickstart below.
- `--config <PATH>` — scoring weights TOML; when omitted, resolved as
  `./config/scoring.toml` in the cwd, then a compiled-in default
  (= the `balanced` preset). A platform config dir
  (`~/.config/ig-mgr/`) is not yet wired.
- `--rebuild-cache` — force a fresh extract of an archive input.
- `--trace <HANDLE>` — print the full per-term scoring breakdown for one
  followee handle. Errors if the handle isn't in the followings set after
  blocked / recently-unfollowed exclusions. Use during tuning to answer
  "why did this account rank where it did?".
- `-v` / `-vv` — debug / trace log verbosity. `RUST_LOG` overrides when set.

## Quickstart for first-time users

You don't need to write any config files to get useful output. The
binary ships three weight presets and picks `balanced` by default:

```bash
ig-mgr ./instagram-export-folder              # uses balanced preset
ig-mgr ./instagram-export-folder --preset engagement
ig-mgr ./instagram-export-folder --preset tenure
```

- **balanced** — sensible middle ground; no signal type dominates.
- **engagement** — surfaces "who do I actually talk to / engage with?";
  demotes dormant tenure-only follows.
- **tenure** — preserves long-standing follows even when interaction
  has tailed off; softens engagement signals.

Iterate from here by:

1. `ig-mgr init` to scaffold `config/keep_allowlist.txt` and
   `config/labels.txt`.
2. Append accounts you want to **never** unfollow to
   `config/keep_allowlist.txt`.
3. Hand-label 20–30 followees in `config/labels.txt` (format in the
   template). The binary prints a confusion-matrix report against
   your labels at the end of every run.
4. Copy a preset to `config/scoring.toml` (e.g.
   `cp config/presets/engagement.toml config/scoring.toml`) and edit
   weights to chase higher label agreement. See
   [`docs/TUNING.md`](docs/TUNING.md) for the journal of how the
   committed `config/scoring.toml` was tuned against a 643-account
   labeled set.

## Development

```bash
cargo build --all-targets        # compile lib, bin, and tests
cargo fmt --all                  # format
cargo clippy --all-targets -- -D warnings
cargo nextest run                # tests (or: cargo test)
cargo insta review               # review snapshot changes (once snapshots exist)
```

`cargo-nextest` and `cargo-insta` are optional local tools:
`cargo install --locked cargo-nextest && cargo install cargo-insta`
(nextest only installs with `--locked`). CI uses nextest.

Local git hooks are managed by [Lefthook](https://github.com/evilmartians/lefthook)
([`lefthook.yml`](lefthook.yml)): `pre-commit` runs `cargo fmt --check` (fast
gate), `pre-push` runs `cargo clippy -D warnings` and `cargo nextest run`
(mirrors CI). Set up once per clone: `brew install lefthook && lefthook install`.

## Tech stack

Rust (edition 2024, stable) — single static binary, no async, no network, no
database. `clap` (CLI) · `serde`/`serde_json` + `serde_path_to_error`
(schema-drift-survivable parsing) · `jiff` (time) · `rayon` (parallel
scoring) · `aho-corasick` (brand-suffix lexicon, single-pass automaton) ·
`csv` (output) · `tracing` (logs) · `anyhow` + `thiserror` (errors). Tests:
`insta` snapshots + `assert_cmd` + `cargo-nextest`. Rationale and the
deliberately-not-used list are in [`docs/DESIGN.md`](docs/DESIGN.md).

## Non-goals

- No web UI, card deck, or swipe interface.
- No Instagram API calls, scraping, or automated unfollow.
- No background daemon — one-shot run, exits when done.
- No persistent DB — the export is the source of truth; history = old output files.
- No login or credentials handling.

## License

[MIT](LICENSE)
