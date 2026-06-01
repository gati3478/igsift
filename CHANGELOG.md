# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Initial development. `v0.1.0` is published as a pre-release (prebuilt binaries on
the GitHub Releases page) for validation; the first full release will be `1.0.0`.
`igsift` reads an Instagram personal-data export and writes a CSV + Markdown +
HTML audit ranking who to unfollow vs. keep, with a `keep_probability` per
account. Fully offline — no network, database, or automated unfollow.

### Added

- Archive resolution: extracted directory, single `.zip`, or multipart `.zip`
  parts, with a fingerprint-based extraction cache.
- Schema-drift-survivable JSON parsers for the export (following/followers, DMs,
  likes, comments, story interactions, saves) with mojibake repair on display
  strings.
- Per-account feature aggregation (raw + decay-weighted + 90d/180d windows +
  mutual-follow flag) and a sigmoid `keep_probability` scorer with
  keep/review/unfollow bucketing.
- Brand/public-figure classifier, restricted-account floor, and mirror
  keeplist / droplist overrides.
- Relationship-aware bucket gates, all monotonic — they only refuse to
  auto-keep a stranger, refuse to drop a years-deep mutual, or floor a
  no-evidence account to review, never manufacturing an unfollow: a deep-mutual
  keep-floor; an opt-in reciprocity keep-ceiling; a non-reciprocal close-tie
  ceiling (an explicit close-friend/favorite marker the followee never returned
  with a follow-back — on by default, with a paired score penalty); a
  dead-mutual gate (a short-tenure follow-back with zero interaction in either
  direction); and an inert-account floor (a zero-signal account in the unfollow
  band is floored to review — an absence of evidence isn't evidence to drop,
  with `__deleted__` accounts exempt).
- DM-attribution fix for accounts that never set an Instagram display name:
  Instagram emits the handle itself as the DM `sender_name` for those, so an
  identity `handle → handle` mapping recovers whole threads the display-name
  resolver previously dropped, correcting their `dm_*` features.
- Effort-skew gate: a monotonic two-tier `Keep → Review` demotion for accounts
  the owner over-invests in (high outbound DM volume, near-zero real replies),
  evidence-guarded on owner DM volume and disabled by default in the presets.
  The SOFT tier demotes unmarked personal keeps; the HARD tier overrides
  close-friend / favorite / mutual markers. Paired with a parser fix:
  Instagram serializes a message-like twice (in `reactions[]` and as a
  standalone "Liked a message" message), so the aggregator now excludes the
  shadow from DM volume / balance — one-sidedness is no longer masked — and
  tracks `dm_inbound_replies` (real replies, not taps).
- Three subcommands (`run`, `init`, `check`), three scoring presets
  (`balanced`/`engagement`/`tenure`), per-term `--trace`, and an optional
  labeled-set confusion-matrix report.
- Three audit report writers, with XSS / CSV-formula-injection escaping:
    - **CSV** — one row per account (`keep_score` + `top_signal` columns, kept
      as raw values for spreadsheet math); the header is a pinned inter-run
      diff contract.
    - **Markdown** — a decision-oriented skim: score rendered as `keep NN%`, a
      Summary proportion bar that sums to exactly 100%, the redundant one-sided
      hint suppressed, droplist-forced rows quarantined under their own
      subheading, and the Review bucket split into **Faded** (full cards) vs.
      **Inert — never engaged** (compact skim table) subsections.
    - **HTML** — a self-contained, sortable/filterable browser report
      ("Keep likelihood" as a percentage with a bucket-keyed bar), with
      **in-report triage**: per-row Keep / Drop toggles (mutually exclusive,
      persisted in `localStorage`) and a floating export bar — color-keyed
      segmented controls (green = keeplist, red = droplist) — that Copies or
      Downloads the appendable handle lists to paste into
      `config/keeplist.txt` / `config/droplist.txt`. Fully local; nothing
      leaves the browser. In the Review bucket, never-engaged ("inert") rows
      carry a muted pill and can be collapsed with a **Hide never-engaged**
      filter. Follows the OS light/dark setting with a header
      **Auto / Light / Dark** switcher (ARIA radiogroup, persisted in
      `localStorage`, anti-FOUC) to override it.
- Polished terminal run-summary dashboard: boxed header banner, colored bucket
  panel with proportional bars, keep_prob histogram, side-by-side keep/unfollow
  cards, and a colored accuracy block. Pipe-safe — emits no ANSI when stdout is
  not a TTY — with an ASCII fallback for non-UTF-8 terminals and
  width-responsive panels. A `--color auto|always|never` flag (default `auto`,
  honoring `NO_COLOR`) controls colorization. Layout is Unicode-display-width
  correct (CJK/wide chars count as two columns, combining/zero-width as zero),
  so boxes and columns stay aligned for any content — including a non-ASCII
  `--config` path in the header; control characters in such paths are
  sanitized. UTF-8 vs. ASCII rendering honors POSIX locale precedence
  (`LC_ALL` › `LC_CTYPE` › `LANG`).

### Dependencies

- Upgraded `zip` 7 → 8 (`deflate-flate2-zlib-rs` backend, read-only deflate),
  which drops the unused `zopfli` encoder for a net-smaller dependency graph.
  This raises the minimum supported Rust to **1.88** (`zip` 8's MSRV); the
  build toolchain stays on the latest stable. Verified: the full suite builds
  and passes on Rust 1.88.

### Release & CI

- Cross-platform release workflow (`.github/workflows/release.yml`): pushing a
  `v*` tag builds and attaches prebuilt binaries (each with a SHA-256 checksum,
  README, and LICENSE) to a GitHub Release for five targets — macOS arm64,
  Windows x64/arm64, and Linux x64/arm64 (statically linked against musl, so the
  Linux builds run on Fedora and any distro regardless of the builder's libc).
- Release creation runs as a dedicated up-front job so the parallel build matrix
  uploads to a pre-existing Release instead of racing to create it; `v0.x` tags
  are published as pre-releases and `v1.0.0`+ as full releases.
- CI now runs a native Windows runtime smoke-test (`windows-smoke`) as a matrix
  over x64 (`windows-latest`) and arm64 (`windows-11-arm`): it builds `igsift`
  and exercises `--version` + `check` + a full audit against the fixture,
  asserting all three output files are produced. The unit/integration suite
  still runs on Linux; this closes the gap where the Windows binaries were only
  cross-built, never executed.
