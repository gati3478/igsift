//! Scoring configuration, loaded from a TOML file.
//!
//! Keeping weights and decay constants in TOML lets them be tuned without a
//! rebuild — the main ergonomic gap Rust has versus a notebook workflow.
//!
//! Path resolution (plan — the installed binary may live in `~/bin`, so a
//! CWD-relative default is wrong):
//!
//! 1. `--config <PATH>` if given — explicit override, always wins.
//! 2. else `./config/scoring.toml` if it exists — dev-tree convenience.
//! 3. else the platform config dir (e.g. `~/.config/ig-mgr/scoring.toml`).
//! 4. else a default compiled into the binary via `include_str!`, so a fresh
//!    install runs zero-config with the documented starting weights.
//!
//! Planned surface (implement when scoring lands):
//!
//! - `ScoringConfig` — deserialized via `serde` from the TOML file.
//! - feature weights (`dm`, `likes`, `comments`, `story_out`, `story_in`, …).
//! - decay constants (`tau_dm_days`, `tau_content_days`).
//! - bucket thresholds (`keep >= 0.7`, `unfollow < 0.3`).
//!
//! Status: scaffold only.
