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
//! ## Account-class + keeplist gates
//!
//! DESIGN.md's `Unfollow` bucket requires `account_class == Personal` AND
//! not `is_close_friend`/`is_favorited`/`is_restricted`. Brand accounts
//! detected by [`crate::features::account_class::Classifier`] downgrade
//! Unfollow → Review in [`assign_bucket`]. The user-maintained keep-
//! keeplist (`config/keeplist.txt`) surfaces on
//! [`AccountFeatures::is_keeplisted`] and applies the same
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

/// `true` when an account's entire relationship is one-directional
/// consumption: a personal account you follow that never followed you back,
/// never messaged you, and never reacted to you, with no explicit keep
/// override. Brands/public figures are legitimately one-way follows and are
/// excluded via `account_class`. Drives the reciprocity keep-ceiling gate.
fn is_parasocial(f: &AccountFeatures) -> bool {
    f.account_class == AccountClass::Personal
        && !f.is_mutual
        && !f.is_favorited
        && !f.is_close_friend
        && !f.is_keeplisted
        && !has_inbound_signal(f)
}

/// Any signal that the other party engaged toward you — the inbound directions
/// Instagram ships. A fully one-sided outbound thread (`dm_balance == 1.0`)
/// does **not** count: it is you talking into the void, not reciprocity.
fn has_inbound_signal(f: &AccountFeatures) -> bool {
    f.inbound_dm_request
        || f.dm_reactions_received > 0
        || matches!(f.dm_balance, Some(b) if b < 1.0)
}

/// Owner-side message count in resolved 1:1 threads (post-dedup): every
/// non-shadow message NOT attributed to the other party. Both
/// `dm_messages_total` and `dm_inbound_replies` are shadow-free, so the
/// difference is the owner's contribution PLUS any message with no
/// classifiable sender (a rare export-drift artifact, normally zero). For the
/// evidence guard that over-count is the conservative direction — it can only
/// trip the volume bar slightly early, and a demotion still requires the
/// independent `reply_skew` check, which is computed from the clean
/// outbound/inbound split. `saturating_sub` defends against underflow.
fn dm_out(f: &AccountFeatures) -> u32 {
    f.dm_messages_total.saturating_sub(f.dm_inbound_replies)
}

/// The effort-skew gate only acts where IG gives bidirectional evidence: a
/// thread the owner genuinely invested in. `min_dm_out == 0` is the disable
/// sentinel — NOT an evidence bar of zero (which is always satisfied).
fn effort_skew_has_evidence(f: &AccountFeatures, p: &ScoringParams) -> bool {
    p.effort_skew_min_dm_out > 0 && dm_out(f) >= p.effort_skew_min_dm_out
}

/// Reply skew — an alias for the post-dedup `dm_balance` viewed as `f64`
/// (owner messages / total real messages). `None` when the thread has no
/// classifiable messages. Named at the call site so the gate reads as a skew
/// check; the underlying value is exactly `dm_balance`.
fn reply_skew(f: &AccountFeatures) -> Option<f64> {
    f.dm_balance.map(f64::from)
}

/// Exemptions from the SOFT tier: the user keep-markers (`is_close_friend`,
/// `is_favorited`) and the structural brand classification (`account_class
/// != Personal`). `is_mutual` is deliberately excluded — a non-deep mutual
/// who never replies in a high-volume thread is the target, not an exception.
fn effort_skew_soft_exempt(f: &AccountFeatures) -> bool {
    f.is_close_friend || f.is_favorited || f.account_class != AccountClass::Personal
}

