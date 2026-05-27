//! Scoring configuration, loaded from a TOML file.
//!
//! Keeping weights and decay constants in TOML lets them be tuned without a
//! rebuild — the main ergonomic gap Rust has versus a notebook workflow.
//!
//! Slice 7B-2 loads only the `[decay]` section that the aggregator's
//! exponential-decay weighting needs ([`DecayConfig`]). The scoring slice
//! will extend [`ScoringConfig`] with the `[weights]` and `[scoring]`
//! sections — serde's default ignore-unknown-fields posture means a
//! partial target type parses the full file harmlessly today and gains
//! fields without churning the read path.
//!
//! Path resolution:
//!
//! 1. `--config <PATH>` if given — explicit override, always wins.
//! 2. else `./config/scoring.toml` in the current working directory — the
//!    dev-tree default; matches `cargo run` from the repo root and
//!    `assert_cmd::Command::cargo_bin(...)` invocations from the integration
//!    tests.
//! 3. else the compiled-in copy of `config/scoring.toml` via [`include_str!`]
//!    — so a fresh `cargo install` runs zero-config with the documented
//!    starting weights even if the user never lays down a config file.
//!
//! A platform config dir (`~/.config/ig-mgr/scoring.toml`) is in the
//! comments of the slice 6 scaffold; it needs the `directories` /
//! `dirs` crate and is deferred until somebody actually installs the
//! binary outside the dev tree.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

/// Top-level scoring configuration. Slice 7B-2 surfaces only [`decay`];
/// the scoring slice will add `weights: WeightsConfig` and `scoring:
/// ScoringParams` here without changing the read path.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoringConfig {
    pub decay: DecayConfig,
}

/// Exponential-decay half-lives, in **days**.
///
/// A signal `tau` days old carries `1/e` (≈ 0.368) the weight of a
/// just-now signal. DESIGN.md sets the starting values at 180d for DM
/// signals and 365d for content interactions — older interactions
/// should fade, not disappear.
#[derive(Debug, Clone, Deserialize)]
pub struct DecayConfig {
    pub tau_dm_days: u32,
    pub tau_content_days: u32,
}

/// Read and parse the scoring config, following the documented resolution
/// chain in the module doc.
///
/// `--config <PATH>` from the CLI is a hard error if the file is missing
/// or malformed (user asked for this file specifically). A missing dev-tree
/// default falls through to the compiled-in copy — that's the "ran the
/// binary from outside the project tree" case, not a misconfiguration.
pub fn read_scoring_config(cli_config: Option<&Path>) -> Result<ScoringConfig> {
    let cfg = if let Some(path) = cli_config {
        parse_file(path)?
    } else {
        let dev_tree = Path::new("config/scoring.toml");
        if dev_tree.exists() {
            parse_file(dev_tree)?
        } else {
            parse_str(BUILTIN_DEFAULT, "<compiled-in default config/scoring.toml>")?
        }
    };
    validate(&cfg)?;
    Ok(cfg)
}

/// Reject degenerate values that would silently poison downstream
/// arithmetic. `tau_days = 0` is the case worth catching: `exp(-0/0)` is
/// `NaN`, and a single NaN propagates through every `+=` into the
/// decayed sums, contaminating sort order once scoring lands.
fn validate(cfg: &ScoringConfig) -> Result<()> {
    ensure!(
        cfg.decay.tau_dm_days > 0,
        "decay.tau_dm_days must be > 0 (got 0). To make DM decay effectively negligible, \
         set a very large τ — never 0, which produces NaN at Δt=0.",
    );
    ensure!(
        cfg.decay.tau_content_days > 0,
        "decay.tau_content_days must be > 0 (got 0). To make content decay effectively negligible, \
         set a very large τ — never 0, which produces NaN at Δt=0.",
    );
    Ok(())
}

const BUILTIN_DEFAULT: &str = include_str!("../config/scoring.toml");

fn parse_file(path: &Path) -> Result<ScoringConfig> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("reading scoring config from {}", path.display()))?;
    parse_str(&body, &path.display().to_string())
}

fn parse_str(body: &str, source_label: &str) -> Result<ScoringConfig> {
    toml::from_str(body).with_context(|| format!("parsing scoring config from {source_label}"))
}

#[cfg(test)]
mod tests {
    //! Unit tests pin (a) the compiled-in default parses, (b) explicit
    //! `--config` paths route through `fs::read_to_string`, and (c) parse
    //! failures surface with the source label (so users see *which* file
    //! is malformed, not just "TOML error").
    use super::*;

    #[test]
    fn builtin_default_parses() {
        let cfg = read_scoring_config(None).expect("builtin default must parse");
        assert!(cfg.decay.tau_dm_days > 0);
        assert!(cfg.decay.tau_content_days > 0);
    }

    #[test]
    fn cli_path_missing_is_hard_error() {
        let err = read_scoring_config(Some(Path::new("/no/such/scoring.toml")))
            .expect_err("missing --config path must be a hard error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("/no/such/scoring.toml"),
            "error should name the missing path: {msg}",
        );
    }

    #[test]
    fn malformed_toml_names_the_source() {
        let err = parse_str("decay = not_a_table\n", "/synthetic.toml")
            .expect_err("malformed TOML must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("/synthetic.toml"),
            "error should name the source: {msg}",
        );
    }

    #[test]
    fn missing_decay_section_fails() {
        // serde ignores unknown top-level keys but a missing required
        // section (no [decay]) must surface as a parse error rather than
        // defaulting to zero τ — `exp(-Δt / 0)` would be NaN.
        let err = parse_str("[weights]\ndm = 3.0\n", "/synthetic.toml")
            .expect_err("missing [decay] must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("decay"), "error must mention `decay`: {msg}");
    }

    #[test]
    fn zero_tau_is_rejected_at_load() {
        // `tau_days = 0` parses cleanly (u32 accepts 0) but `decay_weight`
        // would then evaluate `exp(-0/0) = NaN` at Δt=0 — silently
        // poisoning every decayed sum and breaking sort order. The
        // loader must reject it loudly with a message that names the
        // offending key so users can correct the typo.
        for (key, body) in [
            (
                "tau_dm_days",
                "[decay]\ntau_dm_days = 0\ntau_content_days = 365\n",
            ),
            (
                "tau_content_days",
                "[decay]\ntau_dm_days = 180\ntau_content_days = 0\n",
            ),
        ] {
            // Parse succeeds; validate must fail.
            let cfg: ScoringConfig = toml::from_str(body).expect("toml parses");
            let err = validate(&cfg).expect_err("τ = 0 must be rejected");
            let msg = format!("{err:#}");
            assert!(
                msg.contains(key),
                "error must name the offending key `{key}`: {msg}",
            );
        }
    }
}
