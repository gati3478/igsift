//! First-pass scoring and bucketing.
//!
//! Composes the decay-weighted counts plus boolean flags on
//! [`crate::features::AccountFeatures`] into a single `keep_prob ∈ [0, 1]`
//! via the formula in [`docs/DESIGN.md`](../../docs/DESIGN.md) ("Scoring
//! composition"). Bucketing follows the same source: `keep` at the top,
//! `unfollow` at the bottom (with restricted / boosted gates),
//! `review` covers everything in between.
//!
//! Scoring is embarrassingly parallel across accounts; the current
//! implementation is serial because the account count keeps scoring
//! sub-millisecond. The `&[AccountFeatures]` → `Vec<ScoredAccount>`
//! boundary stays shape-compatible with a future parallel swap if that
//! ever stops holding.
//!
//! ## Where penalties live
//!
//! `dm_balance_penalty` and `reaction_balance_penalty` are derived **here**,
//! not on `AccountFeatures`. The slice 7B-1 design note made the call
//! explicit: `dm_balance` is stored as a raw ratio, and the volume-gating
//! decision belongs alongside the weight in the scoring layer. A
//! `dm_balance = 0.5` over 2 greetings is not the relationship signal that
//! the same ratio over 1000 messages is — gating on volume here keeps the
//! ratio honest at the source.
//!
//! ## Account-class + allowlist gates
//!
//! DESIGN.md's `Unfollow` bucket requires `account_class == Personal` AND
//! not `is_close_friend`/`is_favorited`/`is_restricted`. Brand accounts
//! detected by [`crate::features::account_class::Classifier`] downgrade
//! Unfollow → Review in [`assign_bucket`]. The user-maintained keep-
//! allowlist (`config/keep_allowlist.txt`) surfaces on
//! [`AccountFeatures::is_keep_allowlisted`] and applies the same
//! Unfollow → Review override — a parallel signal to `is_close_friend` /
//! `is_favorited`, kept separate so the CSV `account_class` column
//! doesn't lie about a personal close-friend's profile.

use crate::config::{ScoringConfig, ScoringParams, WeightsConfig};
use crate::features::{AccountClass, AccountFeatures};

/// Below this DM-volume threshold, `dm_balance_penalty` is suppressed.
///
/// `dm_balance` is a ratio: a fresh thread with 1 outbound message has
/// `dm_balance = Some(1.0)` — fully one-sided me — which would otherwise
/// max out the penalty. The threshold is a "real relationship" floor;
/// 5 is a starting guess to be tuned alongside the weights.
const MIN_DM_VOLUME_FOR_BALANCE: u32 = 5;

/// Below this combined-reaction threshold, `reaction_balance_penalty` is
/// suppressed. Same shape as [`MIN_DM_VOLUME_FOR_BALANCE`].
const MIN_REACTION_VOLUME_FOR_BALANCE: u32 = 5;

/// Bucket assignment per DESIGN.md "Buckets".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Keep,
    Review,
    Unfollow,
}

impl Bucket {
    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Keep => "keep",
            Bucket::Review => "review",
            Bucket::Unfollow => "unfollow",
        }
    }
}

