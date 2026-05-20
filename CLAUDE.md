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
- **Key crates**: `clap` (CLI), `serde`/`serde_json`/`serde_path_to_error`
  (parsing), `toml` (config), `jiff` (time), `rayon` (parallel scoring), `csv`
  (output), `tracing` (logs), `anyhow` + `thiserror` (errors), `indicatif` +
  `owo-colors` (UX). Tests: `insta` (snapshots), `assert_cmd` + `predicates`.

## Commands

```bash
cargo build --all-targets                      # compile lib, bin, tests
cargo run -- <export-dir> [--out P] [--config P] [-v]
cargo fmt --all                                # format (rustfmt.toml: edition 2024)
cargo clippy --all-targets -- -D warnings      # lint; CI treats warnings as errors
cargo nextest run                              # tests (cargo test also works)
cargo insta review                             # accept/reject snapshot changes
```

Local tools (optional, used by CI): `cargo install cargo-nextest cargo-insta`.

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
docs/DESIGN.md  ROADMAP.md
```

`[stub]` modules are doc-comment-only and document their intended
responsibility and planned submodules. `export.rs` / `output.rs` graduate to
`export/mod.rs` / `output/mod.rs` when submodules land.

## Conventions

- **Privacy first.** A real export contains personal data; never commit one. The
  `.gitignore` blocks `ig_data/`, `export*/`, `*.zip`, and `recommendations_*`.
  Test fixtures must be sanitized synthetic data.
- **Schema drift is the main risk.** Instagram rotates export paths/keys
  silently. Parsers use `#[serde(default)]` + `Option<T>` and
  `serde_path_to_error` so a changed file degrades or fails _loudly with the
  offending path_, not silently. Re-verify paths against a fresh export before
  trusting `docs/DESIGN.md`.
- **Errors**: `anyhow::Result` in `main`/orchestration; `thiserror` enums inside
  parser modules.
- **Tunables in TOML**, not code — weights/decay/thresholds live in
  `config/scoring.toml` so iteration needs no rebuild.
- `unsafe` is forbidden (`[lints.rust] unsafe_code = "forbid"`).

## Non-goals

No web UI / card deck / TUI (a `ratatui review` subcommand is a possible v2, not
v1). No Instagram API, scraping, or automated unfollow. No daemon. No DB — the
export is the source of truth. No login/credentials. No async/`tokio`, no
network crates.
