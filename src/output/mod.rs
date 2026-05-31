//! Output writers — the audit artifacts the run materializes.
//!
//! Three files per run, sharing a filename stem:
//!
//! - **CSV** (data) — one row per account, sortable / filterable /
//!   diffable in a spreadsheet. Columns per
//!   [`docs/DESIGN.md`](../../docs/DESIGN.md) "Output". The `*_90d` /
//!   `*_180d` columns are raw fixed-window counts for human context —
//!   distinct from the decay-weighted values that drive `keep_prob`.
//! - **Markdown** (skim) — a decision-oriented summary: bucket
//!   counts plus per-account cards in `Unfollow` and `Review`, with
//!   the dominant features and a one-line hint. Built for "decide
//!   whether to open the CSV at all".
//! - **HTML** (browse) — self-contained single-file report with
//!   sortable, filterable per-bucket tables. Built for the "I want
//!   to triage this in a browser" workflow; no server, no JS deps,
//!   double-click to open.
//!
//! ## Ordering
//!
//! The CSV is emitted in **ascending `keep_prob` order** (worst-first).
//! That means the rows the user is most likely to act on — the `Unfollow`
//! and low-end `Review` bucket — surface at the top of the file. The
//! user can sort otherwise in their spreadsheet; this is just the
//! "actionable rows first" default.
//!
//! ## Filenames
//!
//! Default stem: `following-audit_<YYYY-MM-DD>` next to the export
//! directory, with `.csv`, `.md`, and `.html` appended. `--out PATH`
//! overrides; `Path::with_extension` handles either bare-stem
//! (`/tmp/foo`) or extension-bearing (`/tmp/foo.csv`) inputs symmetrically.

mod csv;
mod html;
mod markdown;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::features::{AccountClass, AccountFeatures};
use crate::scoring::{Bucket, ScoredAccount};

/// Presentation threshold (days) for the "long-standing mutual follow"
/// shape in [`decision_hint`]. Mirrors the **default**
/// `scoring.deep_mutual_keep_days` (2 years) so the hint and the
/// deep-mutual keep-floor line up out of the box. It is intentionally a
/// fixed shape descriptor, not the live config value: "a years-deep mutual"
/// is a true characterization regardless of where a user retunes the floor,
/// and keeping it here preserves `decision_hint`'s `(f, bucket)` signature
/// (a pinned SSOT consumed identically by the Markdown and HTML writers).
const LONG_STANDING_MUTUAL_HINT_DAYS: u32 = 730;

/// The one-sided decision hint string. Surfaced as a named const so the
/// Markdown writer can suppress this specific hint when it would merely
/// restate the `one-sided` badge already on the card's attribute line —
/// comparing against this const instead of a copy-pasted literal keeps
/// the two sites from silently drifting apart.
pub(super) const HINT_ONE_SIDED: &str = "one-sided — you follow, no reciprocation";

