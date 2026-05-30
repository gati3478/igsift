# CLAUDE.md

## Project overview

`ig-mgr` — a local-first Rust **CLI** that reads an Instagram personal data
export and writes a three-format audit (CSV + Markdown + HTML): who to
unfollow vs. keep, with a `keep_probability` per account. One-shot run, no UI,
no network, no database, no automated unfollow. The user acts on the output
manually inside Instagram. See [`README.md`](README.md) for the user-facing
quickstart and CLI surface, [`docs/DESIGN.md`](docs/DESIGN.md) for the
algorithm and the "Output" header contract, [`ROADMAP.md`](ROADMAP.md) for
build order, and [`docs/TUNING.md`](docs/TUNING.md) for the weight-tuning
journal.

**Pipeline shape:**

```text
input ──▶ archive::resolve  (dir / .zip / multipart .zip → extracted dir)
      ──▶ export::*          (JSON parsers, schema-drift survivable)
      ──▶ features           (per-account: raw + decay-weighted + 90d/180d)
      ──▶ scoring            (keep_prob + bucket)
      ──▶ output::*          (CSV + Markdown + HTML writers)
```

**Three subcommands** (legacy `ig-mgr <export>` still works as implicit Run):
`run` (score + write audit), `init` (scaffold per-user config files from
embedded templates), `check` (parser dry-run with per-source pass/fail,
plus a config sanity check that the keep-allowlist + drop-list parse and
are disjoint).

**Current bucket split on the real export:** `510 / 130 / 9`
(keep / review / unfollow) at `28.6%` labeled-set agreement (8/28 matched
labels), 1 hard mismatch. Measured 2026-05-29 with `story_out = 0.5`
(halved after `story_likes.json`, ~28k events, folded into
`story_interactions_out` and ~doubled its volume).

**Agreement is feature-ceilinged (~30–36%), not a tuning bug.** The labeled
set shows keep/drop intent is largely _non-separable_ on the current
features: DM is the only clean keep signal (every DM-dominated label is
keep), while `story_out` is a coin flip (it dominates an equal mix of
keep- and drop-labeled accounts) and ~12 keep-labels are low-engagement
brand/local follows that only `tenure` carries — raising them would also
raise drop-intent old follows. The remaining hard mismatch is a
story-heavy drop indistinguishable from story-heavy keeps. The fix is a
**drop-list** (`config/drop_list.txt`, the mirror of `keep_allowlist`) —
**implemented**: a hand-flagged handle is forced to `Unfollow` regardless
of score, closing the hard mismatch that weight-tuning structurally can't
(tuning here trades keep-recall for drop-precision ~1:1). See
[`docs/TUNING.md`](docs/TUNING.md). Scoring weights live in
`config/scoring.toml`; three presets (`balanced` / `engagement` /
`tenure`) ship embedded via `--preset`. `balanced` mirrors the committed
`scoring.toml` (both carry the `story_out = 0.5` correction) and is the
compiled-in fallback when no flag and no cwd file resolve.

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
cargo build --all-targets                          # compile lib, bin, tests
cargo run -- <input> [flags]                       # implicit Run (legacy)
cargo run -- check <input> [--rebuild-cache]       # parser dry-run + config sanity (disjoint lists)
cargo run -- init [--force]                        # scaffold config/ files
cargo fmt --all                                    # format (rustfmt.toml: edition 2024)
cargo clippy --all-targets -- -D warnings          # lint; CI treats warnings as errors
cargo nextest run                                  # tests (cargo test also works)
cargo deny check advisories bans sources           # supply-chain gate (CI + pre-push)
```

Run flags: `--out PATH` (output stem), `--preset balanced|engagement|tenure`
or `--config PATH` (mutually exclusive), `--trace HANDLE` (per-term scoring
breakdown), `--rebuild-cache` (force archive re-extract), `-v` / `-vv`
(debug / trace verbosity; `RUST_LOG` overrides).

`<input>` is a directory (extracted or containing `*.zip` parts) or a
single `.zip` — `archive::resolve` detects and extracts transparently
into `.ig-mgr-extracted*/` next to the input, with cache reuse keyed on
a `{count}\n{total_bytes}` fingerprint (mtime-immune to `cp -p` and
`rsync --times`).

Set up once per clone: `brew install lefthook && lefthook install` (pre-commit fmt + pre-push clippy/test/cargo-deny mirror CI — see `lefthook.yml`). Optional CI tools: `cargo install --locked cargo-nextest cargo-deny` (both install with `--locked`).

## Layout

```
src/
  main.rs                       # binary entry: dispatch on subcommand (run / init / check)
  lib.rs                        # run() / init() / check() orchestration; init_tracing(); re-exports
  cli.rs                        # clap derive: Cli, RunArgs, Command (Run/Init/Check), Preset enum
  archive.rs                    # input detect + zip extract + cache (.ig-mgr-extracted*/)
  config.rs                     # scoring.toml deserialization; preset resolution chain
  export.rs                     # IG export JSON parsers + validate_shape pre-flight
  allowlist.rs                  # config/{keep_allowlist,drop_list}.txt loaders + ensure_disjoint
  labels.rs                     # config/labels.txt loader + confusion-matrix report
  progress.rs                   # indicatif spinner wrapper (auto-hide on -v or off-TTY)
  text.rs                       # fix_mojibake — repairs IG's UTF-8-as-Latin-1 export bug
  features/
    mod.rs                      # re-exports
    aggregate.rs                # per-account features: raw + decayed + windowed + is_mutual
    name_resolution.rs          # display_name → handle bridge for DM attribution
    account_class.rs            # brand-detection (aho-corasick lexicon) + keep-allowlist / drop-list gates
  scoring.rs                    # score_raw composition, sigmoid, bucket assignment, top_terms
  output/
    mod.rs                      # write() dispatcher (CSV+MD+HTML) + shared decision_hint SSOT
    csv.rs                      # CSV row writer (DESIGN.md "Output" header is the contract)
    markdown.rs                 # decision-oriented MD: per-bucket cards + tables
    html.rs                     # self-contained HTML report (inline CSS + JS, no deps)
