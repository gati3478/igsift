# CLAUDE.md

## Project overview

`igsift` — a local-first Rust **CLI** that reads an Instagram personal data
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

**Three subcommands** (the bare `igsift <export>` form is the default,
equivalent to `igsift run <export>`):
`run` (score + write audit), `init` (scaffold per-user config files from
embedded templates), `check` (parser dry-run with per-source pass/fail,
plus a config sanity check that the keeplist + droplist parse and
are disjoint).

**Scoring & calibration.** `keep_prob` is a sigmoid over weighted signals;
weights/decay/thresholds live in `config/scoring.toml`. Three presets
(`balanced` / `engagement` / `tenure`) ship embedded via `--preset`; `balanced`
is the default preset and the compiled-in fallback when neither
`--preset`/`--config` nor a cwd `config/scoring.toml` resolves — so a
binary-only install always gets a known preset, never the owner's personal
`config/scoring.toml`. Label agreement is **feature-ceilinged, not a tuning
bug** — keep/drop intent is largely _non-separable_ on the current features (DM
is the only clean keep signal; `story_out` is a coin flip; many keep-labels are
low-engagement follows only `tenure` carries), so chasing it with weights
trades keep-recall for drop-precision ~1:1. The escape hatch is the **droplist**
(`config/droplist.txt`, the mirror of `keeplist`): a hand-flagged handle
is forced to `Unfollow` regardless of score. The current measured bucket split
and label agreement live in [`docs/TUNING.md`](docs/TUNING.md) — the SSOT for
tuning results; don't restate the numbers here.

## Tech stack

- **Language**: Rust, edition 2024, stable toolchain (`rust-toolchain.toml` pins it).
- **Package shape**: single package, two targets — library crate `igsift`
  (`src/lib.rs`) holds the logic; binary `igsift` (`src/main.rs`) is a thin
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
breakdown), `--rebuild-cache` (force archive re-extract), `--color
auto|always|never` (summary colorization; `auto` = TTY-only, honors `NO_COLOR`,
pipe-safe), `-v` / `-vv` (debug / trace verbosity; `RUST_LOG` overrides).

`<input>` is a directory (extracted or containing `*.zip` parts) or a
single `.zip` — `archive::resolve` detects and extracts transparently
into `.igsift-extracted*/` next to the input, with cache reuse keyed on
a `{count}\n{total_bytes}` fingerprint (mtime-immune to `cp -p` and
`rsync --times`).

Set up once per clone: `brew install lefthook && lefthook install` (pre-commit fmt + pre-push clippy/test/cargo-deny mirror CI — see `lefthook.yml`). Optional CI tools: `cargo install --locked cargo-nextest cargo-deny` (both install with `--locked`).

## Layout