/// One-line characterization of an account's "shape" — what kind of
/// follow it is and why it landed in the bucket. Lives at the output
/// module level so the Markdown and HTML writers share the same
/// labels (drift would silently surface different reasons for the
/// same account across artifacts).
///
/// Order matters: explicit user signals (keeplist, close friend,
/// favorited) trump inferred behaviour (DM activity, recent likes),
/// which in turn trump structural fallbacks (one-sided, brand,
/// dormant). A regression that reorders these would still pass
/// any single-arm test — the table-driven test in this module
/// pins the precedence chain end to end.
pub(super) fn decision_hint(f: &AccountFeatures, bucket: Bucket) -> &'static str {
    // Most decisive signal: the user explicitly forced Unfollow — but the
    // droplist yields to the restricted floor, so guard on `!is_restricted`
    // to stay consistent with `assign_bucket` (restricted → Review beats
    // drop → Unfollow). Without the guard, a restricted + droplisted
    // account would sit in Review yet report "explicit droplist",
    // contradicting its own bucket. Placement above the keep-signals is
    // intentional and safe: it can't co-occur with the keeplist (rejected
    // at load by ensure_disjoint), and a droplisted close-friend/favorited
    // correctly buckets Unfollow, so the drop hint is the honest one there.
    if f.is_droplisted && !f.is_restricted {
        return "explicit droplist";
    }
    if f.is_keeplisted {
        return "explicit keeplist";
    }
    if f.is_close_friend {
        return "marked close friend";
    }
    if f.is_favorited {
        return "favorited";
    }
    if f.is_restricted {
        return "restricted — kept in Review by floor";
    }
    if f.is_hide_story_from {
        return "story hidden — negative signal";
    }
    if f.dm_messages_total_decayed > 0.0 {
        return "active DM partner";
    }
    if f.likes_given_90d > 0 || f.comments_given_90d > 0 {
        return "engaged with their content in last 90 days";
    }
    if f.dm_messages_total > 0 {
        return "DM history exists but no recent messages";
    }
    // A years-deep reciprocal relationship with no recent engagement — the
    // population the deep-mutual keep-floor lands in Keep. Sits below the
    // engagement arms (an active DM partner is better described as such) and
    // above the structural fallbacks so it doesn't read as generic "tenure".
    if f.is_mutual
        && f.mutual_age_days
            .is_some_and(|d| d >= LONG_STANDING_MUTUAL_HINT_DAYS)
    {
        return "long-standing mutual follow";
    }
    if !f.is_mutual {
        return HINT_ONE_SIDED;
    }
    if matches!(f.account_class, AccountClass::Brand) {
        return "brand follow — review intent";
    }
    match bucket {
        Bucket::Unfollow => "dormant — no interaction signal in any window",
        _ => "tenure-only — no engagement signal",
    }
}

/// Render the top-3 signed score contributions as an inline string —
/// e.g. `tenure (+0.21), dm (-0.18)`. Shared by the Markdown and HTML
/// writers (same drift-prevention rationale as [`decision_hint`]); the
/// two callers differ only in `empty`, the placeholder rendered when no
/// term is non-zero. Zero entries are skipped so an account with a single
/// non-zero term doesn't render trailing `, (+0.00)` slots.
pub(super) fn contributions_inline(s: &ScoredAccount, empty: &str) -> String {
    let parts: Vec<String> = s
        .top_terms
        .iter()
        .filter(|(_, v)| *v != 0.0)
        .map(|(label, v)| format!("{label} ({v:+.2})"))
        .collect();
    if parts.is_empty() {
        empty.to_string()
    } else {
        parts.join(", ")
    }
}

/// Paths the writer produced. Always three: CSV, Markdown, HTML.
#[derive(Debug)]
pub struct WrittenPaths {
    pub csv: PathBuf,
    pub md: PathBuf,
    pub html: PathBuf,
}

