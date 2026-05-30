# Contributing

Thanks for your interest. This is a small, single-maintainer Rust CLI; PRs and
issues are welcome, but please keep changes focused and discuss larger ones in
an issue first.

## Setup

```bash
# Local git hooks mirror CI (fmt on commit; clippy + tests + cargo-deny on push)
brew install lefthook && lefthook install   # or see https://github.com/evilmartians/lefthook

# Tools CI uses (the pre-push hook expects cargo-deny):
cargo install --locked cargo-nextest
cargo install --locked cargo-deny
```

## Before you push

The git hooks run these automatically (`fmt` on commit; `clippy`, tests, and
`cargo deny` on push). You can also run them by hand — this is the **full set
CI runs**, so if they pass locally, CI passes:

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo build --all-targets
cargo nextest run                          # or: cargo test
cargo deny check advisories bans sources
```

## Conventions worth knowing

The full project conventions live in [`CLAUDE.md`](CLAUDE.md) (it doubles as the
guide for AI coding agents). The load-bearing ones:

- **Privacy first.** A real Instagram export is personal data and must **never**
  be committed. Exports are gitignored; report output never lives in the repo.
  Test fixtures must be sanitized synthetic data only — see
  [`tests/fixtures/README.md`](tests/fixtures/README.md).
- **Fixture counts are locked in.** `tests/cli.rs` asserts exact integer counts
  against the synthetic fixture. If a count drifts after a parser change, the
  parser dropped data — fix the parser, don't relax the assertion.
- **Tunables live in TOML**, not code (`config/scoring.toml` / the embedded
  presets), and the decision-hint and keep/drop-override logic each have a
  single source of truth with a precedence-chain test. Mirror the existing
  structure when extending them.
- `unsafe` is forbidden; schema-drift-survivable parsing (`#[serde(default)]` +
  `Option<T>` + `serde_path_to_error`) is deliberate — see `CLAUDE.md`.

## Licensing

By contributing you agree your contributions are licensed under the repository's
[MIT License](LICENSE).
