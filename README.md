# Instagram Manager (`ig-mgr`)

[![CI](https://github.com/gati3478/ig-manager/actions/workflows/ci.yml/badge.svg)](https://github.com/gati3478/ig-manager/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Decide who to unfollow on Instagram — from your own data, fully offline.**

`ig-mgr` reads your Instagram data export, scores every account you follow by a
`keep_probability`, and sorts them into **keep / review / unfollow**. It writes a
CSV, a Markdown summary, and a self-contained HTML report you can sort and filter
in your browser. No login, no API, no automation — you make the actual unfollows
by hand.

```
┌──────────────────────────┐     ┌─────────────────┐     ┌────────────────────┐
│ IG personal data export  │ ──▶ │  CLI: ig-mgr    │ ──▶ │ following-audit.*  │
│ dir / .zip / multipart   │     │  score + rank   │     │  CSV · MD · HTML   │
└──────────────────────────┘     └─────────────────┘     └────────────────────┘
```

## Quickstart

1. **Get your data.** In Instagram, request a **Download Your Information**
   export in **JSON** format and download the `.zip`(s) once they're ready.
2. **Build the tool.** Needs a stable Rust toolchain.
    ```bash
    cargo build --release          # binary lands at target/release/ig-mgr
    ```
3. **Run it** against the export — a folder, a single `.zip`, or the folder of
   multipart `.zip` parts Instagram ships for large accounts:
    ```bash
    ig-mgr ./instagram-export       # or, without installing: cargo run -- ./instagram-export
    ```
4. **Read the report.** Three files appear next to your input as
   `following-audit_<date>.{csv,md,html}`. Open the **HTML** in a browser — a
   sortable, filterable table — then do the unfollows by hand in Instagram.

No config files are required; `ig-mgr` ships with sensible defaults.

## Usage

**Input** — an already-extracted directory, a single `.zip`, or a directory of
the multipart `*.zip` parts. Archives are extracted and cached next to the input;
`--rebuild-cache` forces a fresh extract.

**Scoring presets** — pick the lens that matches how you decide (`--preset`):

| Preset                 | Keeps the accounts that…                                             |
| ---------------------- | -------------------------------------------------------------------- |
| `balanced` _(default)_ | …score well across all signals — no single one dominates             |
| `engagement`           | …you actually talk to and interact with; drops dormant follows       |
| `tenure`               | …you've followed for a long time, even if interaction has tailed off |

**Subcommands**

```bash
ig-mgr <input>          # score + write the audit (default; `run` is the explicit form)
ig-mgr check <input>    # dry-run: parse every source (✓/✗) and sanity-check your config
ig-mgr init             # scaffold the optional config files (see Customizing)
```

**Options**

- `--out <PATH>` — output stem (default: `following-audit_<date>` next to the input)
- `--preset <name>` — `balanced` | `engagement` | `tenure` (mutually exclusive with `--config`)
- `--config <PATH>` — use your own scoring-weights TOML instead of a preset
- `--rebuild-cache` — re-extract the archive even if a cache exists
- `--trace <handle>` — print the full per-signal score breakdown for one account
- `-v` / `-vv` — more logging (also hides the progress spinner)

## Customizing the results

Run `ig-mgr init` to scaffold three optional files under `config/`:

- **`keep_allowlist.txt`** — accounts you'll **never** unfollow (floored to _review_ at worst).
- **`drop_list.txt`** — accounts to **always** force into _unfollow_, whatever the score (the exact mirror of the allowlist). A handle can't be on both lists.
- **`labels.txt`** — hand-label 20–30 accounts as keep/drop; `ig-mgr` reports how well its scores agree with you after each run.

To tune the scoring weights yourself, copy a preset to `config/scoring.toml` and
edit it — see [`docs/TUNING.md`](docs/TUNING.md).

## How it works

For each account you follow, `ig-mgr` aggregates the signals in your export — DMs,
likes, comments, story interactions, how long you've followed, whether they
follow you back — into a `keep_probability`, then buckets it into keep / review /
unfollow. A few hard rules override the score: _restricted_ accounts never drop
below review, allowlisted accounts are never unfollowed, and drop-listed accounts
are always unfollowed. Display names mangled by Instagram's exporter are repaired
on the way in.

Score-vs-intent agreement is **feature-ceilinged** — the export simply doesn't
separate every keep from every drop, which is what the allowlist and drop-list
are for, not a bug to tune away. The algorithm is in
[`docs/DESIGN.md`](docs/DESIGN.md); the tuning journal and current measured
results are in [`docs/TUNING.md`](docs/TUNING.md).

## Development

```bash
cargo build --all-targets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo nextest run                          # or: cargo test
cargo deny check advisories bans sources
```

Local [Lefthook](https://github.com/evilmartians/lefthook) hooks mirror these on
commit/push; CI runs them as the authoritative gate. See
[`CONTRIBUTING.md`](CONTRIBUTING.md) to contribute and
[`SECURITY.md`](SECURITY.md) to report a privacy/security issue.

## Tech stack

Rust (edition 2024) — one static binary, no async, network, or database.
`clap`, `serde` + `serde_path_to_error` (drift-tolerant parsing), `jiff`,
`aho-corasick`, `zip`, `indicatif`, `csv`, `tracing`, `anyhow`. The HTML report
is hand-rolled markup — no template engine. Full rationale and the
deliberately-not-used list are in [`docs/DESIGN.md`](docs/DESIGN.md).

## Non-goals

No web/swipe UI, no Instagram API / scraping / automated unfollow, no daemon, no
database, no login. The export is the source of truth; the run is one-shot.

## License

[MIT](LICENSE)