/// Write the CSV, Markdown, and HTML artifacts at `<stem>.{csv,md,html}`.
/// Returns the three paths actually written for the caller to surface in
/// the run summary.
pub fn write(scored: &[ScoredAccount], stem: &Path) -> Result<WrittenPaths> {
    let csv_path = stem.with_extension("csv");
    let md_path = stem.with_extension("md");
    let html_path = stem.with_extension("html");

    let csv_file = File::create(&csv_path)
        .with_context(|| format!("creating CSV at {}", csv_path.display()))?;
    csv::write_to(scored, BufWriter::new(csv_file))
        .with_context(|| format!("writing CSV to {}", csv_path.display()))?;

    let md_file =
        File::create(&md_path).with_context(|| format!("creating MD at {}", md_path.display()))?;
    markdown::write_to(scored, BufWriter::new(md_file))
        .with_context(|| format!("writing MD to {}", md_path.display()))?;

    let html_file = File::create(&html_path)
        .with_context(|| format!("creating HTML at {}", html_path.display()))?;
    html::write_to(scored, BufWriter::new(html_file))
        .with_context(|| format!("writing HTML to {}", html_path.display()))?;

    Ok(WrittenPaths {
        csv: csv_path,
        md: md_path,
        html: html_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> AccountFeatures {
        AccountFeatures {
            username: "u".to_owned(),
            display_name: None,
            account_class: AccountClass::Personal,
            follow_tenure_days: Some(365),
            mutual_age_days: None,
            is_close_friend: false,
            is_favorited: false,
            is_blocked: false,
            is_restricted: false,
            is_hide_story_from: false,
            is_removed_suggestion: false,
            recently_unfollowed: false,
            is_mutual: true,
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
        }
    }

    /// Each row builds a feature shape where TWO branches could fire
    /// and asserts the precedence rule chose the higher-priority one.
    /// A regression that reorders the chain trips at least one row.
    #[test]
    fn decision_hint_precedence_chain() {
        struct Case {
            label: &'static str,
            expected: &'static str,
            mutate: fn(&mut AccountFeatures),
            bucket: Bucket,
        }
        let cases = &[
            Case {
                label: "droplist beats keeplist",
                expected: "explicit droplist",
                // is_droplisted + is_keeplisted is load-time-rejected
                // in production (`lists::ensure_disjoint`); the pairing
                // exists here only to pin the hint's source ordering so a
                // reorder mutation trips.
                mutate: |f| {
                    f.is_droplisted = true;
                    f.is_keeplisted = true;
                },
                bucket: Bucket::Unfollow,
            },
            Case {
                // The droplist yields to the restricted floor, exactly as
                // `assign_bucket` does (restricted → Review beats drop →
                // Unfollow). The hint must mirror that: a restricted +
                // droplisted account sits in Review, so reporting
                // "explicit droplist" would contradict its own bucket.
                // This `!is_restricted` guard is the one place the two
                // precedence chains must agree; pin it.
                label: "restricted beats droplist in the hint",
                expected: "restricted — kept in Review by floor",
                mutate: |f| {
                    f.is_droplisted = true;
                    f.is_restricted = true;
                },
                bucket: Bucket::Review,
            },
            Case {
                // Droplist alone (no other flag) → its own hint. Pins that
                // the arm fires without being coupled to any other signal,
                // and keeps the compound `is_droplisted && !is_restricted`
                // condition's mutations caught.
                label: "droplist alone",
                expected: "explicit droplist",
                mutate: |f| {
                    f.is_droplisted = true;
                },
                bucket: Bucket::Unfollow,
            },
            Case {
                label: "keeplist beats close_friend",
                expected: "explicit keeplist",
                mutate: |f| {
                    f.is_keeplisted = true;
                    f.is_close_friend = true;
                },
                bucket: Bucket::Keep,
            },
            Case {
                label: "close_friend beats favorited",
                expected: "marked close friend",
                mutate: |f| {
                    f.is_close_friend = true;
                    f.is_favorited = true;
                },
                bucket: Bucket::Keep,
            },
            Case {
                label: "favorited beats restricted",
                expected: "favorited",
                mutate: |f| {
                    f.is_favorited = true;
                    f.is_restricted = true;
                },
                bucket: Bucket::Review,
            },
            Case {
                label: "restricted beats hide_story",
                expected: "restricted — kept in Review by floor",
                mutate: |f| {
                    f.is_restricted = true;
                    f.is_hide_story_from = true;
                },
                bucket: Bucket::Review,
            },
            Case {
                label: "hide_story beats active DM",
                expected: "story hidden — negative signal",
                mutate: |f| {
                    f.is_hide_story_from = true;
                    f.dm_messages_total_decayed = 5.0;
                },
                bucket: Bucket::Review,
            },
            Case {
                label: "active DM beats recent likes",
                expected: "active DM partner",
                mutate: |f| {
                    f.dm_messages_total_decayed = 5.0;
                    f.likes_given_90d = 10;
                },
                bucket: Bucket::Keep,
            },
            Case {
                label: "recent likes beats old DM history",
                expected: "engaged with their content in last 90 days",
                mutate: |f| {
                    f.likes_given_90d = 1;
                    f.dm_messages_total = 50;
                },
                bucket: Bucket::Keep,
            },
            Case {
                label: "old DM history beats one-sided fallback",
                expected: "DM history exists but no recent messages",
                mutate: |f| {
                    f.dm_messages_total = 1;
                    f.is_mutual = false;
                },
                bucket: Bucket::Review,
            },
            Case {
                // Guard: a mutual account with old DM history still reports
                // the DM-history shape, not the long-standing-mutual shape —
                // the new arm sits below the engagement arms.
                label: "old DM history beats long-standing mutual",
                expected: "DM history exists but no recent messages",
                mutate: |f| {
                    f.dm_messages_total = 1;
                    f.mutual_age_days = Some(3000);
                },
                bucket: Bucket::Keep,
            },
            Case {
                // The new arm: a years-deep mutual with no engagement signal
                // reports its relationship shape instead of falling through
                // to the generic tenure-only tail.
                label: "long-standing mutual beats tenure-only tail",
                expected: "long-standing mutual follow",
                mutate: |f| {
                    f.mutual_age_days = Some(3000); // baseline is_mutual = true
                },
                bucket: Bucket::Keep,
            },
            Case {
                label: "one-sided beats brand-fallback",
                expected: "one-sided — you follow, no reciprocation",
                mutate: |f| {
                    f.is_mutual = false;
                    f.account_class = AccountClass::Brand;
                },
                bucket: Bucket::Keep,
            },
            Case {
                label: "brand beats bucket-tail",
                expected: "brand follow — review intent",
                mutate: |f| {
                    f.account_class = AccountClass::Brand;
                },
                bucket: Bucket::Keep,
            },
            Case {
                label: "Unfollow tail",
                expected: "dormant — no interaction signal in any window",
                mutate: |_| {},
                bucket: Bucket::Unfollow,
            },
            Case {
                label: "non-Unfollow tail",
                expected: "tenure-only — no engagement signal",
                mutate: |_| {},
                bucket: Bucket::Keep,
            },
        ];
        for c in cases {
            let mut f = baseline();
            (c.mutate)(&mut f);
            let got = decision_hint(&f, c.bucket);
            assert_eq!(got, c.expected, "{}: got {got:?}", c.label);
        }
    }

    #[test]
    fn recent_comments_alone_count_as_engaged() {
        // The 90d-engagement branch is `likes_90d > 0 || comments_90d > 0`.
        // The precedence-chain table only fires the likes arm, which
        // short-circuits the comments comparison — so a mutation to the
        // comments side survives. Pin the comments-only arm explicitly.
        let mut f = baseline();
        f.comments_given_90d = 1; // likes_given_90d stays 0
        assert_eq!(
            decision_hint(&f, Bucket::Keep),
            "engaged with their content in last 90 days",
        );
    }

    #[test]
    fn long_standing_mutual_hint_boundary() {
        // The arm fires at `mutual_age_days >= LONG_STANDING_MUTUAL_HINT_DAYS`
        // (inclusive), for a mutual account with no stronger signal. Below
        // the threshold, or with an undatable relationship (None), it falls
        // through to the tenure-only tail. baseline() is is_mutual = true.
        let at = LONG_STANDING_MUTUAL_HINT_DAYS;

        let mut f = baseline();
        f.mutual_age_days = Some(at);
        assert_eq!(
            decision_hint(&f, Bucket::Keep),
            "long-standing mutual follow"
        );

        let mut f = baseline();
        f.mutual_age_days = Some(at - 1);
        assert_eq!(
            decision_hint(&f, Bucket::Keep),
            "tenure-only — no engagement signal"
        );

        let mut f = baseline();
        f.mutual_age_days = None;
        assert_eq!(
            decision_hint(&f, Bucket::Keep),
            "tenure-only — no engagement signal"
        );
    }
}
