# Instagram Following Sift (`igsift`)

[![CI](https://github.com/gati3478/igsift/actions/workflows/ci.yml/badge.svg)](https://github.com/gati3478/igsift/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Decide who to unfollow on Instagram — from your own data, fully offline.**

`igsift` reads your Instagram data export, scores every account you follow by a
`keep_probability`, and sorts them into **keep / review / unfollow**. It writes a
CSV, a Markdown summary, and a self-contained HTML report you can sort and filter
in your browser. No login, no API, no automation — you make the actual unfollows
by hand.

```
┌──────────────────────────┐     ┌─────────────────┐     ┌────────────────────┐
│ IG personal data export  │ ──▶ │  CLI: igsift    │ ──▶ │ following-audit.*  │
│ dir / .zip / multipart   │     │  score + rank   │     │  CSV · MD · HTML   │
└──────────────────────────┘     └─────────────────┘     └────────────────────┘
```

## Quickstart

### 1. Get your data

In Instagram, request a **Download Your Information** export in **JSON** format
and download the `.zip`(s) once they're ready.

### 2. Get `igsift`

Pick whichever fits — a prebuilt binary if you just want to run it, or a build
from source if you'd rather compile it yourself.

**Option A — Download a release (fastest).** Grab the binary for your platform
from the [**Releases**](https://github.com/gati3478/igsift/releases) page — it's
`igsift` on macOS/Linux and `igsift.exe` on Windows — then clear the
"downloaded from the internet" guard so the OS will run it:

```bash
# macOS / Linux
chmod +x igsift                                   # mark it executable
xattr -d com.apple.quarantine igsift 2>/dev/null || true   # macOS only
```

```powershell
# Windows (PowerShell)
Unblock-File .\igsift.exe                          # clear the SmartScreen block
```

**Option B — Build from source (any OS).** Needs a stable Rust toolchain —
install it via [**rustup**](https://www.rust-lang.org/tools/install) (the official
installer) if you don't have one:

```bash
cargo build --release    # binary lands at target/release/igsift (igsift.exe on Windows)
```

### 3. Run it

Point it at the export — a folder, a single `.zip`, or the folder of multipart
`.zip` parts Instagram ships for large accounts:

```bash
./igsift ./instagram-export                       # macOS / Linux downloaded binary
cargo run -- ./instagram-export                   # from a source checkout (any OS)
```

```powershell
.\igsift.exe .\instagram-export                    # Windows (PowerShell)
```

### 4. Read the report

Three files appear next to your input as `following-audit_<date>.{csv,md,html}`.
Open the **HTML** in a browser — a sortable, filterable table — then do the
unfollows by hand in Instagram.

No config files are required; `igsift` ships with sensible defaults.

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
igsift <input>          # score + write the audit (default; `run` is the explicit form)
igsift check <input>    # dry-run: parse every source (✓/✗) and sanity-check your config
igsift init             # scaffold the optional config files (see Customizing)
```

**Options**

- `--out <PATH>` — output stem (default: `following-audit_<date>` next to the input)
- `--preset <name>` — `balanced` | `engagement` | `tenure` (mutually exclusive with `--config`)
- `--config <PATH>` — use your own scoring-weights TOML instead of a preset
- `--rebuild-cache` — re-extract the archive even if a cache exists
- `--trace <handle>` — print the full per-signal score breakdown for one account
- `-v` / `-vv` — more logging (also hides the progress spinner)

## Customizing the results

Run `igsift init` to scaffold three optional files under `config/`:

- **`keeplist.txt`** — accounts you'll **never** unfollow (floored to _review_ at worst).
- **`droplist.txt`** — accounts to **always** force into _unfollow_, whatever the score (the exact mirror of the keeplist). A handle can't be on both lists.
- **`labels.txt`** — hand-label 20–30 accounts as keep/drop; `igsift` reports how well its scores agree with you after each run.

To tune the scoring weights yourself, copy a preset to `config/scoring.toml` and
edit it — see [`docs/TUNING.md`](docs/TUNING.md).

## How it works

For each account you follow, `igsift` aggregates the signals in your export — DMs,
likes, comments, story interactions, how long you've followed, whether they
follow you back — into a `keep_probability`, then buckets it into keep / review /
unfollow. A few hard rules override the score: _restricted_ accounts never drop
below review, keeplisted accounts are never unfollowed, and droplisted accounts
are always unfollowed. Display names mangled by Instagram's exporter are repaired
on the way in.

Score-vs-intent agreement is **feature-ceilinged** — the export simply doesn't
separate every keep from every drop, which is what the keeplist and droplist
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