```
src/
  main.rs                       # binary entry: dispatch on subcommand (run / init / check)
  lib.rs                        # run() / init() / check() orchestration; init_tracing(); re-exports
  cli.rs                        # clap derive: Cli, RunArgs, Command (Run/Init/Check), Preset enum
  archive.rs                    # input detect + zip extract + cache (.igsift-extracted*/)
  config.rs                     # scoring.toml deserialization; preset resolution chain
  export.rs                     # IG export JSON parsers + validate_shape pre-flight
  lists.rs                      # config/{keeplist,droplist}.txt loaders + ensure_disjoint
  labels.rs                     # config/labels.txt loader + confusion-matrix report (caps-aware render)
  term_style.rs                 # terminal vocabulary: Caps detection (TTY/NO_COLOR/locale/width) + palette/glyphs + bar/boxed renderers (pipe-safe paint chokepoint)
  summary.rs                    # run-summary dashboard: header banner + bucket panel + histogram + keep/unfollow cards
  progress.rs                   # indicatif spinner wrapper (auto-hide on -v or off-TTY)
  text.rs                       # fix_mojibake — repairs IG's UTF-8-as-Latin-1 export bug
  features/
    mod.rs                      # re-exports
    aggregate.rs                # per-account features: raw + decayed + windowed + is_mutual + dm_inbound_replies (post-shadow-dedup)
    name_resolution.rs          # display_name → handle bridge for DM attribution
    account_class.rs            # brand-detection (aho-corasick lexicon) + keeplist / droplist gates
  scoring.rs                    # score_raw composition, sigmoid, bucket assignment (incl. effort-skew gate), top_terms
  output/
    mod.rs                      # write() dispatcher (CSV+MD+HTML) + shared writer SSOT (decision_hint, HINT_ONE_SIDED, contributions_inline)
    csv.rs                      # CSV row writer (DESIGN.md "Output" header is the contract: keep_score, top_signal, reply_skew)
    markdown.rs                 # decision-oriented MD: keep-% cards + proportion-bar summary + droplist quarantine
    html.rs                     # self-contained HTML report (inline CSS+JS, no deps) + per-row keep/drop triage → localStorage → copy/paste to lists
tests/
  cli.rs                        # binary integration tests + fixture-count assertions (locked-in)
  fixtures/sample_export/       # sanitized synthetic export
examples/
  showcase.rs                   # synthetic-data generator for the README screenshots (real scorer + writers)
config/
  scoring.toml                 # Gati's tuned weights + decay constants
  keeplist.txt.example         # per-user keeplist template (real .txt gitignored)
  droplist.txt.example         # per-user droplist template — forces Unfollow (real .txt gitignored)
  labels.txt.example           # per-user labels template (real .txt gitignored)
  presets/
    balanced.toml              # default preset; compiled-in fallback when no config/preset resolves
    engagement.toml            # engagement-weighted preset
    tenure.toml                # tenure-weighted preset
scripts/
  walk_export_schema.sh        # JSON-schema walker; drift detector for IG exports
  showcase-shots.sh            # regenerate docs/assets/ README screenshots (example → Chromium shots)
.github/workflows/
  ci.yml                       # fmt/clippy/test/cargo-deny + Windows x64/arm64 runtime smoke-test
  release.yml                  # on `v*` tag: cross-compile 5 targets → GitHub Release
docs/DESIGN.md  docs/TUNING.md  docs/GOING-PUBLIC.md  docs/specs/  docs/assets/  ROADMAP.md
```

(`docs/specs/` holds the dated design specs: the reciprocity and effort-skew
gates are linked from Conventions; the terminal run-summary dashboard is
[`docs/specs/2026-05-31-terminal-summary-dashboard-design.md`](docs/specs/2026-05-31-terminal-summary-dashboard-design.md).
`docs/assets/` holds the README screenshots; `docs/plans/` holds transient
implementation plans.)

## Conventions

- **Privacy first.** A real export contains personal data; never commit one.
  Exports live in `ig-exported-data/` (gitignored); the binary's positional
  path can point anywhere — inside or outside the repo. `.gitignore` is the
  safety net but a fragile one: only the names it lists are matched, so an
  export dropped at any other name **will** be tracked. Test
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
  `src/output/mod.rs::decision_hint`. The 18-row precedence-chain test
  is the contract; both writers call the shared function. Adding new
  rules: insert at the right precedence, extend the table-driven test.
  The one-sided hint string is hoisted to a `HINT_ONE_SIDED` const in
  the same module: the Markdown writer suppresses _that specific_ hint
  when it would only restate the `one-sided` attribute badge already on
  the card, so the two sites compare against the const, never a
  copy-pasted literal. HTML does not suppress (no attribute-line
  redundancy there). The `keep_prob → "NN%"` percentage is a
  human-report-only rendering (Markdown + HTML); the CSV keeps the raw
  `0.0–1.0` float — `pct()` currently lives in both writer modules.
- **Keeplist / droplist are mirror overrides.** Two per-user
  handle lists bracket the score: `config/keeplist.txt` floors
  `Unfollow → Review`, `config/droplist.txt` forces `→ Unfollow`. Both
  load through the shared `lists::parse` (case-insensitive
  `HashSet<String>`), surface as `is_keeplisted` / `is_droplisted`
  on `AccountFeatures`, and gate in `scoring::assign_bucket`. Precedence
  (top wins): `is_restricted` (Review floor) → `is_droplisted` (Unfollow)
  → effort-skew HARD (Review) → deep-mutual floor (Keep) → `keep_min`
  (+ effort-skew SOFT / non-reciprocal close-tie / reciprocity gate → Review) → keep-gates.
  `is_restricted` is the one floor the droplist yields to. A handle on
  **both** lists is a contradiction — `lists::ensure_disjoint` rejects
  it loudly at load (in `run`), before scoring, so the two rungs never
  compete by construction. When adding a new override, mirror this end to
  end (loader → `Classifier` field + lookup → `AccountFeatures` field →
  `assign_bucket` rung → `decision_hint` row) and the ~7 test
  struct-builders.
