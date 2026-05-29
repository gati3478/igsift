# Instagram Manager (`ig-mgr`)

Local-first CLI that reads an Instagram personal data export and produces a
ranked audit — who to unfollow (and remove from followers) versus who to keep,
with a `keep_probability` per account derived from the full breadth of
exported interactions. No UI, no API automation, no network — I act on the
output manually inside Instagram.

```
┌──────────────────────────┐     ┌─────────────────┐     ┌────────────────────┐
│ IG personal data export  │ ──▶ │  CLI: ig-mgr    │ ──▶ │ following-audit.*  │
│ dir / .zip / multipart   │     │  score + rank   │     │  CSV · MD · HTML   │
└──────────────────────────┘     └─────────────────┘     └────────────────────┘
```

One invocation, three artifacts: CSV for spreadsheet triage, Markdown for
skim-review, HTML for browser-based filterable triage. I review the output
and do the unfollows by hand.

## Status

**Active — end-to-end pipeline landed, six refinement phases shipped.** The
binary accepts a directory or a `.zip` (single or multipart, transparently
extracted and cached), runs a progress-bar pipeline through parser →
feature aggregation (raw + decay-weighted + 90d/180d windowed counts +
mutual-follower flag) → scoring (`keep_prob` + `keep`/`review`/`unfollow`
bucket gated by the restricted floor, the brand/keep-allowlist gates, and
the drop-list force-Unfollow override) → three writers (CSV,
decision-oriented Markdown with per-bucket cards, self-contained HTML with
sortable+filterable tables). Three subcommands (`run`, `init`, `check`),
three shipped scoring presets (`balanced`/`engagement`/`tenure`), and an
optional held-out labeled-set confusion-matrix report (`config/labels.txt`)
that quantifies agreement after every run.

Display names are mojibake-repaired at parse time (IG's exporter ships
UTF-8 bytes as Latin-1, so `HÃ¼seyin` becomes `Hüseyin` and Arabic /
Georgian / emoji surface correctly). Bucket split on the real 649-account
export was `485 / 154 / 10` at 28.6% labeled-set agreement, 0 hard
mismatches — **now stale**: measured before `story_likes.json` was folded
into `story_interactions_out`, so it needs a re-run and re-tune. See
[`ROADMAP.md`](ROADMAP.md), [`docs/DESIGN.md`](docs/DESIGN.md)
for the algorithm, and [`docs/TUNING.md`](docs/TUNING.md) for the
weight-tuning journal.

> A previous SvelteKit web-app prototype (card-deck review UI, SQLite/Drizzle)
> was retired — the interactive direction is friction I don't need for a
> one-shot periodic cleanup. This repo is a clean restart as a Rust CLI.

## Build & run

```bash
cargo build --release                                # binary at target/release/ig-mgr
cargo run -- /path/to/export                         # extracted folder
cargo run -- /path/to/export.zip                     # single .zip (auto-extract + cache)
cargo run -- /path/to/parts/                         # directory of multipart .zip parts
cargo run -- /path/to/export --out ~/cleanup -v
```

Three input shapes are accepted transparently: an already-extracted
directory, a single `.zip` file, or a directory of multipart `*.zip`
parts that IG ships for large exports. Archives extract to
`.ig-mgr-extracted*/` next to the input and are cached across re-runs
(cache fingerprint is `{count}\n{total_bytes}`, so `cp -p` /
`rsync --times` replacements don't slip past as "fresh"). Use
`--rebuild-cache` to force a fresh extract.

### Subcommands

```bash
ig-mgr <input>                       # implicit Run (legacy form)
ig-mgr run <input>                   # explicit form of the above
ig-mgr check <input>                 # parser-only dry-run, per-source ✓ / ✗
ig-mgr init [--force]                # scaffold config/{keep_allowlist,drop_list}.txt + labels.txt
```

`check` runs the same parser stack as `run` without aggregation /
scoring / writing, reporting each source individually. Fast pre-flight
when you're not sure a fresh export extracted cleanly.

### Options

- `--out <PATH>` — output stem; defaults to `following-audit_<DATE>.{csv,md,html}`
  next to the input.
- `--preset <NAME>` — pick a shipped scoring shape (`balanced`,
  `engagement`, `tenure`). Mutually exclusive with `--config`. See
  Quickstart below.
- `--config <PATH>` — scoring weights TOML; when omitted, resolved as
  `./config/scoring.toml` in the cwd, then the compiled-in fallback
  (= the `balanced` preset). A platform config dir
  (`~/.config/ig-mgr/`) is not yet wired.
- `--rebuild-cache` — force a fresh extract of an archive input.
- `--trace <HANDLE>` — print the full per-term scoring breakdown for one
  followee handle. Errors if the handle isn't in the followings set after
  blocked / recently-unfollowed exclusions. Use during tuning to answer
  "why did this account rank where it did?".
- `-v` / `-vv` — debug / trace log verbosity. Also disables the
  progress spinner so log lines don't interleave. `RUST_LOG` overrides
  when set.

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

1. `ig-mgr init` to scaffold `config/keep_allowlist.txt`,
   `config/drop_list.txt`, and `config/labels.txt`.
2. Append accounts you want to **never** unfollow to
   `config/keep_allowlist.txt`; append accounts you want **always**
   forced into Unfollow to `config/drop_list.txt` (the exact inverse —
   it overrides the score and every keep-signal). A handle on both
   lists is a contradiction and errors loudly at load. The drop-list is
   the escape hatch for accounts the score can't separate (a story-heavy
   follow you've decided to drop, say); a `restricted` account still
   stays in Review even if drop-listed.
3. Hand-label 20–30 followees in `config/labels.txt` (format in the
   template). The binary prints a confusion-matrix report against
   your labels at the end of every run.
4. Copy a preset to `config/scoring.toml` (e.g.
   `cp config/presets/engagement.toml config/scoring.toml`) and edit
   weights to chase higher label agreement. See
   [`docs/TUNING.md`](docs/TUNING.md) for the journal of how the
   committed `config/scoring.toml` was tuned against a 649-account
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
database. `clap` (CLI + subcommands) · `serde` / `serde_json` +
`serde_path_to_error` (schema-drift-survivable parsing) · `jiff` (time) ·
`rayon` (parallel scoring) · `aho-corasick` (brand-suffix lexicon,
single-pass automaton) · `zip` (archive extraction, pure-Rust, deflate-only)
· `indicatif` (progress spinner + bytes bar) · `csv` (output) · `tracing`
(logs) · `anyhow` + `thiserror` (errors). Tests: `insta` snapshots +
`assert_cmd` + `cargo-nextest`. The HTML report is hand-rolled markup —
no template engine. Rationale and the deliberately-not-used list are in
[`docs/DESIGN.md`](docs/DESIGN.md).

## Non-goals

- No web UI, card deck, or swipe interface.
- No Instagram API calls, scraping, or automated unfollow.
- No background daemon — one-shot run, exits when done.
- No persistent DB — the export is the source of truth; history = old output files.
- No login or credentials handling.

## License

[MIT](LICENSE)