tests/
  cli.rs                  # binary integration tests + fixture-count assertions (locked-in)
  fixtures/sample_export/ # sanitized synthetic export
config/
  scoring.toml                 # Gati's tuned weights + decay constants
  keep_allowlist.txt.example   # per-user keep-allowlist template (real .txt gitignored)
  drop_list.txt.example        # per-user drop-list template — forces Unfollow (real .txt gitignored)
  labels.txt.example           # per-user labels template (real .txt gitignored)
  presets/
    balanced.toml              # default preset — mirror of config/scoring.toml; compiled-in fallback
    engagement.toml            # engagement-weighted preset
    tenure.toml                # tenure-weighted preset
scripts/
  walk_export_schema.sh # JSON-schema walker; drift detector for IG exports
docs/DESIGN.md  docs/TUNING.md  docs/GOING-PUBLIC.md  ROADMAP.md
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
  `config/labels.txt`. Use structural descriptors
  (`a label=drop account at keep_prob=0.302`) instead of raw handles
  when documenting tuning rounds.
  Brand-business handles (public-facing pages like `tbilisicamerashop`) are
  fine in committed docs because the brand name is already public.
- **Schema drift is the main risk.** Instagram rotates export paths/keys
  silently. Parsers use `#[serde(default)]` + `Option<T>` and
  `serde_path_to_error` so a changed file degrades or fails _loudly with the
  offending path_, not silently. Re-run `scripts/walk_export_schema.sh`
  against every fresh export and diff against the last-known-good output to
  catch drift before it bites the parser.
- **Errors**: `anyhow::Result` throughout, with `.context(...)` /
  `serde_path_to_error` carrying the offending path on parse failures.
- **Tunables in TOML**, not code — weights/decay/thresholds live in
  `config/scoring.toml` (or one of the three embedded presets via
  `--preset`). Adding a new preset means dropping a new TOML in
  `config/presets/`, embedding via `include_str!` in `src/cli.rs`, and
  extending the `Preset` enum. Compiled-in fallback always points at
  `balanced` (not `scoring.toml`), so a binary-only install never
  inherits the project owner's calibration bias.
- **Mojibake fix at parse boundaries.** IG's exporter ships display
  names as UTF-8 bytes mis-read as Latin-1 (the `HÃ¼seyin` /
  `ÃÂÃÂ` bug). `src/text.rs::fix_mojibake` is the only repair site;
  every display-string capture in `src/export.rs` (DM participants,
  sender_name, reaction actor, content, title, Me.name) AND the NameResolver
  build-side in `src/features/name_resolution.rs` must apply it
  consistently — drop the fix from one side and the cross-side join
  silently breaks. The wire-through test in `name_resolution::tests`
  pins this; don't relax it.
- **Decision-hint SSOT.** The one-line account-shape characterization
  surfaced by both Markdown and HTML writers lives in
  `src/output/mod.rs::decision_hint`. The 13-row precedence-chain test
  is the contract; both writers call the shared function. Adding new
  rules: insert at the right precedence, extend the table-driven test.
- **Keep-allowlist / drop-list are mirror overrides.** Two per-user
  handle lists bracket the score: `config/keep_allowlist.txt` floors
  `Unfollow → Review`, `config/drop_list.txt` forces `→ Unfollow`. Both
  load through the shared `allowlist::parse` (case-insensitive
  `HashSet<String>`), surface as `is_keep_allowlisted` / `is_drop_listed`
  on `AccountFeatures`, and gate in `scoring::assign_bucket`. Precedence
  (top wins): `is_restricted` (Review floor) → `is_drop_listed` (Unfollow)
  → `keep_min` → keep-gates. `is_restricted` is the one floor the
  drop-list yields to. A handle on **both** lists is a contradiction —
  `allowlist::ensure_disjoint` rejects it loudly at load (in `run`),
  before scoring, so the two rungs never compete by construction. When
  adding a new override, mirror this end to end (loader → `Classifier`
  field + lookup → `AccountFeatures` field → `assign_bucket` rung →
  `decision_hint` row) and the ~7 test struct-builders.
- **Archive cache fingerprint, not mtime.** `archive::resolve` writes
  `{count}\n{total_bytes}\n` into `.complete` and invalidates on any
  mismatch. mtime-based checks are vulnerable to `cp -p` and to
  `rsync --times` replacing content without bumping mtimes — never
  roll back to a mtime-only check.
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