- **Relationship gates are monotonic (keep = relationship, not
  consumption).** Monotonic config-gated rungs in `assign_bucket` bracket the
  score, each moving an account in **one** direction only so none can
  manufacture a wrongful `unfollow`: the **deep-mutual keep-floor**
  (`scoring.deep_mutual_keep_days`, default 730 — mutual + `mutual_age_days`
  ≥ threshold floors to Keep; `0` disables) and the **reciprocity
  keep-ceiling** (`scoring.require_reciprocity_for_keep`, default **false**,
  opt-in — when on, a personal, non-mutual account with no inbound signal
  can't auto-keep on one-way consumption, demoted Keep → Review).
  `mutual_age_days` (days since the _later_ of your-follow / their-follow-back,
  from `followers_*.json` timestamps) is computed in `features::aggregate`.
  The ceiling defaults off across all presets + the `serde` default — the only
  labeled pass (TUNING round 7) measured it as harmful for a content-consumer
  following style; it's preserved as a toggle for mutual-heavy users.
  The **non-reciprocal close-tie ceiling** (`scoring.demote_nonmutual_close_ties`,
  default **true** — the mirror-inverse of the reciprocity ceiling) demotes a
  personal, non-mutual account the owner marked close-friend/favorited (and not
  keeplisted) Keep → Review: an explicit marker the followee never reciprocated.
  Unlike the other gates it ships **on** in every preset (high-precision +
  Review-only, never `unfollow`). A paired always-on penalty weight
  (`nonmutual_close_tie_penalty`) erodes `score_raw` so the report reflects the
  flag; the predicate `is_nonreciprocal_close_tie` is the shared SSOT for the
  penalty term + the gate rung (TUNING round 10). See
  [`docs/specs/2026-06-01-nonmutual-close-tie-gate-design.md`](docs/specs/2026-06-01-nonmutual-close-tie-gate-design.md).
  The **effort-skew gate** (`scoring.effort_skew_min_dm_out` / `_soft` /
  `_hard`; `min_dm_out = 0` disables) adds two more monotonic rungs — a SOFT
  tier (demotes an unmarked personal Keep) and a HARD tier (overrides
  close-friend / favorite / mutual + the deep-mutual floor) — both Keep →
  Review only and **evidence-guarded** on owner DM volume, so they act only
  inside a 1:1 thread the owner invested in; off by default in the presets.
  See [`docs/specs/2026-05-31-effort-skew-gate-design.md`](docs/specs/2026-05-31-effort-skew-gate-design.md).
  These gates are deliberately
  **gates not weights** — their correctness doesn't depend on the noisy
  `labels.txt` oracle. Full rationale (deep-mutual + reciprocity):
  [`docs/specs/2026-05-30-reciprocity-aware-scoring.md`](docs/specs/2026-05-30-reciprocity-aware-scoring.md).
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
  entries. The `igsift()` test helper spawns the binary with
  `cwd = std::env::temp_dir()` so the cwd-relative `config/*` loaders miss
  any per-user files at the repo root — without this, a developer with
  their own `config/labels.txt` or `config/keeplist.txt` sees
  fixture counts contaminate. Don't undo the cwd override.
- `unsafe` is forbidden (`[lints.rust] unsafe_code = "forbid"`).

## Non-goals

No web UI / TUI, no Instagram API / scraping / automated unfollow, no daemon,
no DB, no login, no async or network crates — the export is the source of truth
and the run is one-shot. A `ratatui review` subcommand is a possible v2, not v1.
Full rationale and the deliberately-not-used crate list:
[`docs/DESIGN.md`](docs/DESIGN.md) ("Deliberately not using").