fn assign_bucket(f: &AccountFeatures, keep_prob: f64, p: &ScoringParams) -> Bucket {
    // Restricted floors the bucket at `review`, regardless of `keep_prob`
    // — DESIGN.md treats "restricted" as a manual signal that the
    // account warrants human attention before any automated drop call.
    // This is the one floor the droplist yields to.
    if f.is_restricted {
        return Bucket::Review;
    }
    // Droplist (`config/droplist.txt`) is the user-maintained always-
    // unfollow override — the inverse of the keeplist. It beats
    // `keep_min` and every keep-gate below; only the restricted floor
    // above outranks it. A both-listed handle can't reach here:
    // `lists::ensure_disjoint` rejects it at load. The presentation
    // mirror of this ordering lives in `output::decision_hint` (its
    // droplist rule is guarded on `!is_restricted`); keep the two in sync.
    if f.is_droplisted {
        return Bucket::Unfollow;
    }
    // HARD effort-skew tier: extreme owner-dominated one-sidedness overrides
    // stale IG keep markers (close-friend / favorite / mutual) AND the
    // deep-mutual floor below — but never keeplist (explicit intent) or the
    // restricted floor above. Monotonic: Keep/anything → Review only.
    if !f.is_keeplisted
        && effort_skew_has_evidence(f, p)
        && reply_skew(f).is_some_and(|s| s >= p.effort_skew_hard)
    {
        return Bucket::Review;
    }
    // Deep-mutual keep-floor: a long reciprocal history is a real
    // relationship worth keeping even with no recent engagement. Floors to
    // Keep from any score. `deep_mutual_keep_days == 0` disables it. Sits
    // below the droplist (an explicit drop intent beats a long history) and
    // below `is_restricted` (manual-attention floor wins). Only ever moves an
    // account UP to Keep — it cannot produce an Unfollow.
    if p.deep_mutual_keep_days > 0
        && f.is_mutual
        && f.mutual_age_days
            .is_some_and(|age| age >= p.deep_mutual_keep_days)
    {
        return Bucket::Keep;
    }
    if keep_prob >= p.keep_min {
        // SOFT effort-skew tier: demote an UNMARKED personal Keep that scored
        // on the owner's outbound but draws near-zero real replies back.
        if !f.is_keeplisted
            && !effort_skew_soft_exempt(f)
            && effort_skew_has_evidence(f, p)
            && reply_skew(f).is_some_and(|s| s >= p.effort_skew_soft)
        {
            return Bucket::Review;
        }
        // Reciprocity keep-ceiling: refuse to auto-keep a personal account
        // whose entire relationship is one-directional consumption (you
        // follow them, they don't follow back, never messaged or reacted to
        // you). Demotes Keep → Review only — never produces an Unfollow. The
        // exact mirror of the brand/favorite Unfollow gate below.
        if p.require_reciprocity_for_keep && is_parasocial(f) {
            return Bucket::Review;
        }
        return Bucket::Keep;
    }
    if keep_prob < p.unfollow_max {
        // DESIGN.md gates `unfollow` on `account_class == Personal` AND
        // `!is_close_friend && !is_favorited && !is_restricted`. Restricted
        // is handled above; the remaining gates fold in here. The keep-
        // keeplist (`config/keeplist.txt`) is the user-maintained
        // override for accounts the lexicon can't be trusted on —
        // brands / public figures the heuristic missed, or personal
        // accounts the export under-represents.
        if f.is_close_friend
            || f.is_favorited
            || f.is_keeplisted
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
            mutual_age_days: None,
            is_close_friend: false,
            is_favorited: false,
            is_restricted: false,
            is_hide_story_from: false,
            is_removed_suggestion: false,
            is_mutual: false,
            is_keeplisted: false,
            is_droplisted: false,
            likes_given: 0,
            comments_given: 0,
            story_interactions_out: 0,
            stories_viewed: 0,
            saved_their_content: 0,
            dm_messages_total: 0,
            dm_recency_days: None,
            dm_balance: None,
            dm_inbound_replies: 0,
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
                require_reciprocity_for_keep: true,
                deep_mutual_keep_days: 730,
                effort_skew_min_dm_out: 0,
                effort_skew_soft: 0.85,
                effort_skew_hard: 0.95,
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
        // Mutual so the reciprocity keep-ceiling gate doesn't fire — this
        // test isolates the `keep_min` boundary, not the gate. (mutual_age
        // stays None, so the deep-mutual floor doesn't fire either.)
        acct.is_mutual = true;
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
    fn keeplisted_gates_unfollow_to_review() {
        // A personal-classed account on the keeplist must downgrade
        // Unfollow → Review without touching `account_class`. The two
        // gates are parallel signals.
        let mut cfg = baseline_cfg();
        cfg.weights.removed_suggestion_penalty = 10.0;
        let mut acct = baseline_account("sparse_friend");
        acct.is_keeplisted = true;
        acct.is_hide_story_from = true;
        acct.is_removed_suggestion = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(scored[0].keep_prob < cfg.scoring.unfollow_max);
        assert_eq!(scored[0].bucket, Bucket::Review);
        // Class stayed Personal — keeplist is NOT classification.
        assert_eq!(scored[0].features.account_class, AccountClass::Personal);
    }

    #[test]
    fn droplisted_forces_unfollow_over_keep_signals() {
        // Acceptance test 1: a droplisted account with close-friend boost
        // and keep_prob ≈ 1.0 must still bucket Unfollow — the droplist
        // beats keep_min and every keep-gate.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("doomed");
        acct.is_droplisted = true;
        acct.is_close_friend = true; // would drive keep_prob ≈ 0.993
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob > 0.99,
            "keep_prob {} should be high to prove the droplist overrides it",
            scored[0].keep_prob,
        );
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn droplisted_forces_unfollow_over_brand_gate() {
        // Acceptance test 2: a Brand account would normally floor at Review
        // (the Unfollow gate blocks non-Personal). The droplist overrides
        // that gate and forces Unfollow.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("brand_to_drop");
        acct.account_class = AccountClass::Brand;
        acct.is_droplisted = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn restricted_beats_droplist() {
        // Acceptance test 3: the one exception. `is_restricted` is a hard
        // Review floor that the droplist yields to — restricted means
        // "look at this before any drop call", which outranks a standing
        // drop intent.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("watch_then_maybe_drop");
        acct.is_droplisted = true;
        acct.is_restricted = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
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

    #[test]
    fn bucket_as_str_labels() {
        // The CSV `bucket` column is part of the DESIGN.md output contract;
        // pin the exact strings so a mutation can't silently relabel them.
        assert_eq!(Bucket::Keep.as_str(), "keep");
        assert_eq!(Bucket::Review.as_str(), "review");
        assert_eq!(Bucket::Unfollow.as_str(), "unfollow");
    }

    #[test]
    fn keep_prob_divides_raw_score_by_scale() {
        // keep_prob = sigmoid((score_raw - threshold) / scale). The default
        // scale of 1.0 makes `/scale` and `*scale` indistinguishable; pin
        // the division with a non-unit scale.
        let mut cfg = baseline_cfg();
        cfg.scoring.scale = 2.0;
        let mut acct = baseline_account("scaled");
        acct.likes_given_decayed = 2.0; // score_raw = 2.0 (likes weight 1.0)
        let kp = score(std::slice::from_ref(&acct), &cfg)[0].keep_prob;
        // sigmoid(2.0 / 2.0) = sigmoid(1.0) ≈ 0.7310586; a `*scale`
        // mutation would yield sigmoid(4.0) ≈ 0.982.
        assert!((kp - 0.731_058_578_630_005).abs() < 1e-9, "keep_prob={kp}");
    }

    #[test]
    fn tenure_contribution_is_weight_times_log_days_plus_one() {
        // Pins the exact tenure term `w.tenure * ln(days + 1)`. The existing
        // ratio test tolerates `*`→`+` and `+1`→`-1`/`*1` mutations; an exact
        // value does not.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("tenured");
        acct.follow_tenure_days = Some(99);
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        let expected = 0.3 * 100.0_f64.ln(); // w.tenure(0.3) * ln(99 + 1)
        assert!(
            (score_raw - expected).abs() < 1e-9,
            "score_raw={score_raw} expected={expected}",
        );
    }

    #[test]
    fn hide_story_penalty_is_signed_negative() {
        // A hide-story-only account must score exactly `-hide_story_penalty`.
        // A deleted `-` (penalty applied as a boost) would flip the sign
        // without tripping the bucket-level tests.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("hidden");
        acct.is_hide_story_from = true;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - (-2.0)).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn dm_balance_penalty_magnitude_is_pinned() {
        // `(dm_balance - 0.5).max(0) * 2`, gated on volume. For balance=1.0
        // above the floor the penalty is exactly 1.0 → contribution -1.0.
        // Pins the `-0.5` offset and the `*2` scale, which the sign-only
        // volume-gate test leaves free.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("onesided");
        acct.dm_messages_total = 10; // above MIN_DM_VOLUME_FOR_BALANCE
        acct.dm_balance = Some(1.0); // fully one-sided me
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - (-1.0)).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn reaction_balance_penalty_applies_above_volume_floor() {
        // No test previously exercised reaction_balance_penalty at all.
        // given=6, received=2 → total 8 ≥ floor, balance 0.75 →
        // (0.75-0.5)*2 = 0.5 penalty; weight 0.5 → contribution -0.25.
        // Raw reaction counts don't feed the positive decayed terms.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("reactor");
        acct.dm_reactions_given = 6;
        acct.dm_reactions_received = 2;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - (-0.25)).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn reaction_balance_penalty_floor_is_exclusive_at_threshold() {
        // total == MIN_REACTION_VOLUME_FOR_BALANCE (5) must still apply the
        // penalty (the guard is `total < MIN`, strict). A `<`→`<=`/`==`
        // mutation would suppress it exactly at the boundary.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("boundary");
        acct.dm_reactions_given = 5; // total 5, balance 1.0
        acct.dm_reactions_received = 0;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        // penalty = (1.0-0.5)*2 = 1.0; weight 0.5 → -0.5.
        assert!((score_raw - (-0.5)).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn reaction_balance_penalty_suppressed_below_floor() {
        let cfg = baseline_cfg();
        let mut acct = baseline_account("light");
        acct.dm_reactions_given = 3; // total 3 < floor → no penalty
        acct.dm_reactions_received = 0;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - 0.0).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn keep_prob_equal_to_unfollow_max_is_review_not_unfollow() {
        // The unfollow gate is `keep_prob < unfollow_max` — exclusive. Set
        // unfollow_max to a keep_prob the scorer actually produces so the
        // comparison is an exact equality; a `<`→`<=` mutation would flip
        // this account into Unfollow.
        let mut cfg = baseline_cfg();
        let mut acct = baseline_account("edge");
        acct.is_hide_story_from = true; // score_raw = -2.0
        let kp = score(std::slice::from_ref(&acct), &cfg)[0].keep_prob;
        cfg.scoring.unfollow_max = kp;
        cfg.scoring.keep_min = kp + 0.1;
        let bucket = score(std::slice::from_ref(&acct), &cfg)[0].bucket;
        assert_eq!(
            bucket,
            Bucket::Review,
            "keep_prob == unfollow_max must stay Review (exclusive `<`)",
        );
    }

    // ── reciprocity keep-ceiling gate (Rule 1) ───────────────────────────
    //
    // A personal, non-mutual account with no inbound signal cannot auto-keep
    // on one-directional consumption alone — it floors at Review. The gate
    // can only move keep→review; it never produces Unfollow.

    /// A high-scoring account whose whole relationship is one-way consumption.
    fn parasocial_keeper(handle: &str) -> AccountFeatures {
        let mut a = baseline_account(handle);
        a.likes_given_decayed = 5.0; // score_raw 5 → keep_prob ≈ 0.993 → keep
        a // Personal, !mutual, no inbound, not favorited/close/keeplisted
    }

    #[test]
    fn reciprocity_gate_floors_nonmutual_personal_to_review() {
        let cfg = baseline_cfg();
        let acct = parasocial_keeper("consumed");
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob >= cfg.scoring.keep_min,
            "must score into keep first"
        );
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    /// Bucket of a parasocial keeper after applying one mutation — used to
    /// check that any single relationship signal exempts it from the gate.
    fn parasocial_keeper_bucket(apply: impl FnOnce(&mut AccountFeatures)) -> Bucket {
        let cfg = baseline_cfg();
        let mut acct = parasocial_keeper("x");
        apply(&mut acct);
        score(std::slice::from_ref(&acct), &cfg)[0].bucket
    }

    #[test]
    fn reciprocity_gate_exempts_each_relationship_signal() {
        // Each signal alone must keep the otherwise-parasocial account in Keep.
        use Bucket::Keep;
        assert_eq!(
            parasocial_keeper_bucket(|a| a.is_mutual = true),
            Keep,
            "mutual"
        );
        assert_eq!(
            parasocial_keeper_bucket(|a| a.account_class = AccountClass::Brand),
            Keep,
            "brand",
        );
        assert_eq!(
            parasocial_keeper_bucket(|a| a.is_favorited = true),
            Keep,
            "favorited"
        );
        assert_eq!(
            parasocial_keeper_bucket(|a| a.is_close_friend = true),
            Keep,
            "close_friend",
        );
        assert_eq!(
            parasocial_keeper_bucket(|a| a.is_keeplisted = true),
            Keep,
            "keeplisted",
        );
        assert_eq!(
            parasocial_keeper_bucket(|a| a.dm_reactions_received = 1),
            Keep,
            "inbound_reaction",
        );
        assert_eq!(
            parasocial_keeper_bucket(|a| {
                a.dm_messages_total = 10;
                a.dm_balance = Some(0.5);
            }),
            Keep,
            "two_way_dm",
        );
    }

    #[test]
    fn one_sided_outbound_thread_does_not_exempt() {
        // dm_balance == 1.0 (everything I sent, nothing back) is NOT
        // reciprocity — talking into the void. Still gated to Review.
        let cfg = baseline_cfg();
        let mut acct = parasocial_keeper("voidshout");
        acct.dm_messages_total = 50;
        acct.dm_balance = Some(1.0);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn reciprocity_gate_disabled_keeps_nonmutual_personal() {
        let mut cfg = baseline_cfg();
        cfg.scoring.require_reciprocity_for_keep = false;
        let acct = parasocial_keeper("consumed");
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Keep);
    }

    // ── deep-mutual keep-floor (Rule 2) ───────────────────────────────────
    //
    // A mutual account with a long reciprocal history floors at Keep even
    // from a low score. Can only move up to Keep; never produces Unfollow.

    /// Mutual, neutral score (0.5 → Review without the floor).
    fn old_mutual(handle: &str, age: Option<u32>) -> AccountFeatures {
        let mut a = baseline_account(handle);
        a.is_mutual = true;
        a.mutual_age_days = age;
        a
    }

    #[test]
    fn deep_mutual_floor_keeps_low_scorer() {
        let cfg = baseline_cfg(); // deep_mutual_keep_days = 730
        let acct = old_mutual("longtime", Some(800));
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob < cfg.scoring.keep_min,
            "score alone is below keep"
        );
        assert_eq!(scored[0].bucket, Bucket::Keep);
    }

    #[test]
    fn deep_mutual_floor_boundary_is_inclusive() {
        let cfg = baseline_cfg();
        let acct = old_mutual("edge", Some(cfg.scoring.deep_mutual_keep_days));
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Keep,
            "age == threshold must keep (>=)"
        );
    }

    #[test]
    fn deep_mutual_floor_below_threshold_does_not_fire() {
        let cfg = baseline_cfg();
        let acct = old_mutual("recent", Some(cfg.scoring.deep_mutual_keep_days - 1));
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn deep_mutual_floor_none_age_does_not_fire() {
        let cfg = baseline_cfg();
        let acct = old_mutual("undatable", None);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn deep_mutual_floor_requires_mutual_flag() {
        // Defensive: even with an old age, a non-mutual account is not floored.
        let cfg = baseline_cfg();
        let mut acct = baseline_account("nonmutual");
        acct.is_mutual = false;
        acct.mutual_age_days = Some(2000);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_ne!(scored[0].bucket, Bucket::Keep);
    }

    #[test]
    fn deep_mutual_floor_disabled_when_zero() {
        let mut cfg = baseline_cfg();
        cfg.scoring.deep_mutual_keep_days = 0;
        let acct = old_mutual("longtime", Some(5000));
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review, "0 disables the floor");
    }

    #[test]
    fn droplist_beats_deep_mutual_floor() {
        let cfg = baseline_cfg();
        let mut acct = old_mutual("doomed", Some(5000));
        acct.is_droplisted = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn restricted_beats_deep_mutual_floor() {
        let cfg = baseline_cfg();
        let mut acct = old_mutual("watch", Some(5000));
        acct.is_restricted = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    // ── effort-skew gate (Rule 3) ─────────────────────────────────────────
    //
    // Two-tier monotonic gate: when the owner sent many more messages than
    // they received replies, a high-scoring Keep is demoted to Review.
    // SOFT tier (>= soft, < hard): exempt IG keep markers (close_friend,
    // favorited, non-Personal). HARD tier (>= hard): overrides those
    // markers. Neither tier yields Unfollow; keeplist is never overridden.
    // Evidence guard (min_dm_out > 0) prevents firing on thin threads;
    // min_dm_out == 0 disables the gate entirely.

    /// A Keep-scoring account with a high-volume, owner-dominated DM thread:
    /// 10 owner messages, 1 real reply → reply_skew (dm_balance) per `balance`,
    /// my_dm_out = 9. Scores into Keep on outbound likes.
    fn skewed_keeper(handle: &str, balance: f32) -> AccountFeatures {
        let mut a = baseline_account(handle);
        a.likes_given_decayed = 5.0; // keep_prob ≈ 0.97
        a.dm_messages_total = 10;
        a.dm_inbound_replies = 1; // my_dm_out = 10 - 1 = 9
        a.dm_balance = Some(balance);
        a
    }

    fn skew_cfg() -> ScoringConfig {
        let mut cfg = baseline_cfg();
        cfg.scoring.effort_skew_min_dm_out = 8;
        cfg.scoring.effort_skew_soft = 0.85;
        cfg.scoring.effort_skew_hard = 0.95;
        cfg
    }

    #[test]
    fn soft_tier_demotes_unmarked_skewed_keeper() {
        let cfg = skew_cfg();
        let acct = skewed_keeper("talker", 0.90); // soft ≤ 0.90 < hard
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob >= cfg.scoring.keep_min,
            "scores into Keep first"
        );
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn soft_tier_respects_close_friend() {
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("bff", 0.90);
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Keep,
            "soft_exempt: close friend stays Keep"
        );
    }

    #[test]
    fn soft_tier_demotes_non_deep_mutual() {
        // Mutual is NOT in soft_exempt — a follow-back who never replies in a
        // high-volume thread is the target. (Not deep: mutual_age stays None.)
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("mutual_ghost", 0.90);
        acct.is_mutual = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn hard_tier_demotes_close_friend() {
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("ghost_bff", 0.97); // ≥ hard
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Review,
            "hard tier overrides close friend"
        );
    }

    #[test]
    fn hard_tier_beats_deep_mutual_floor() {
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("old_ghost", 0.97);
        acct.is_mutual = true;
        acct.mutual_age_days = Some(3000); // would floor to Keep at rung 4
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Review,
            "hard tier sits above deep-mutual floor"
        );
    }

    #[test]
    fn keeplist_survives_both_tiers() {
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("kept", 0.99);
        acct.is_keeplisted = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Keep,
            "keeplist is explicit intent, never overridden by skew"
        );
    }

    #[test]
    fn evidence_guard_blocks_thin_thread() {
        // reply_skew is extreme but my_dm_out (5) is below min_dm_out (8):
        // no evidence → no demotion.
        let cfg = skew_cfg();
        let mut acct = baseline_account("thin");
        acct.likes_given_decayed = 5.0;
        acct.dm_messages_total = 6;
        acct.dm_inbound_replies = 1; // my_dm_out = 5 < 8
        acct.dm_balance = Some(0.99);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Keep,
            "below evidence bar → gate cannot fire"
        );
    }

    #[test]
    fn gate_disabled_when_min_is_zero() {
        // The sentinel: min_dm_out == 0 must NOT fire the gate on every thread.
        let cfg = baseline_cfg(); // effort_skew_min_dm_out = 0
        let acct = skewed_keeper("loud", 0.99);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Keep,
            "min_dm_out=0 disables the gate"
        );
    }

    #[test]
    fn gate_never_yields_unfollow() {
        // Monotonicity: a LOW-scoring unmarked personal account (keep_prob
        // below unfollow_max → would be Unfollow) that is also high-skew with
        // evidence is intercepted by the HARD tier ABOVE the unfollow branch
        // and lands in Review — the gate never *produces* Unfollow.
        let cfg = skew_cfg();
        let mut acct = baseline_account("loud_lowscore");
        acct.is_hide_story_from = true; // score_raw -2.0 → keep_prob ≈ 0.12 < unfollow_max
        acct.dm_messages_total = 10;
        acct.dm_inbound_replies = 1; // my_dm_out = 9 ≥ 8 (evidence)
        acct.dm_balance = Some(0.99); // ≥ effort_skew_hard
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob < cfg.scoring.unfollow_max,
            "must score into the Unfollow range without the gate: {}",
            scored[0].keep_prob,
        );
        assert_eq!(
            scored[0].bucket,
            Bucket::Review,
            "gate yields Review, never Unfollow"
        );

        // Control: identical account, gate disabled → reaches its natural
        // Unfollow. Proves the gate caused the Review above (didn't coincide)
        // and confirms the gate is what stands between this account and Unfollow.
        let off = baseline_cfg(); // effort_skew_min_dm_out = 0 (gate off), same threshold
        let scored_off = score(std::slice::from_ref(&acct), &off);
        assert_eq!(
            scored_off[0].bucket,
            Bucket::Unfollow,
            "gate off → natural Unfollow"
        );
    }

    #[test]
    fn soft_tier_boundary_is_inclusive() {
        // 0.5 is exactly representable in both f32 and f64, so
        // f64::from(dm_balance) == effort_skew_soft holds precisely — this
        // pins the `>=` boundary at true equality, not soft+ε from an f32 cast.
        let mut cfg = skew_cfg();
        cfg.scoring.effort_skew_soft = 0.5;
        let acct = skewed_keeper("edge", 0.5); // skew 0.5 == soft, < hard (no HARD fire)
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Review,
            "reply_skew == soft must demote (>=)"
        );
    }
}
