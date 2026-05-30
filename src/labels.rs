//! Optional labeled-set accuracy report for tuning the scoring config.
//!
//! Hybrid calibration methodology (per DESIGN.md "Open questions"): the
//! manual loop iterates on the live ranking, with this labeled set serving
//! as a held-out accuracy floor. The confusion matrix surfaces "are we
//! improving?" between iterations; the per-account mismatch list answers
//! "which accounts specifically am I getting wrong?" — both inputs for the
//! next weight edit.
//!
//! Format: `config/labels.txt`, one entry per line — a handle, whitespace,
//! then `keep` or `drop`:
//!
//! ```text
//! alice_synth   keep
//! brand_xyz     drop
//! ```
//!
//! `#` introduces a comment to end-of-line; blank lines are ignored.
//! Duplicate handles are a HARD parse error — silently deduping would
//! hide a typo that changes scoring semantics. Labels that don't match
//! any aggregated account surface as a warning in the report (not a
//! parse error — the user might have labeled an account that was blocked
//! or unfollowed since).
//!
//! The file is **never** materialized by the binary; the user maintains
//! it by hand. Auto-detect (`config/labels.txt` exists → report runs)
//! means a tuning session that hasn't committed labels yet sees no
//! report and no error — the file is opt-in.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::scoring::{Bucket, ScoredAccount};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// User has confirmed: this account belongs in the `keep` bucket.
    Keep,
    /// User has confirmed: this account belongs in the `unfollow` bucket.
    /// The `review` bucket is intentionally not labelable — `review` is
    /// "I haven't decided", and a label means "I have decided".
    Drop,
}

impl Label {
    pub fn as_str(self) -> &'static str {
        match self {
            Label::Keep => "keep",
            Label::Drop => "drop",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LabelSet {
    by_handle: HashMap<String, Label>,
}

impl LabelSet {
    pub fn len(&self) -> usize {
        self.by_handle.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_handle.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Label)> {
        self.by_handle.iter().map(|(k, v)| (k.as_str(), *v))
    }
}

/// Read `config/labels.txt` if it exists. `Ok(None)` is the "no labels
/// committed yet" case — the report is opt-in, not a missing-file error.
pub fn load_default() -> Result<Option<LabelSet>> {
    let path = Path::new("config/labels.txt");
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(load(path)?))
}

/// Parse a labels file at an arbitrary path. Used by tests and by the
/// implicit `config/labels.txt` lookup.
pub fn load(path: &Path) -> Result<LabelSet> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("reading labels file from {}", path.display()))?;
    parse(&body, &path.display().to_string())
}

fn parse(body: &str, source: &str) -> Result<LabelSet> {
    let mut by_handle: HashMap<String, Label> = HashMap::new();
    for (idx, raw) in body.lines().enumerate() {
        let line_no = idx + 1;
        // Strip end-of-line comments before tokenizing. `#` is a comment
        // delimiter, so by construction no handle contains it — the split
        // is unambiguous regardless of script (Instagram handles are
        // ASCII in practice, but the parser doesn't lean on that).
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let handle = parts.next().ok_or_else(|| {
            anyhow::anyhow!("{source}:{line_no}: empty entry after comment strip")
        })?;
        let label_raw = parts.next().ok_or_else(|| {
            anyhow::anyhow!(
                "{source}:{line_no}: handle {handle:?} has no label (expected `keep` or `drop`)"
            )
        })?;
        if parts.next().is_some() {
            bail!(
                "{source}:{line_no}: extra tokens after `{handle} {label_raw}` \
                 (handles must not contain whitespace)"
            );
        }
        let label = match label_raw {
            "keep" => Label::Keep,
            "drop" => Label::Drop,
            other => bail!(
                "{source}:{line_no}: unknown label {other:?} for {handle:?} \
                 (expected `keep` or `drop`)"
            ),
        };
        if let Some(prior) = by_handle.insert(handle.to_owned(), label) {
            bail!(
                "{source}:{line_no}: duplicate handle {handle:?} \
                 (previously labeled {prior:?}, now {label:?}) — \
                 dedup is the user's call, not the loader's",
                prior = prior.as_str(),
                label = label.as_str(),
            );
        }
    }
    Ok(LabelSet { by_handle })
}

