# CLAUDE.md

## Project overview

`ig-mgr` — a local-first Rust **CLI** that reads an Instagram personal data
export and produces a ranked recommendation file (CSV + Markdown): who to
unfollow vs. keep, with a `keep_probability` per account. One-shot run, no UI,
no network, no database, no automated unfollow. The user acts on the output
manually inside Instagram.

**Current state: scaffolding only.** Module boundaries, dependencies, config,
CI, and smoke tests exist; the analysis pipeline is **not implemented**. Read
[`docs/DESIGN.md`](docs/DESIGN.md) for the algorithm and [`ROADMAP.md`](ROADMAP.md)
for build order before writing pipeline code.

## Tech stack

- **Language**: Rust, edition 2024, stable toolchain (`rust-toolchain.toml` pins it).
- **Package shape**: single package, two targets — library crate `ig_mgr`
  (`src/lib.rs`) holds the logic; binary `ig-mgr` (`src/main.rs`) is a thin
  shell. **Not** a workspace. Integration tests in `tests/` use the lib.
- **Dependencies**: the full set lives in `Cargo.toml`; per-crate rationale and
  the deliberately-not-used list are in [`docs/DESIGN.md`](docs/DESIGN.md). Two
  picks an agent should not "modernize away": **`jiff`** (not `chrono`) for
  time, and **`serde_path_to_error`** wrapping every parse — it is the
  schema-drift survival mechanism (see Conventions), not optional ceremony.

## Commands

```bash
cargo build --all-targets                      # compile lib, bin, tests
cargo run -- <export-dir> [--out P] [--config P] [-v]
cargo fmt --all                                # format (rustfmt.toml: edition 2024)
cargo clippy --all-targets -- -D warnings      # lint; CI treats warnings as errors
cargo nextest run                              # tests (cargo test also works)
cargo insta review                             # accept/reject snapshot changes
```

Set up once per clone: `brew install lefthook && lefthook install` (pre-commit fmt + pre-push clippy/test mirror CI — see `lefthook.yml`). Optional CI tools: `cargo install --locked cargo-nextest && cargo install cargo-insta` (nextest only installs with `--locked`).

## Layout

```
src/
  main.rs       # binary entry: parse args, init tracing, call run()
  lib.rs        # run() orchestration, init_tracing(); re-exports modules
  cli.rs        # clap derive: Cli (export_dir, --out, --config, --verbose)
  config.rs     # scoring.toml deserialization               [stub]
  export.rs     # IG export JSON parsers                      [stub]
  features.rs   # per-account feature aggregation             [stub]
  scoring.rs    # keep_probability + bucketing                [stub]
  output.rs     # CSV + Markdown writers                      [stub]
tests/
  cli.rs                  # E2E smoke tests via assert_cmd
  fixtures/sample_export/ # sanitized fixture (to be added)
config/
  scoring.toml       # tunable weights + decay constants
  keep_allowlist.txt # user-maintained never-unfollow list
scripts/
  walk_export_schema.sh # JSON-schema walker; drift detector for IG exports
docs/DESIGN.md  ROADMAP.md
```

`[stub]` modules are doc-comment-only and document their intended
responsibility and planned submodules. `export.rs` / `output.rs` graduate to
`export/mod.rs` / `output/mod.rs` when submodules land.

## Conventions

- **Privacy first.** A real export contains personal data; never commit one.
  Exports live in `ig-exported-data/` (gitignored); the binary's positional
  path can point anywhere — inside or outside the repo. `.gitignore` is the
  safety net but a fragile one: only the names listed there
  (`/ig-exported-data/`, `/ig_data/`, `/export/`, `/exports/`, `*.zip`) are
  matched. An export dropped at any other name **will** be tracked. Test
  fixtures must be sanitized synthetic data.
- **Schema drift is the main risk.** Instagram rotates export paths/keys
  silently. Parsers use `#[serde(default)]` + `Option<T>` and
  `serde_path_to_error` so a changed file degrades or fails _loudly with the
  offending path_, not silently. Re-run `scripts/walk_export_schema.sh`
  against every fresh export and diff against the last-known-good output to
  catch drift before it bites the parser.
- **Errors**: `anyhow::Result` in `main`/orchestration; `thiserror` enums inside
  parser modules.
- **Tunables in TOML**, not code — weights/decay/thresholds live in
  `config/scoring.toml` so iteration needs no rebuild.
- `unsafe` is forbidden (`[lints.rust] unsafe_code = "forbid"`).

## Non-goals

No web UI / TUI, no Instagram API / scraping / automated unfollow, no daemon,
no DB, no login, no async or network crates — the export is the source of truth
and the run is one-shot. A `ratatui review` subcommand is a possible v2, not v1.
Full rationale and the deliberately-not-used crate list:
[`docs/DESIGN.md`](docs/DESIGN.md) ("Deliberately not using").
