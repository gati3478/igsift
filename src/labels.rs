//! Optional labeled-set accuracy report for tuning the scoring config.
//!
//! Hybrid calibration methodology (per DESIGN.md "Open questions"): the
//! manual loop iterates on the live ranking, with this labeled set serving
//! as a held-out accuracy floor. The confusion matrix surfaces "are we
//! improving?" between iterations; the per-account mismatch list answers
//! "which accounts specifically am I getting wrong?" — both inputs for the
//! next weight edit.
//!
//! Format: `config/labels.txt`, one entry per line:
//!
//! ```text
//! # ASCII handle, then `keep` or `drop`
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
    fn as_str(self) -> &'static str {
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
        // Strip end-of-line comments before tokenizing. A `#` mid-line is
        // a comment start — handles are ASCII identifiers and don't
        // contain `#`, so this is unambiguous.
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
    let by_handle: HashMap<&str, &ScoredAccount> = scored
        .iter()
        .map(|s| (s.features.username.as_str(), s))
        .collect();

    let mut confusion: Confusion = [[0; 3]; 2];
    let mut missing: Vec<&str> = Vec::new();
    let mut hard_mismatches: Vec<(&str, Label, &ScoredAccount)> = Vec::new();
    let mut keep_total: u32 = 0;
    let mut drop_total: u32 = 0;

    for (handle, label) in labels.iter() {
        match label {
            Label::Keep => keep_total += 1,
            Label::Drop => drop_total += 1,
        }
        let Some(acct) = by_handle.get(handle) else {
            missing.push(handle);
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
            hard_mismatches.push((handle, label, acct));
        }
    }

    let scored_total = labels.len() - missing.len();
    let agreed = confusion[0][0] + confusion[1][2];
    let agreement_pct = if scored_total == 0 {
        0.0
    } else {
        100.0 * f64::from(agreed) / scored_total as f64
    };

    println!(
        "labels: {} total ({keep_total} keep, {drop_total} drop)",
        labels.len(),
    );
    println!("confusion matrix:");
    println!("                 bucket=keep  bucket=review  bucket=unfollow");
    println!(
        "  label=keep     {:>11}  {:>13}  {:>15}",
        confusion[0][0], confusion[0][1], confusion[0][2],
    );
    println!(
        "  label=drop     {:>11}  {:>13}  {:>15}",
        confusion[1][0], confusion[1][1], confusion[1][2],
    );
    println!(
        "agreement: {agreed}/{scored_total} ({agreement_pct:.1}%)  \
         [label=keep ∩ bucket=keep + label=drop ∩ bucket=unfollow]"
    );

    if !missing.is_empty() {
        println!(
            "labels not in scored set ({}; account may have been blocked or unfollowed):",
            missing.len(),
        );
        for handle in &missing {
            println!("  {handle}");
        }
    }

    if hard_mismatches.is_empty() {
        println!("hard mismatches: none");
    } else {
        println!("hard mismatches ({}):", hard_mismatches.len());
        for (handle, label, acct) in &hard_mismatches {
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
}
