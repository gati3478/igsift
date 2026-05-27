# CLAUDE.md

## Project overview

`ig-mgr` — a local-first Rust **CLI** that reads an Instagram personal data
export and produces a ranked recommendation file (CSV + Markdown): who to
unfollow vs. keep, with a `keep_probability` per account. One-shot run, no UI,
no network, no database, no automated unfollow. The user acts on the output
manually inside Instagram.

**Current state: end-to-end pipeline landed + four tuning rounds.** Every
functional ROADMAP slice is in: parser layer (every JSON source DESIGN.md
lists — relationships, DM inbox + message requests, nested-`Owner` activity,
shape-A activity, shape-D comments, the `me` identity and `display_name →
handle` resolver), per-account feature aggregation (raw + decay-weighted + DM
features + 90d/180d windowed counts), scoring (`keep_prob` + bucket per
account), CSV + Markdown writers, brand / public-figure account-class
heuristic (16-token lexicon) with user-maintained keep-allowlist override
(`config/keep_allowlist.txt`), and the held-out labeled-set confusion-matrix
report (`config/labels.txt` loaded by `src/labels.rs`) — the accuracy floor
for weight tuning. Four tuning rounds landed (threshold + tenure calibration,
`unfollow_max` widening, brand-lexicon expansion); current bucket split on
the real export is `481 / 155 / 7` (keep / review / unfollow) at `25%`
labeled-set agreement, 0 hard mismatches. Remaining: the operational "run,
clean up, evaluate regret" feedback loop. Read
[`docs/DESIGN.md`](docs/DESIGN.md) for the algorithm,
[`ROADMAP.md`](ROADMAP.md) for build order, and
[`docs/TUNING.md`](docs/TUNING.md) for the tuning journal (newest at top)
before writing pipeline code.

## Tech stack

- **Language**: Rust, edition 2024, stable toolchain (`rust-toolchain.toml` pins it).
- **Package shape**: single package, two targets — library crate `ig_mgr`
  (`src/lib.rs`) holds the logic; binary `ig-mgr` (`src/main.rs`) is a thin
  shell. **Not** a workspace. Integration tests in `tests/` use the lib.
- **Dependencies**: the full set lives in `Cargo.toml`; per-crate rationale and
  the deliberately-not-used list are in [`docs/DESIGN.md`](docs/DESIGN.md).
  Three picks an agent should not "modernize away": **`jiff`** (not `chrono`)
  for time, **`serde_path_to_error`** wrapping every parse — the schema-drift
  survival mechanism (see Conventions), not optional ceremony — and
  **`aho-corasick`** for the brand-suffix lexicon (single-automaton pass over
  each handle vs. N independent `str::contains` calls).

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
  main.rs                       # binary entry: parse args, init tracing, call run()
  lib.rs                        # run() orchestration, init_tracing(); re-exports modules
  cli.rs                        # clap derive: Cli (export_dir, --out, --config, --verbose)
  config.rs                     # scoring.toml deserialization (decay + weights + scoring params)
  export.rs                     # IG export JSON parsers — every source DESIGN.md lists
  allowlist.rs                  # config/keep_allowlist.txt loader (case-insensitive HashSet)
  labels.rs                     # config/labels.txt loader + confusion-matrix report
  features/
    mod.rs                      # re-exports
    aggregate.rs                # per-account features: raw + decayed + windowed counts
    name_resolution.rs          # display_name → handle bridge for DM attribution
    account_class.rs            # brand-detection (aho-corasick lexicon) + allowlist gate
  scoring.rs                    # score_raw composition, sigmoid, bucket assignment
  output/
    mod.rs                      # CSV + Markdown writer dispatcher
    csv.rs                      # CSV row writer (DESIGN.md "Output" header is the contract)
    markdown.rs                 # skim-summary Markdown: top/bottom-20 tables
tests/
  cli.rs                  # smoke tests + fixture-count assertions (locked-in)
  fixtures/sample_export/ # sanitized synthetic export
config/
  scoring.toml             # tunable weights + decay constants
  keep_allowlist.txt       # user-maintained never-unfollow list (template; per-user content gitignored)
  labels.txt.example       # per-user labels template (real `labels.txt` gitignored)
scripts/
  walk_export_schema.sh # JSON-schema walker; drift detector for IG exports
docs/DESIGN.md  ROADMAP.md  TUNING.md
```

## Conventions

- **Privacy first.** A real export contains personal data; never commit one.
  Exports live in `ig-exported-data/` (gitignored); the binary's positional
  path can point anywhere — inside or outside the repo. `.gitignore` is the
  safety net but a fragile one: only the names listed there
  (`/ig-exported-data/`, `/ig_data/`, `/export/`, `/exports/`, `*.zip`) are
  matched. An export dropped at any other name **will** be tracked. Test
  fixtures must be sanitized synthetic data. The same posture extends to
  **committed docs**: personal followee handles paired with the user's
  explicit `keep` / `drop` intent are the same disclosure as the gitignored
  `config/labels.txt`. Use structural descriptors (`a label=drop account at
keep_prob=0.302`) instead of raw handles when documenting tuning rounds.
  Brand-business handles (public-facing pages like `tbilisicamerashop`) are
  fine in committed docs because the brand name is already public.
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
- **Fixture counts are locked-in.** `tests/cli.rs` asserts exact integer
  counts against the synthetic fixture. If a count drifts after a parser
  change, the parser silently dropped data — diagnose the parser, don't
  relax the assertion. Pair with the structural unit tests in
  `src/export.rs` (`#[cfg(test)] mod tests`) which pin nested fields so
  counts alone can't paper over a regression that returns defaulted
  entries. The `ig_mgr()` test helper spawns the binary with
  `cwd = std::env::temp_dir()` so the cwd-relative `config/*` loaders miss
  any per-user files at the repo root — without this, a developer with
  their own `config/labels.txt` or `config/keep_allowlist.txt` sees
  fixture counts contaminate. Don't undo the cwd override.
- `unsafe` is forbidden (`[lints.rust] unsafe_code = "forbid"`).

## Non-goals

No web UI / TUI, no Instagram API / scraping / automated unfollow, no daemon,
no DB, no login, no async or network crates — the export is the source of truth
and the run is one-shot. A `ratatui review` subcommand is a possible v2, not v1.
Full rationale and the deliberately-not-used crate list:
[`docs/DESIGN.md`](docs/DESIGN.md) ("Deliberately not using").
