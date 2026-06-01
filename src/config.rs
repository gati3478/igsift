//! Scoring configuration, loaded from a TOML file.
//!
//! Keeping weights and decay constants in TOML lets them be tuned without a
//! rebuild — the main ergonomic gap Rust has versus a notebook workflow.
//!
//! Slice 7B-2 wired `[decay]` ([`DecayConfig`]). The first-pass scoring
//! slice extends the same target type with [`WeightsConfig`] and
//! [`ScoringParams`] — one config struct backs the whole pipeline so the
//! read path stays single-purpose.
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
//! A platform config dir (`~/.config/igsift/scoring.toml`) is in the
//! comments of the slice 6 scaffold; it needs the `directories` /
//! `dirs` crate and is deferred until somebody actually installs the
//! binary outside the dev tree.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

/// Top-level scoring configuration. Backs the whole pipeline — decay
/// constants feed the aggregator, weights and scoring params feed
/// [`crate::scoring`].
#[derive(Debug, Clone, Deserialize)]
pub struct ScoringConfig {
    pub decay: DecayConfig,
    pub weights: WeightsConfig,
    pub scoring: ScoringParams,
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

/// Per-feature weights. Every field corresponds 1:1 with a key in the
/// `[weights]` section of `config/scoring.toml` and appears exactly once
/// in DESIGN.md's "Scoring composition" formula. The composition function
/// in [`crate::scoring`] reproduces that 1:1 mapping by naming each
/// local term identically to its field here — so a weight change in TOML
/// can be traced through to a single line of scoring code.
#[derive(Debug, Clone, Deserialize)]
pub struct WeightsConfig {
    pub dm: f64,
    pub likes: f64,
    pub comments: f64,
    pub story_out: f64,
    pub stories_viewed: f64,
    pub saved: f64,
    pub reactions_given: f64,
    pub reactions_received: f64,
    pub tenure: f64,
    pub dm_balance_penalty: f64,
    pub reaction_balance_penalty: f64,
    pub hide_story_penalty: f64,
    pub removed_suggestion_penalty: f64,
    /// Subtracted from `score_raw` for a personal, non-mutual account the
    /// owner marked close-friend/favorited (an unreciprocated explicit tie).
    /// Always-on like the other penalties — `0.0` disables the score erosion;
    /// the Review floor is governed separately by
    /// `ScoringParams::demote_nonmutual_close_ties`. See
    /// `scoring::is_nonreciprocal_close_tie`.
    pub nonmutual_close_tie_penalty: f64,
    pub close_friend_boost: f64,
    pub favorite_boost: f64,
    pub inbound_request: f64,
}

/// Sigmoid mapping `score_raw → keep_prob` plus the bucket cut-offs.
///
/// `keep_prob = sigmoid((score_raw - threshold) / scale)` with the
/// bucketing rule:
///
/// - `keep_prob >= keep_min` → keep
/// - `keep_prob < unfollow_max` → unfollow (with the additional gates
///   documented in DESIGN.md "Buckets")
/// - everything in between → review
#[derive(Debug, Clone, Deserialize)]
pub struct ScoringParams {
    pub threshold: f64,
    pub scale: f64,
    pub keep_min: f64,
    pub unfollow_max: f64,
    /// When `true` (opt-in; **off** by default), a personal, non-mutual
    /// account with no inbound signal cannot bucket `keep` on
    /// one-directional consumption alone — it floors at `review`. See
    /// [`crate::scoring::assign_bucket`]. The `engagement` preset also
    /// leaves this off (it scores raw activity by design).
    #[serde(default = "default_require_reciprocity_for_keep")]
    pub require_reciprocity_for_keep: bool,
    /// A mutual account whose reciprocal age (`mutual_age_days`) is ≥ this
    /// many days floors at `keep` — a long reciprocal history is a real
    /// relationship. `0` disables the floor. Default 730 (2 years).
    #[serde(default = "default_deep_mutual_keep_days")]
    pub deep_mutual_keep_days: u32,
    /// Evidence guard for the effort-skew gate: the gate only acts on a
    /// thread with at least this many owner-side (non-shadow) messages —
    /// see [`crate::scoring`]'s `dm_out`. **`0` disables the entire gate**
    /// (sentinel; mirrors
    /// `deep_mutual_keep_days == 0`) — it is NOT "evidence bar of zero",
    /// which would fire on every thread. See
    /// `docs/specs/2026-05-31-effort-skew-gate-design.md`.
    #[serde(default = "default_effort_skew_min_dm_out")]
    pub effort_skew_min_dm_out: u32,
    /// SOFT tier: an unmarked personal account scoring into Keep whose
    /// `dm_balance` (post-dedup reply skew) is ≥ this is demoted to Review.
    #[serde(default = "default_effort_skew_soft")]
    pub effort_skew_soft: f64,
    /// HARD tier: any account whose reply skew is ≥ this is demoted to
    /// Review even with a close-friend / favorite / mutual marker (keeplist
    /// and the restricted floor still win).
    #[serde(default = "default_effort_skew_hard")]
    pub effort_skew_hard: f64,
    /// When `true` (**default**), a personal, non-mutual account marked
    /// close-friend/favorited (and not keeplisted) is floored at `review`
    /// instead of auto-keeping on the stale marker — the mirror-inverse of the
    /// reciprocity ceiling. Monotonic: Keep → Review only. See
    /// [`crate::scoring::assign_bucket`]. Unlike effort-skew/reciprocity this
    /// defaults ON: it is high-precision (explicit marker + non-mutual +
    /// personal) and never produces Unfollow.
    #[serde(default = "default_demote_nonmutual_close_ties")]
    pub demote_nonmutual_close_ties: bool,
    /// A personal mutual younger than this many days that shows no
    /// interaction in either direction (no DM sent or received, no inbound
    /// reaction, ≤ 1 like/comment in 90d) is floored at `review` instead of
    /// auto-keeping on undecayed mutual + tenure alone — a follow-back that
    /// never became a relationship. `0` disables (sentinel; mirrors
    /// `deep_mutual_keep_days`). Monotonic: Keep → Review only. Default 437
    /// (≈ p25 of a content-consumer's kept-mutual tenure). See
    /// `docs/specs/2026-06-01-dead-mutual-review-gate-design.md`.
    #[serde(default = "default_dead_mutual_review_max_tenure_days")]
    pub dead_mutual_review_max_tenure_days: u32,
}

fn default_require_reciprocity_for_keep() -> bool {
    // Off by default: the only labeled data we have (2026-05-30 pass) showed
    // the reciprocity ceiling demotes deliberately-curated one-way follows,
    // halving agreement. It stays an opt-in toggle for mutual-heavy users who
    // want non-mutual strangers surfaced — see docs/TUNING.md round 7.
    false
}

fn default_deep_mutual_keep_days() -> u32 {
    730
}

fn default_effort_skew_min_dm_out() -> u32 {
    // Disabled by default: the gate can override IG keep markers, so it must
    // be opt-in. The owner's config/scoring.toml turns it on.
    0
}

fn default_effort_skew_soft() -> f64 {
    0.85
}

fn default_effort_skew_hard() -> f64 {
    0.95
}

fn default_demote_nonmutual_close_ties() -> bool {
    // On by default — see docs/specs/2026-06-01-nonmutual-close-tie-gate-design.md.
    // High-precision (explicit marker + non-mutual + personal), Review-only.
    true
}

fn default_dead_mutual_review_max_tenure_days() -> u32 {
    // On by default at the p25 of the owner's kept-mutual tenure — see
    // docs/specs/2026-06-01-dead-mutual-review-gate-design.md. Review-only,
    // zero measured labels.txt regression. 0 disables.
    437
}

/// Read and parse the scoring config, following the documented resolution
/// chain in the module doc.
///
/// Resolution order (first match wins):
/// 1. `preset` (when caller passed a `--preset NAME`) — embedded bytes,
///    cannot fail to find but can fail to parse if the shipped file is
///    malformed (compile-time-checked via [`include_str!`]).
/// 2. `cli_config` (when caller passed a `--config PATH`) — hard error
///    on missing or malformed file.
/// 3. `./config/scoring.toml` if present in cwd.
/// 4. Compiled-in default (= the `balanced` preset bytes).
pub fn read_scoring_config(
    cli_config: Option<&Path>,
    preset: Option<crate::cli::Preset>,
) -> Result<ScoringConfig> {
    let (cfg, source) = if let Some(p) = preset {
        (
            parse_str(p.body(), &format!("<preset {}>", p.name()))?,
            format!("--preset {}", p.name()),
        )
    } else if let Some(path) = cli_config {
        (parse_file(path)?, format!("--config {}", path.display()))
    } else {
        let dev_tree = Path::new("config/scoring.toml");
        if dev_tree.exists() {
            (parse_file(dev_tree)?, dev_tree.display().to_string())
        } else {
            (
                parse_str(BUILTIN_DEFAULT, "<compiled-in default (balanced preset)>")?,
                "<compiled-in default (balanced preset)>".to_owned(),
            )
        }
    };
    validate(&cfg)?;
    // Surface which resolution rung fired so a user who laid down a custom
    // `config/scoring.toml` but invoked the binary from the wrong cwd sees
    // "<compiled-in default>" instead of silently getting baseline weights
    // and wondering why their tuning didn't take effect. Demoted to debug
    // so the default-verbosity stdout stays focused on the audit summary
    // and doesn't interleave with the progress spinner — re-emerges with
    // `-v`.
    tracing::debug!("loaded scoring config: {source}");
    Ok(cfg)
}

/// Reject degenerate values that would silently poison downstream
/// arithmetic.
///
/// `tau_days = 0` produces `exp(-0/0) = NaN`, and a single NaN propagates
/// through every `+=` into the decayed sums — contaminating sort order
/// the moment scoring runs. The same NaN-propagation risk extends to the
/// weight and scoring-param fields: a non-finite weight contaminates
/// `score_raw`; `scale = 0` divides by zero inside the sigmoid;
/// `keep_min <= unfollow_max` collapses the review band.
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

