# Instagram Manager (`ig-mgr`)

Local-first CLI that reads my Instagram personal data export and produces a
ranked recommendation file: who to unfollow (and remove from followers) versus
who to keep, with a `keep_probability` per account derived from the full
breadth of exported interactions. No UI, no API automation — I act on the
output manually inside Instagram.

```
┌──────────────────────────┐     ┌─────────────────┐     ┌────────────────────┐
│ IG personal data export  │ ──▶ │  CLI: ig-mgr    │ ──▶ │ recommendations.*  │
│ (unzipped export folder) │     │  score + rank   │     │ (CSV + MD summary) │
└──────────────────────────┘     └─────────────────┘     └────────────────────┘
```

One invocation, one input folder, two output files. I review the output and do
the unfollows by hand.

## Status

**Active — end-to-end pipeline landed.** Every functional ROADMAP slice is in:
the parser layer, per-account feature aggregation (raw counts, decay-weighted
counts, 90d/180d windowed counts), first-pass scoring (`keep_prob` plus a
`keep` / `review` / `unfollow` bucket per account), CSV + Markdown writers,
and the brand / public-figure account-class heuristic (with the
user-maintained keep-allowlist override) all run against a real export today.
The binary prints per-source counts plus top-10 / bottom-10 candidates with
their dominant feature, and writes `recommendations_<DATE>.csv` + `.md` next
to the export directory. Remaining: weight tuning against a labeled sample,
and the operational "run, clean up, evaluate regret" feedback loop. See
[`ROADMAP.md`](ROADMAP.md) for the task list and
[`docs/DESIGN.md`](docs/DESIGN.md) for the full design.

> A previous SvelteKit web-app prototype (card-deck review UI, SQLite/Drizzle)
> was retired — the interactive direction is friction I don't need for a
> one-shot periodic cleanup. This repo is a clean restart as a Rust CLI.

## Build & run

```bash
cargo build --release            # binary at target/release/ig-mgr
cargo run -- /path/to/export     # run against an unzipped export folder
cargo run -- /path/to/export --out ~/cleanup --verbose
```

Options: `--out <PATH>` (output stem, defaults next to the export),
`--config <PATH>` (scoring weights; when omitted, resolved from the dev tree,
your platform config dir, or a built-in default), `-v`/`-vv` (verbosity).
`RUST_LOG` overrides verbosity when set.

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
database. `clap` (CLI) · `serde`/`serde_json` (parsing) · `jiff` (time) ·
`rayon` (parallel scoring) · `csv` (output) · `tracing` (logs) · `anyhow` +
`thiserror` (errors). Tests: `insta` snapshots + `assert_cmd`. Rationale and
the deliberately-not-used list are in [`docs/DESIGN.md`](docs/DESIGN.md).

## Non-goals

- No web UI, card deck, or swipe interface.
- No Instagram API calls, scraping, or automated unfollow.
- No background daemon — one-shot run, exits when done.
- No persistent DB — the export is the source of truth; history = old output files.
- No login or credentials handling.

## License

[MIT](LICENSE)