/// One scored account. Carries the underlying features so the CSV /
/// Markdown writers (separate slice) can emit raw + scored data side by
/// side without re-joining.
#[derive(Debug, Clone)]
pub struct ScoredAccount {
    pub features: AccountFeatures,
    pub score_raw: f64,
    pub keep_prob: f64,
    pub bucket: Bucket,
    /// Label of the term with the largest signed contribution to
    /// `score_raw`. Drives the `notes` CSV column.
    pub dominant_feature: &'static str,
    /// Top-3 signed contributions to `score_raw`, by absolute
    /// magnitude descending. Used by the Markdown card writer for
    /// "Why this bucket". A `0.0` entry signals "fewer than 3 non-zero
    /// terms" and renders as omitted; the dominant term is always
    /// `top_terms[0]`.
    pub top_terms: [(&'static str, f64); 3],
}

/// Score every account in `features` with the weights and bucket
/// cut-offs in `cfg`.
///
/// Output order mirrors input order — the caller sorts when it cares
/// (e.g. `lib::run` sorts by `keep_prob` for the top/bottom summary).
pub fn score(features: &[AccountFeatures], cfg: &ScoringConfig) -> Vec<ScoredAccount> {
    features
        .iter()
        .map(|f| score_one(f, &cfg.weights, &cfg.scoring))
        .collect()
}

fn score_one(f: &AccountFeatures, w: &WeightsConfig, p: &ScoringParams) -> ScoredAccount {
    let (score_raw, dominant_feature, top_terms) = compose_score(f, w);
    let keep_prob = sigmoid((score_raw - p.threshold) / p.scale);
    let bucket = assign_bucket(f, keep_prob, p);
    ScoredAccount {
        features: f.clone(),
        score_raw,
        keep_prob,
        bucket,
        dominant_feature,
        top_terms,
    }
}

/// Number of terms in the scoring composition. Pinned via this constant so
/// `term_contributions` returns a fixed-size array and any future term
/// addition fails to compile until both the const and the array literal
/// move together.
pub const NUM_TERMS: usize = 16;

/// Per-term signed contributions to `score_raw`. Penalties surface negative
/// so consumers (the dominant-feature pick, the `--trace` flag) can rank
/// terms by signed magnitude without re-deriving the sign of each term.
///
/// Labels match the field names in [`WeightsConfig`] (and therefore the keys
/// in `config/scoring.toml`) so a `dominant_feature: dm` call-out or a
/// trace row maps unambiguously back to the term that drove it.
pub fn term_contributions(
    f: &AccountFeatures,
    w: &WeightsConfig,
) -> [(&'static str, f64); NUM_TERMS] {
    let tenure = match f.follow_tenure_days {
        Some(d) => w.tenure * (f64::from(d) + 1.0).ln(),
        None => 0.0,
    };
    let close_friend = if f.is_close_friend {
        w.close_friend_boost
    } else {
        0.0
    };
    let favorite = if f.is_favorited {
        w.favorite_boost
    } else {
        0.0
    };
    let inbound_request = if f.inbound_dm_request {
        w.inbound_request
    } else {
        0.0
    };
    let hide_story = if f.is_hide_story_from {
        w.hide_story_penalty
    } else {
        0.0
    };
    let removed_suggestion = if f.is_removed_suggestion {
        w.removed_suggestion_penalty
    } else {
        0.0
    };
    let dm_balance_term = w.dm_balance_penalty * dm_balance_penalty(f);
    let reaction_balance_term = w.reaction_balance_penalty * reaction_balance_penalty(f);

    [
        ("dm", w.dm * f.dm_messages_total_decayed),
        ("likes", w.likes * f.likes_given_decayed),
        ("comments", w.comments * f.comments_given_decayed),
        ("story_out", w.story_out * f.story_interactions_out_decayed),
        (
            "stories_viewed",
            w.stories_viewed * f.stories_viewed_decayed,
        ),
        ("saved", w.saved * f.saved_their_content_decayed),
        (
            "reactions_given",
            w.reactions_given * f.dm_reactions_given_decayed,
        ),
        (
            "reactions_received",
            w.reactions_received * f.dm_reactions_received_decayed,
        ),
        ("tenure", tenure),
        ("close_friend_boost", close_friend),
        ("favorite_boost", favorite),
        ("inbound_request", inbound_request),
        ("dm_balance_penalty", -dm_balance_term),
        ("reaction_balance_penalty", -reaction_balance_term),
        ("hide_story_penalty", -hide_story),
        ("removed_suggestion_penalty", -removed_suggestion),
    ]
}

/// Apply the per-weight composition formula and return the raw score,
/// the dominant term label, and the top-3 signed contributions.
fn compose_score(
    f: &AccountFeatures,
    w: &WeightsConfig,
) -> (f64, &'static str, [(&'static str, f64); 3]) {
    let contribs = term_contributions(f, w);
    let score_raw: f64 = contribs.iter().map(|(_, v)| v).sum();

    // Sort by |contribution| descending so dominant_feature == top_terms[0]
    // and the writer can read both off the same array.
    let mut ranked = contribs;
    ranked.sort_by(|(_, a), (_, b)| {
        b.abs()
            .partial_cmp(&a.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut top_terms: [(&'static str, f64); 3] = [("", 0.0); 3];
    for (slot, (label, val)) in top_terms.iter_mut().zip(ranked.iter().take(3)) {
        if *val != 0.0 {
            *slot = (*label, *val);
        }
    }

    // Penalties surface signed negative so a hide-story account whose
    // penalty dominates renders as "hide_story_penalty", not as a
    // small positive engagement term.
    let dominant = if top_terms[0].1 != 0.0 {
        top_terms[0].0
    } else {
        "none"
    };

    (score_raw, dominant, top_terms)
}

/// `max(0, dm_balance - 0.5) * 2` ∈ [0, 1], gated on a minimum DM volume.
///
/// Asymmetric by design: `dm_balance = 1.0` (fully one-sided me) returns
/// `1.0`, the full penalty. `dm_balance = 0.0` (fully one-sided them) is
/// not over-extension — it's reciprocity in the inbound direction, not a
/// penalty signal — so it returns `0.0`. Threads below the volume floor
/// return `0.0` regardless of ratio: 1-message threads have unstable
/// ratios and shouldn't dominate scoring.
fn dm_balance_penalty(f: &AccountFeatures) -> f64 {
    if f.dm_messages_total < MIN_DM_VOLUME_FOR_BALANCE {
        return 0.0;
    }
    match f.dm_balance {
        Some(b) => (f64::from(b) - 0.5).max(0.0) * 2.0,
        None => 0.0,
    }
}

/// Same shape as [`dm_balance_penalty`]; computed from raw reaction
/// counts (not stored as a ratio on `AccountFeatures` — slice 7B-1
/// kept only `dm_balance` materialized).
fn reaction_balance_penalty(f: &AccountFeatures) -> f64 {
    let total = f.dm_reactions_given + f.dm_reactions_received;
    if total < MIN_REACTION_VOLUME_FOR_BALANCE {
        return 0.0;
    }
    let given = f64::from(f.dm_reactions_given);
    let denom = f64::from(total);
    let balance = given / denom;
    (balance - 0.5).max(0.0) * 2.0
}

/// Standard logistic. `sigmoid(0) = 0.5`; `sigmoid(+∞) = 1.0`;
/// `sigmoid(-∞) = 0.0`. The `(-x).exp()` form is the numerically stable
/// arm for positive `x`; for very negative `x` `exp(-x)` saturates to
/// `+∞` which the IEEE division `1 / (1 + ∞)` returns as `0.0` — no
/// branch needed.
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn assign_bucket(f: &AccountFeatures, keep_prob: f64, p: &ScoringParams) -> Bucket {
    // Restricted floors the bucket at `review`, regardless of `keep_prob`
    // — DESIGN.md treats "restricted" as a manual signal that the
    // account warrants human attention before any automated drop call.
    if f.is_restricted {
        return Bucket::Review;
    }
    if keep_prob >= p.keep_min {
        return Bucket::Keep;
    }
    if keep_prob < p.unfollow_max {
        // DESIGN.md gates `unfollow` on `account_class == Personal` AND
        // `!is_close_friend && !is_favorited && !is_restricted`. Restricted
        // is handled above; the remaining gates fold in here. The keep-
        // allowlist (`config/keep_allowlist.txt`) is the user-maintained
        // override for accounts the lexicon can't be trusted on —
        // brands / public figures the heuristic missed, or personal
        // accounts the export under-represents.
        if f.is_close_friend
            || f.is_favorited
            || f.is_keep_allowlisted
            || f.account_class != AccountClass::Personal
        {
            return Bucket::Review;
        }
        return Bucket::Unfollow;
    }
    Bucket::Review
}

#[cfg(test)]
mod tests {
    //! Pins the load-bearing invariants of the scoring layer:
    //! sigmoid endpoints, bucket boundary semantics, the restricted
    //! floor, the boost-beats-penalty composition rule, and engagement
    //! monotonicity. Each test feeds a single hand-shaped
    //! `AccountFeatures` value — none of the parser layer or the
    //! aggregator runs.
    use super::*;
    use crate::config::{DecayConfig, ScoringConfig, ScoringParams, WeightsConfig};

    fn baseline_account(handle: &str) -> AccountFeatures {
        AccountFeatures {
            username: handle.to_owned(),
            display_name: None,
            account_class: AccountClass::default(),
            follow_tenure_days: None,
            is_close_friend: false,
            is_favorited: false,
            is_blocked: false,
            is_restricted: false,
            is_hide_story_from: false,
            is_removed_suggestion: false,
            recently_unfollowed: false,
            is_mutual: false,
            is_keep_allowlisted: false,
            likes_given: 0,
            comments_given: 0,
            story_interactions_out: 0,
            stories_viewed: 0,
            saved_their_content: 0,
            dm_messages_total: 0,
            dm_recency_days: None,
            dm_balance: None,
            dm_reactions_given: 0,
            dm_reactions_received: 0,
            inbound_dm_request: false,
            likes_given_decayed: 0.0,
            comments_given_decayed: 0.0,
            story_interactions_out_decayed: 0.0,
            stories_viewed_decayed: 0.0,
            saved_their_content_decayed: 0.0,
            dm_messages_total_decayed: 0.0,
            dm_reactions_given_decayed: 0.0,
            dm_reactions_received_decayed: 0.0,
            likes_given_90d: 0,
            comments_given_90d: 0,
            dm_reactions_given_180d: 0,
            dm_reactions_received_180d: 0,
        }
    }

    fn baseline_cfg() -> ScoringConfig {
        ScoringConfig {
            decay: DecayConfig {
                tau_dm_days: 180,
                tau_content_days: 365,
            },
            weights: WeightsConfig {
                dm: 3.0,
                likes: 1.0,
                comments: 1.5,
                story_out: 1.0,
                stories_viewed: 0.2,
                saved: 0.5,
                reactions_given: 1.0,
                reactions_received: 2.0,
                tenure: 0.3,
                dm_balance_penalty: 1.0,
                reaction_balance_penalty: 0.5,
                hide_story_penalty: 2.0,
                removed_suggestion_penalty: 0.3,
                close_friend_boost: 5.0,
                favorite_boost: 3.0,
                inbound_request: 0.5,
            },
            scoring: ScoringParams {
                threshold: 0.0,
                scale: 1.0,
                keep_min: 0.7,
                unfollow_max: 0.3,
            },
        }
    }

    #[test]
    fn sigmoid_endpoints() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-12);
        assert_eq!(sigmoid(f64::INFINITY), 1.0);
        assert_eq!(sigmoid(f64::NEG_INFINITY), 0.0);
        // Monotonicity at the tails — covered separately for completeness.
        assert!(sigmoid(10.0) > 0.999);
        assert!(sigmoid(-10.0) < 0.001);
    }

    #[test]
    fn empty_account_lands_at_threshold_review() {
        // Nothing populated → score_raw = 0 → keep_prob = sigmoid(0) = 0.5.
        // Bucket boundaries are keep ≥ 0.7 and unfollow < 0.3, so 0.5 is
        // review.
        let cfg = baseline_cfg();
        let acct = baseline_account("ghost");
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored.len(), 1);
        assert!((scored[0].score_raw - 0.0).abs() < 1e-12);
        assert!((scored[0].keep_prob - 0.5).abs() < 1e-12);
        assert_eq!(scored[0].bucket, Bucket::Review);
        assert_eq!(scored[0].dominant_feature, "none");
    }

    #[test]
    fn close_friend_boost_lands_in_keep() {
        let cfg = baseline_cfg();
        let mut acct = baseline_account("bff");
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        // sigmoid(5.0) ≈ 0.9933
        assert!(scored[0].keep_prob > 0.99);
        assert_eq!(scored[0].bucket, Bucket::Keep);
        assert_eq!(scored[0].dominant_feature, "close_friend_boost");
    }

    #[test]
    fn restricted_floors_at_review_even_when_score_says_keep() {
        // A restricted account with strong engagement still floors at
        // review — DESIGN.md's "restricted ⇒ review minimum" rule.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("watch");
        acct.is_restricted = true;
        acct.is_close_friend = true; // would normally drive keep
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(scored[0].keep_prob > 0.99);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn restricted_also_floors_from_below() {
        // An account that would otherwise be `Unfollow` (heavy hide-story
        // penalty, no engagement) gets pushed back up to `Review` if
        // restricted.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("watch");
        acct.is_restricted = true;
        acct.is_hide_story_from = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn boosts_dominate_hide_story_penalty() {
        // `close_friend_boost = 5.0`, `hide_story_penalty = 2.0`. Net
        // +3.0 > 0; with threshold 0, keep_prob = sigmoid(3) ≈ 0.953.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("complicated");
        acct.is_close_friend = true;
        acct.is_hide_story_from = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].score_raw > 0.0,
            "boost must outweigh hide-story penalty: {}",
            scored[0].score_raw,
        );
        assert_eq!(scored[0].bucket, Bucket::Keep);
    }

    #[test]
    fn unfollow_gated_when_favorited() {
        // A favorited account with no signals scores below `unfollow_max`
        // on its own, but the favorite_boost (+3) pushes it well into
        // keep. To exercise the gate cleanly, drag the score below
        // unfollow_max with a stronger penalty and verify the favorite
        // gate still forces `Review`. hide_story_penalty (2.0) +
        // removed_suggestion_penalty (0.3) sum to -2.3; favorite_boost
        // (+3) yields net +0.7, keep_prob ≈ sigmoid(0.7) ≈ 0.668 → review.
        // Bump removed_suggestion via weighting indirection so net is
        // clearly below unfollow_max.
        let mut cfg = baseline_cfg();
        cfg.weights.removed_suggestion_penalty = 10.0; // synthetic — make penalty dominant
        let mut acct = baseline_account("brand");
        acct.is_favorited = true;
        acct.is_hide_story_from = true;
        acct.is_removed_suggestion = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob < cfg.scoring.unfollow_max,
            "score_raw {} keep_prob {} expected < unfollow_max {}",
            scored[0].score_raw,
            scored[0].keep_prob,
            cfg.scoring.unfollow_max,
        );
        // Gated to Review by the favorite flag, not Unfollow.
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn unfollow_when_no_boost_and_low_prob() {
        // hide_story_penalty + removed_suggestion_penalty → strongly
        // negative score; no boost flag → eligible for Unfollow.
        let mut cfg = baseline_cfg();
        cfg.weights.removed_suggestion_penalty = 10.0;
        let mut acct = baseline_account("ghost");
        acct.is_hide_story_from = true;
        acct.is_removed_suggestion = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(scored[0].keep_prob < cfg.scoring.unfollow_max);
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn bucket_boundary_keep_min_is_inclusive() {
        // keep_min = 0.7 → `keep_prob == 0.7` should be Keep. Solve for
        // score_raw such that sigmoid(score_raw) = 0.7:
        // score_raw = ln(0.7 / 0.3) ≈ 0.847.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("edge");
        // Stage score_raw via a single decayed signal: pick likes with
        // weight 1.0, so likes_given_decayed = target.
        let target = (0.7_f64 / 0.3_f64).ln();
        acct.likes_given_decayed = target;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!((scored[0].keep_prob - 0.7).abs() < 1e-9);
        assert_eq!(scored[0].bucket, Bucket::Keep);
    }

    #[test]
    fn bucket_boundary_unfollow_max_is_exclusive() {
        // unfollow_max = 0.3 → `keep_prob == 0.3` should NOT be Unfollow
        // (predicate is `<`). With nothing boosting, predicate-edge
        // accounts land in Review.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("edge");
        let target = (0.3_f64 / 0.7_f64).ln(); // negative ≈ -0.847
        acct.likes_given_decayed = target;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!((scored[0].keep_prob - 0.3).abs() < 1e-9);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn monotonic_in_likes_given_decayed() {
        // More positive-signal mass must produce a higher score_raw,
        // all else equal. Standing in for the broader monotonicity
        // posture across every positive-weighted feature.
        let cfg = baseline_cfg();
        let mut low = baseline_account("a");
        low.likes_given_decayed = 1.0;
        let mut high = baseline_account("b");
        high.likes_given_decayed = 10.0;
        let lo = score(std::slice::from_ref(&low), &cfg)[0].score_raw;
        let hi = score(std::slice::from_ref(&high), &cfg)[0].score_raw;
        assert!(hi > lo, "higher likes must yield higher score: {lo} {hi}");
    }

    #[test]
    fn dm_balance_penalty_volume_gated() {
        // Below the volume floor, `dm_balance = 1.0` (one-sided me)
        // does NOT apply the penalty. Above the floor, it does.
        let cfg = baseline_cfg();

        let mut low_volume = baseline_account("low");
        low_volume.dm_messages_total = MIN_DM_VOLUME_FOR_BALANCE - 1;
        low_volume.dm_balance = Some(1.0);

        let mut high_volume = baseline_account("high");
        high_volume.dm_messages_total = MIN_DM_VOLUME_FOR_BALANCE;
        high_volume.dm_balance = Some(1.0);

        let lo = score(std::slice::from_ref(&low_volume), &cfg)[0].score_raw;
        let hi = score(std::slice::from_ref(&high_volume), &cfg)[0].score_raw;
        assert!(
            lo > hi,
            "high-volume one-sided thread must score worse than \
             low-volume one (penalty kicks in): lo={lo} hi={hi}",
        );
    }

    #[test]
    fn one_sided_them_is_not_a_penalty() {
        // `dm_balance = 0.0` (fully one-sided them) is reciprocity in
        // the inbound direction, not over-extension. Asymmetric penalty.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("they-talk");
        acct.dm_messages_total = 20;
        acct.dm_balance = Some(0.0);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        // No positive engagement either, so score_raw is 0.
        assert!((scored[0].score_raw - 0.0).abs() < 1e-12);
    }

    #[test]
    fn tenure_contributes_logarithmically() {
        // A long-tenure account scores higher than a brand-new one, but
        // the gap is logarithmic — doubling tenure does NOT double the
        // contribution.
        let cfg = baseline_cfg();
        let mut new = baseline_account("new");
        new.follow_tenure_days = Some(30);
        let mut old = baseline_account("old");
        old.follow_tenure_days = Some(3000);
        let lo = score(std::slice::from_ref(&new), &cfg)[0].score_raw;
        let hi = score(std::slice::from_ref(&old), &cfg)[0].score_raw;
        assert!(hi > lo);
        // ln(3001) / ln(31) ≈ 2.33 — verifies the log scaling rather
        // than linear.
        let ratio = hi / lo;
        assert!(
            (1.5..4.0).contains(&ratio),
            "tenure ratio should be in log-scaled range: {ratio}",
        );
    }

    #[test]
    fn brand_class_gates_unfollow_to_review() {
        // A Brand account scoring below `unfollow_max` must downgrade to
        // Review, not Unfollow. DESIGN.md: "Public figures / brands with
        // low `keep_prob` get `review`, never `unfollow`."
        let mut cfg = baseline_cfg();
        cfg.weights.removed_suggestion_penalty = 10.0;
        let mut acct = baseline_account("nytimes_official");
        acct.account_class = AccountClass::Brand;
        acct.is_hide_story_from = true;
        acct.is_removed_suggestion = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob < cfg.scoring.unfollow_max,
            "keep_prob {} should be below unfollow_max {} to exercise the gate",
            scored[0].keep_prob,
            cfg.scoring.unfollow_max,
        );
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn keep_allowlisted_gates_unfollow_to_review() {
        // A personal-classed account on the keep-allowlist must downgrade
        // Unfollow → Review without touching `account_class`. The two
        // gates are parallel signals.
        let mut cfg = baseline_cfg();
        cfg.weights.removed_suggestion_penalty = 10.0;
        let mut acct = baseline_account("sparse_friend");
        acct.is_keep_allowlisted = true;
        acct.is_hide_story_from = true;
        acct.is_removed_suggestion = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(scored[0].keep_prob < cfg.scoring.unfollow_max);
        assert_eq!(scored[0].bucket, Bucket::Review);
        // Class stayed Personal — allowlist is NOT classification.
        assert_eq!(scored[0].features.account_class, AccountClass::Personal);
    }

    #[test]
    fn dominant_feature_picks_signed_max() {
        // A heavy hide-story penalty paired with a modest like signal:
        // the (negative) penalty has larger magnitude → dominant_feature
        // surfaces as the penalty, not the smaller positive term.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("ghost");
        acct.is_hide_story_from = true; // -2.0 contribution
        acct.likes_given_decayed = 0.5; // +0.5 contribution
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].dominant_feature, "hide_story_penalty");
    }
}