    for (name, w) in [
        ("dm", cfg.weights.dm),
        ("likes", cfg.weights.likes),
        ("comments", cfg.weights.comments),
        ("story_out", cfg.weights.story_out),
        ("stories_viewed", cfg.weights.stories_viewed),
        ("saved", cfg.weights.saved),
        ("reactions_given", cfg.weights.reactions_given),
        ("reactions_received", cfg.weights.reactions_received),
        ("tenure", cfg.weights.tenure),
        ("dm_balance_penalty", cfg.weights.dm_balance_penalty),
        (
            "reaction_balance_penalty",
            cfg.weights.reaction_balance_penalty,
        ),
        ("hide_story_penalty", cfg.weights.hide_story_penalty),
        (
            "removed_suggestion_penalty",
            cfg.weights.removed_suggestion_penalty,
        ),
        (
            "nonmutual_close_tie_penalty",
            cfg.weights.nonmutual_close_tie_penalty,
        ),
        ("close_friend_boost", cfg.weights.close_friend_boost),
        ("favorite_boost", cfg.weights.favorite_boost),
        ("inbound_request", cfg.weights.inbound_request),
    ] {
        ensure!(
            w.is_finite(),
            "weights.{name} must be finite (got {w}). NaN/Inf propagates through every `+=` in scoring."
        );
    }