/// Confusion matrix counts, indexed `[label][bucket]`. Labels in row order
/// `[Keep, Drop]`; buckets in column order `[Keep, Review, Unfollow]`.
type Confusion = [[u32; 3]; 2];

fn bucket_col(b: Bucket) -> usize {
    match b {
        Bucket::Keep => 0,
        Bucket::Review => 1,
        Bucket::Unfollow => 2,
    }
}

/// Computed accuracy-report data, separated from printing so the matrix
/// and agreement arithmetic are unit-testable — [`report`] only formats
/// this to stdout. `missing` and `hard_mismatches` are sorted by handle
/// for stable round-over-round diffs.
#[derive(Debug, PartialEq)]
pub(crate) struct ReportData {
    confusion: Confusion,
    keep_total: u32,
    drop_total: u32,
    missing: Vec<String>,
    hard_mismatches: Vec<(String, Label)>,
    agreed: u32,
    scored_total: usize,
    agreement_pct: f64,
}

/// Tally the confusion matrix, agreement, missing labels, and hard
/// mismatches of `labels` against the `scored` set. Pure — no I/O.
///
/// Hard mismatches are the two unambiguous disagreements
/// (`label=keep & bucket=unfollow`, `label=drop & bucket=keep`); soft
/// mismatches against `review` are visible in the matrix but not listed,
/// since `review` is an honest "unsure" outcome.
pub(crate) fn compute(labels: &LabelSet, scored: &[ScoredAccount]) -> ReportData {
    let by_handle: HashMap<&str, &ScoredAccount> = scored
        .iter()
        .map(|s| (s.features.username.as_str(), s))
        .collect();

    let mut confusion: Confusion = [[0; 3]; 2];
    let mut missing: Vec<String> = Vec::new();
    let mut hard_mismatches: Vec<(String, Label)> = Vec::new();
    let mut keep_total: u32 = 0;
    let mut drop_total: u32 = 0;

    for (handle, label) in labels.iter() {
        match label {
            Label::Keep => keep_total += 1,
            Label::Drop => drop_total += 1,
        }
        let Some(acct) = by_handle.get(handle) else {
            missing.push(handle.to_owned());
            continue;
        };
        let row = match label {
            Label::Keep => 0,
            Label::Drop => 1,
        };
        confusion[row][bucket_col(acct.bucket)] += 1;
        let hard = matches!(
            (label, acct.bucket),
            (Label::Keep, Bucket::Unfollow) | (Label::Drop, Bucket::Keep)
        );
        if hard {
            hard_mismatches.push((handle.to_owned(), label));
        }
    }

    let scored_total = labels.len() - missing.len();
    let agreed = confusion[0][0] + confusion[1][2];
    let agreement_pct = if scored_total == 0 {
        0.0
    } else {
        100.0 * f64::from(agreed) / scored_total as f64
    };

    // Sorted so round-over-round diffs only show real changes (HashMap
    // iteration order is randomized per-process via `RandomState`).
    missing.sort_unstable();
    hard_mismatches.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    ReportData {
        confusion,
        keep_total,
        drop_total,
        missing,
        hard_mismatches,
        agreed,
        scored_total,
        agreement_pct,
    }
}

