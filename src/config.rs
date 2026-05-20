//! Scoring configuration, loaded from `config/scoring.toml`.
//!
//! Keeping weights and decay constants in TOML lets them be tuned without a
//! rebuild — the main ergonomic gap Rust has versus a notebook workflow.
//!
//! Planned surface (implement when scoring lands):
//!
//! - `ScoringConfig` — deserialized via `serde` from the TOML file.
//! - feature weights (`dm`, `likes`, `comments`, `story_out`, `story_in`, …).
//! - decay constants (`tau_dm_days`, `tau_content_days`).
//! - bucket thresholds (`keep >= 0.7`, `unfollow < 0.3`).
//!
//! Status: scaffold only.