    ensure!(
        cfg.scoring.scale > 0.0 && cfg.scoring.scale.is_finite(),
        "scoring.scale must be a finite positive number (got {}). 0 divides by zero inside the sigmoid.",
        cfg.scoring.scale,
    );
    ensure!(
        cfg.scoring.threshold.is_finite(),
        "scoring.threshold must be finite (got {}).",
        cfg.scoring.threshold,
    );
    ensure!(
        (0.0..=1.0).contains(&cfg.scoring.keep_min),
        "scoring.keep_min must lie in [0, 1] (got {}); it is a probability.",
        cfg.scoring.keep_min,
    );
    ensure!(
        (0.0..=1.0).contains(&cfg.scoring.unfollow_max),
        "scoring.unfollow_max must lie in [0, 1] (got {}); it is a probability.",
        cfg.scoring.unfollow_max,
    );
    ensure!(
        cfg.scoring.keep_min > cfg.scoring.unfollow_max,
        "scoring.keep_min ({}) must be strictly greater than scoring.unfollow_max ({}); \
         otherwise the review band collapses.",
        cfg.scoring.keep_min,
        cfg.scoring.unfollow_max,
    );

    for (name, v) in [
        ("effort_skew_soft", cfg.scoring.effort_skew_soft),
        ("effort_skew_hard", cfg.scoring.effort_skew_hard),
    ] {
        ensure!(
            (0.0..=1.0).contains(&v),
            "scoring.{name} must lie in [0, 1] (got {v}); it is a reply-skew ratio.",
        );
    }
    ensure!(
        cfg.scoring.effort_skew_hard >= cfg.scoring.effort_skew_soft,
        "scoring.effort_skew_hard ({}) must be >= scoring.effort_skew_soft ({}); \
         the hard (marker-overriding) tier cannot be easier to trip than the soft tier.",
        cfg.scoring.effort_skew_hard,
        cfg.scoring.effort_skew_soft,
    );

