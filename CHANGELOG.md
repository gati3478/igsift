# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Initial development; not yet tagged. `ig-mgr` reads an Instagram personal-data
export and writes a CSV + Markdown + HTML audit ranking who to unfollow vs. keep,
with a `keep_probability` per account. Fully offline — no network, database, or
automated unfollow.

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
- Three subcommands (`run`, `init`, `check`), three scoring presets
  (`balanced`/`engagement`/`tenure`), per-term `--trace`, and an optional
  labeled-set confusion-matrix report.
- CSV, decision-oriented Markdown, and self-contained HTML report writers, with
  XSS / CSV-formula-injection escaping.
