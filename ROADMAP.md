# Roadmap — `ig-mgr`

Implementation order. Each parser/feature step is verified against a real
export before moving on — see [`docs/DESIGN.md`](docs/DESIGN.md) for the design
behind each item.

- [x] **Download a fresh export** (2026-05-17) — pulled; nothing inspected or
      parsed yet, schema notes in DESIGN are presumed stale until re-verified.
- [x] **Scaffold the repository** (2026-05-20) — Rust CLI skeleton: module
      boundaries, dependencies, config files, CI, and smoke tests. No pipeline
      logic.
- [ ] **Re-validate the file layout and JSON keys** against the fresh export —
      every parser path is suspect until confirmed.
- [ ] **Minimal parser** for followers + following + DM threads; confirm the
      following set and DM volumes match reality.
- [ ] **Remaining feature extractors**, one at a time, verifying counts against
      spot-checks (likes, comments, stories, tags, saved, searches).
- [ ] **First-pass scoring** with hand-set weights; eyeball top/bottom 50.
- [ ] **Tune weights and decay constants** — consider a small labeled set of
      ~30 accounts I already know I want to keep/drop, fit weights to match.
- [ ] **Brand / public-figure heuristic** + user-maintained allowlist.
- [ ] **CSV + Markdown output writers.**
- [ ] **Run on the real export**, do the cleanup, evaluate regret a few weeks
      later, iterate.