    Ok(())
}

/// Compiled-in fallback when no preset / config flag / cwd file resolves.
/// Points at the balanced preset specifically (NOT the user-editable
/// `config/scoring.toml`) so a binary-only install ships with the
/// unbiased default rather than whatever calibration is currently
/// committed to the repo for the project owner's labeled set.
const BUILTIN_DEFAULT: &str = include_str!("../config/presets/balanced.toml");

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
    //! `--config` paths route through `fs::read_to_string`, (c) parse
    //! failures surface with the source label, and (d) the validate()
    //! posture loudly rejects each NaN-poisoning input vector (zero τ,
    //! non-finite weights, zero scale, inverted bucket cut-offs).
    use super::*;

    /// A complete, valid TOML body. Tests start from this baseline and
    /// override only the field under test so each case stays focused on
    /// the one input it pins, rather than restating the whole config.
    const VALID_BODY: &str = r#"
[decay]
tau_dm_days = 180
tau_content_days = 365

[weights]
dm = 3.0
likes = 1.0
comments = 1.5
story_out = 1.0
stories_viewed = 0.2
saved = 0.5
reactions_given = 1.0
reactions_received = 2.0
tenure = 0.3
dm_balance_penalty = 1.0
reaction_balance_penalty = 0.5
hide_story_penalty = 2.0
removed_suggestion_penalty = 0.3
close_friend_boost = 5.0
favorite_boost = 3.0
inbound_request = 0.5
nonmutual_close_tie_penalty = 4.0