/// Print the accuracy report to stdout. Matches the style of the other
/// `lib::run` smoke lines so the tuning iteration loop reads as one
/// continuous report.
///
/// - Header line with label totals (`N labels: K keep, D drop`).
/// - Confusion matrix (2 × 3).
/// - Agreement ratio.
/// - "Labels not in scored set" warnings (one per missing handle).
/// - Hard mismatches: `label=keep & bucket=unfollow` and
///   `label=drop & bucket=keep`. Soft mismatches (label=keep & bucket=review,
///   label=drop & bucket=review) are visible in the matrix but not listed —
///   `review` is an honest "I'm unsure" outcome.
pub fn report(labels: &LabelSet, scored: &[ScoredAccount]) {
    let data = compute(labels, scored);
    let by_handle: HashMap<&str, &ScoredAccount> = scored
        .iter()
        .map(|s| (s.features.username.as_str(), s))
        .collect();

    println!(
        "labels: {} total ({} keep, {} drop)",
        labels.len(),
        data.keep_total,
        data.drop_total,
    );
    println!("confusion matrix:");
    println!("                 bucket=keep  bucket=review  bucket=unfollow");
    println!(
        "  label=keep     {:>11}  {:>13}  {:>15}",
        data.confusion[0][0], data.confusion[0][1], data.confusion[0][2],
    );
    println!(
        "  label=drop     {:>11}  {:>13}  {:>15}",
        data.confusion[1][0], data.confusion[1][1], data.confusion[1][2],
    );
    println!(
        "agreement: {}/{} ({:.1}%)  \
         [label=keep ∩ bucket=keep + label=drop ∩ bucket=unfollow]",
        data.agreed, data.scored_total, data.agreement_pct,
    );

    if !data.missing.is_empty() {
        println!(
            "labels not in scored set ({}; account may have been blocked or unfollowed):",
            data.missing.len(),
        );
        for handle in &data.missing {
            println!("  {handle}");
        }
    }

    if data.hard_mismatches.is_empty() {
        println!("hard mismatches: none");
    } else {
        println!("hard mismatches ({}):", data.hard_mismatches.len());
        for (handle, label) in &data.hard_mismatches {
            let acct = by_handle[handle.as_str()];
            println!(
                "  {handle}  label={}  keep_prob={:.3}  bucket={}  dominant={}",
                label.as_str(),
                acct.keep_prob,
                acct.bucket.as_str(),
                acct.dominant_feature,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    //! Pin the parser's behavior on every edge the format spec calls out:
    //! comments, blank lines, malformed lines, unknown labels, and the
    //! load-bearing duplicate-handle rejection.
    use super::*;

    fn parse_ok(body: &str) -> LabelSet {
        parse(body, "/synthetic.txt").expect("parse must succeed")
    }

    fn parse_err(body: &str) -> String {
        let err = parse(body, "/synthetic.txt").expect_err("parse must fail");
        format!("{err:#}")
    }

    #[test]
    fn empty_body_is_empty_set() {
        let labels = parse_ok("");
        assert!(labels.is_empty());
    }

    #[test]
    fn comments_and_blanks_are_skipped() {
        let labels = parse_ok(
            "# header comment\n\n  \n\
             alice  keep   # trailing comment\n\
             bob drop\n\
             # tail comment\n",
        );
        assert_eq!(labels.len(), 2);
        let map: HashMap<&str, Label> = labels.iter().collect();
        assert_eq!(map.get("alice"), Some(&Label::Keep));
        assert_eq!(map.get("bob"), Some(&Label::Drop));
    }

    #[test]
    fn unknown_label_names_offender() {
        let msg = parse_err("alice maybe\n");
        assert!(
            msg.contains("maybe"),
            "error must name the bad label: {msg}"
        );
        assert!(msg.contains("alice"), "error must name the handle: {msg}");
    }

    #[test]
    fn missing_label_is_an_error() {
        let msg = parse_err("alice\n");
        assert!(
            msg.contains("no label") || msg.contains("expected"),
            "error must point at the missing label: {msg}",
        );
    }

    #[test]
    fn duplicate_handle_is_rejected() {
        // Silent dedup would hide a typo that changes scoring semantics —
        // the loader must surface the conflict and let the user fix it.
        let msg = parse_err("alice keep\nalice drop\n");
        assert!(msg.contains("duplicate"), "error must mention dup: {msg}");
        assert!(msg.contains("alice"), "error must name the handle: {msg}");
    }

    #[test]
    fn extra_tokens_are_rejected() {
        let msg = parse_err("alice keep extra-token\n");
        assert!(
            msg.contains("extra") || msg.contains("whitespace"),
            "error must flag extra tokens: {msg}",
        );
    }

    #[test]
    fn line_number_surfaces_in_errors() {
        // Parse errors point at the offending line for fast triage —
        // a labels file with hundreds of entries is unreadable without it.
        let msg = parse_err("alice keep\n\nbob bogus\n");
        assert!(msg.contains(":3:"), "error must include line 3: {msg}");
    }

    #[test]
    fn non_empty_set_is_not_empty() {
        let labels = parse_ok("alice keep\n");
        assert!(!labels.is_empty());
        assert_eq!(labels.len(), 1);
    }

    #[test]
    fn label_as_str_values() {
        assert_eq!(Label::Keep.as_str(), "keep");
        assert_eq!(Label::Drop.as_str(), "drop");
    }

    fn scored_acct(handle: &str, bucket: Bucket) -> ScoredAccount {
        use crate::features::{AccountClass, AccountFeatures};
        ScoredAccount {
            features: AccountFeatures {
                username: handle.to_owned(),
                display_name: None,
                account_class: AccountClass::Personal,
                follow_tenure_days: None,
                mutual_age_days: None,
                is_close_friend: false,
                is_favorited: false,
                is_blocked: false,
                is_restricted: false,
                is_hide_story_from: false,
                is_removed_suggestion: false,
                recently_unfollowed: false,
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
            },
            score_raw: 0.0,
            keep_prob: 0.0,
            bucket,
            dominant_feature: "none",
            top_terms: [("", 0.0); 3],
        }
    }

    #[test]
    fn compute_tallies_matrix_agreement_and_mismatches() {
        let labels = parse_ok(
            "agree_keep keep\n\
             agree_drop drop\n\
             soft_keep keep\n\
             hard_keep keep\n\
             missing_one drop\n",
        );
        let scored = vec![
            scored_acct("agree_keep", Bucket::Keep), // keep∩keep → agree
            scored_acct("agree_drop", Bucket::Unfollow), // drop∩unfollow → agree
            scored_acct("soft_keep", Bucket::Review), // keep∩review → soft, not hard
            scored_acct("hard_keep", Bucket::Unfollow), // keep∩unfollow → HARD
                                                     // `missing_one` deliberately absent from the scored set.
        ];
        let data = compute(&labels, &scored);

        assert_eq!(data.keep_total, 3);
        assert_eq!(data.drop_total, 2);
        assert_eq!(data.missing, vec!["missing_one".to_owned()]);
        assert_eq!(data.scored_total, 4, "5 labels − 1 missing");

        // confusion[label][bucket]; label keep=0/drop=1, bucket keep=0/review=1/unfollow=2.
        assert_eq!(data.confusion[0][0], 1, "keep∩keep");
        assert_eq!(data.confusion[0][1], 1, "keep∩review");
        assert_eq!(data.confusion[0][2], 1, "keep∩unfollow");
        assert_eq!(data.confusion[1][2], 1, "drop∩unfollow");

        assert_eq!(data.agreed, 2, "keep∩keep + drop∩unfollow");
        assert!((data.agreement_pct - 50.0).abs() < 1e-9, "2/4 = 50%");
        assert_eq!(
            data.hard_mismatches,
            vec![("hard_keep".to_owned(), Label::Keep)],
        );
    }

    #[test]
    fn compute_empty_scored_set_is_zero_agreement_not_nan() {
        // Guards the `scored_total == 0` branch — without it the agreement
        // ratio divides by zero and prints NaN.
        let labels = parse_ok("a keep\nb drop\n");
        let data = compute(&labels, &[]);
        assert_eq!(data.scored_total, 0);
        assert_eq!(data.agreed, 0);
        assert_eq!(data.agreement_pct, 0.0);
    }
}