[scoring]
threshold = 0.0
scale = 1.0
keep_min = 0.7
unfollow_max = 0.3
"#;

    #[test]
    fn builtin_default_parses() {
        let cfg = read_scoring_config(None, None).expect("builtin default must parse");
        assert!(cfg.decay.tau_dm_days > 0);
        assert!(cfg.decay.tau_content_days > 0);
        assert!(cfg.weights.dm.is_finite());
        assert!(cfg.scoring.scale > 0.0);
    }

    #[test]
    fn cli_path_missing_is_hard_error() {
        let err = read_scoring_config(Some(Path::new("/no/such/scoring.toml")), None)
            .expect_err("missing --config path must be a hard error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("/no/such/scoring.toml"),
            "error should name the missing path: {msg}",
        );
    }

    #[test]
    fn all_presets_parse_and_validate() {
        // Every shipped preset must parse and clear validate() — a
        // regression in any of the three preset TOML files would brick
        // the binary's default config path or one of the user-facing
        // `--preset` choices.
        for p in [
            crate::cli::Preset::Balanced,
            crate::cli::Preset::Engagement,
            crate::cli::Preset::Tenure,
        ] {
            let cfg = read_scoring_config(None, Some(p))
                .unwrap_or_else(|e| panic!("preset {} must parse: {e:#}", p.name()));
            assert!(cfg.scoring.keep_min > cfg.scoring.unfollow_max);
        }
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
    fn valid_body_parses_and_validates() {
        let cfg = parse_str(VALID_BODY, "/synthetic.toml").expect("baseline must parse");
        validate(&cfg).expect("baseline must validate");
    }

    #[test]
    fn missing_decay_section_fails() {
        // A complete TOML missing only the [decay] table must surface as
        // a parse error — defaulting to τ=0 would poison every decayed
        // sum with `exp(-Δt/0) = NaN`.
        let body = VALID_BODY.replace("[decay]\ntau_dm_days = 180\ntau_content_days = 365\n", "");
        let err = parse_str(&body, "/synthetic.toml").expect_err("missing [decay] must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("decay"), "error must mention `decay`: {msg}");
    }

    #[test]
    fn missing_weights_section_fails() {
        let body = VALID_BODY.split("\n[weights]").next().unwrap().to_owned()
            + "\n[scoring]\nthreshold = 0.0\nscale = 1.0\nkeep_min = 0.7\nunfollow_max = 0.3\n";
        let err = parse_str(&body, "/synthetic.toml").expect_err("missing [weights] must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("weights"),
            "error must mention `weights`: {msg}",
        );
    }

    #[test]
    fn zero_tau_is_rejected_at_load() {
        // `tau_days = 0` parses cleanly (u32 accepts 0) but `decay_weight`
        // would then evaluate `exp(-0/0) = NaN` at Δt=0 — silently
        // poisoning every decayed sum and breaking sort order.
        for (key, replacement) in [
            ("tau_dm_days", "tau_dm_days = 0"),
            ("tau_content_days", "tau_content_days = 0"),
        ] {
            let body = VALID_BODY
                .replace(&format!("{key} = 180"), replacement)
                .replace(&format!("{key} = 365"), replacement);
            let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
            let err = validate(&cfg).expect_err("τ = 0 must be rejected");
            let msg = format!("{err:#}");
            assert!(
                msg.contains(key),
                "error must name the offending key `{key}`: {msg}",
            );
        }
    }

    #[test]
    fn zero_scale_is_rejected() {
        let body = VALID_BODY.replace("scale = 1.0", "scale = 0.0");
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("scale = 0 must be rejected");
        let msg = format!("{err:#}");
        assert!(msg.contains("scale"), "error must mention `scale`: {msg}");
    }

    #[test]
    fn non_finite_weight_is_rejected() {
        // TOML doesn't have a literal NaN, but `nan` is accepted as a
        // float keyword. Verify both NaN and inf are caught.
        for (val, label) in [("nan", "NaN"), ("inf", "Inf")] {
            let body = VALID_BODY.replace("dm = 3.0", &format!("dm = {val}"));
            let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
            let err = validate(&cfg).expect_err("{label} weight must be rejected");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("dm"),
                "error for {label} must name the offending weight `dm`: {msg}",
            );
        }
    }

    #[test]
    fn inverted_bucket_cutoffs_are_rejected() {
        // keep_min ≤ unfollow_max collapses the review band — every
        // account lands in either keep or unfollow with no middle.
        let body = VALID_BODY
            .replace("keep_min = 0.7", "keep_min = 0.2")
            .replace("unfollow_max = 0.3", "unfollow_max = 0.5");
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("inverted cut-offs must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("keep_min") && msg.contains("unfollow_max"),
            "error must reference both cut-offs: {msg}",
        );
    }

    #[test]
    fn out_of_range_keep_min_is_rejected() {
        let body = VALID_BODY.replace("keep_min = 0.7", "keep_min = 1.5");
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("keep_min > 1 must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("keep_min"),
            "error must mention `keep_min`: {msg}",
        );
    }

    #[test]
    fn effort_skew_defaults_to_disabled() {
        // A config body with no effort-skew keys must parse, with the gate
        // OFF by default (min_dm_out == 0 sentinel) so presets / binary-only
        // installs keep current behavior.
        let cfg = parse_str(VALID_BODY, "/synthetic.toml").expect("parses");
        validate(&cfg).expect("validates");
        assert_eq!(cfg.scoring.effort_skew_min_dm_out, 0);
        assert_eq!(cfg.scoring.effort_skew_soft, 0.85);
        assert_eq!(cfg.scoring.effort_skew_hard, 0.95);
    }

    #[test]
    fn effort_skew_hard_below_soft_is_rejected() {
        let body = format!(
            "{VALID_BODY}effort_skew_min_dm_out = 8\n\
             effort_skew_soft = 0.9\neffort_skew_hard = 0.8\n"
        );
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("hard < soft must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("effort_skew_hard") && msg.contains("effort_skew_soft"),
            "error must reference both thresholds: {msg}",
        );
    }

    #[test]
    fn effort_skew_threshold_out_of_range_is_rejected() {
        let body = format!("{VALID_BODY}effort_skew_soft = 1.5\n");
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("soft > 1 must be rejected");
        assert!(format!("{err:#}").contains("effort_skew_soft"));
    }

    #[test]
    fn nonmutual_close_tie_defaults_to_demote_true() {
        // A config body with no `demote_nonmutual_close_ties` key parses with
        // the gate ON by default — unlike effort-skew/reciprocity, this gate
        // ships live (high-precision, Review-only). The weight is required, so
        // VALID_BODY carries it (see `nonmutual_close_tie_penalty_is_required`).
        let cfg = parse_str(VALID_BODY, "/synthetic.toml").expect("parses");
        validate(&cfg).expect("validates");
        assert!(cfg.scoring.demote_nonmutual_close_ties);
    }

    #[test]
    fn dead_mutual_review_max_tenure_defaults_to_437() {
        // A config body with no `dead_mutual_review_max_tenure_days` key
        // parses with the gate ON by default at the p25 tenure threshold —
        // like the close-tie gate, this ships live (Review-only). Mirror of
        // `deep_mutual_keep_days` wiring; 0 disables.
        let cfg = parse_str(VALID_BODY, "/synthetic.toml").expect("parses");
        validate(&cfg).expect("validates");
        assert_eq!(cfg.scoring.dead_mutual_review_max_tenure_days, 437);
    }

    #[test]
    fn dead_mutual_review_max_tenure_zero_disables() {
        let body = format!("{VALID_BODY}dead_mutual_review_max_tenure_days = 0\n");
        let cfg = parse_str(&body, "/synthetic.toml").expect("parses");
        assert_eq!(cfg.scoring.dead_mutual_review_max_tenure_days, 0);
    }

    #[test]
    fn nonmutual_close_tie_penalty_is_required() {
        // The penalty is a [weights] field with no serde default (every weight
        // is required). A body omitting it must fail to parse, naming the field.
        let body = VALID_BODY.replace("nonmutual_close_tie_penalty = 4.0\n", "");
        let err =
            parse_str(&body, "/synthetic.toml").expect_err("missing required weight must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nonmutual_close_tie_penalty"),
            "error must name the missing weight: {msg}",
        );
    }

    #[test]
    fn non_finite_nonmutual_close_tie_penalty_is_rejected() {
        let body = VALID_BODY.replace(
            "nonmutual_close_tie_penalty = 4.0",
            "nonmutual_close_tie_penalty = nan",
        );
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("NaN penalty must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nonmutual_close_tie_penalty"),
            "error must name the offending weight: {msg}",
        );
    }
}
