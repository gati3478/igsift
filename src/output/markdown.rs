//! Markdown audit writer — decision-oriented summary.
//!
//! The MD report is designed to be read in three passes:
//!
//! 1. **Unfollow** — every account in the bucket as a full card, since
//!    these are the rows the user is most likely to act on.
//! 2. **Review** — sorted by *decision difficulty* (`|keep_prob - 0.5|`
//!    ascending) so the hardest calls surface first. The top
//!    [`REVIEW_CARDS`] cards carry full rationale; the remainder
//!    collapses to a one-line table.
//! 3. **Keep** — terse: top-N and bottom-N tables only. The saturated
//!    middle (hundreds of `keep_prob ≈ 1.0` accounts) is intentionally
//!    omitted; the user does not need to read it.
//!
//! Each card surfaces the three largest signed score contributions and
//! a one-line decision hint derived from the feature shape. The hint
//! is a small heuristic, not a model — it answers "what shape of
//! account is this?" so the user can pattern-match faster than they
//! could from raw numbers.

use std::cmp::Ordering;
use std::io::Write;

use anyhow::{Context, Result};

use super::csv::profile_url;
use super::{contributions_inline, decision_hint};
use crate::features::AccountFeatures;
use crate::scoring::{Bucket, ScoredAccount};

/// Number of high-rationale cards rendered at the top of the Review
/// section. Rows past this fold render as a one-line table — the user
/// has already seen the hardest calls and the rest is index-style
/// lookup.
const REVIEW_CARDS: usize = 30;
/// Number of rows shown at the top and bottom of the Keep table. The
/// saturated middle is omitted by design.
const KEEP_EDGE_N: usize = 20;

pub fn write_to(scored: &[ScoredAccount], mut writer: impl Write) -> Result<()> {
    let keep_count = scored.iter().filter(|s| s.bucket == Bucket::Keep).count();
    let review_count = scored.iter().filter(|s| s.bucket == Bucket::Review).count();
    let unfollow_count = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Unfollow)
        .count();

    writeln!(writer, "# ig-mgr following audit").context("md header")?;
    writeln!(writer).context("md")?;
    writeln!(writer, "## Summary").context("md")?;
    writeln!(writer).context("md")?;
    writeln!(writer, "- Accounts scored: **{}**", scored.len()).context("md")?;
    writeln!(writer, "- Keep: **{keep_count}**").context("md")?;
    writeln!(writer, "- Review: **{review_count}**").context("md")?;
    writeln!(writer, "- Unfollow: **{unfollow_count}**").context("md")?;
    writeln!(writer).context("md")?;

    write_unfollow_section(&mut writer, scored)?;
    write_review_section(&mut writer, scored)?;
    write_keep_section(&mut writer, scored)?;

    Ok(())
}

fn write_unfollow_section(writer: &mut impl Write, scored: &[ScoredAccount]) -> Result<()> {
    let mut rows: Vec<&ScoredAccount> = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Unfollow)
        .collect();
    // Ascending keep_prob — strongest unfollow signal at the top.
    rows.sort_by(|a, b| {
        a.keep_prob
            .partial_cmp(&b.keep_prob)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });

    writeln!(writer, "## Unfollow ({})", rows.len()).context("md")?;
    writeln!(writer).context("md")?;
    if rows.is_empty() {
        writeln!(writer, "_None — nothing scored low enough._").context("md")?;
        writeln!(writer).context("md")?;
        return Ok(());
    }
    for s in &rows {
        write_card(writer, s)?;
    }
    Ok(())
}

fn write_review_section(writer: &mut impl Write, scored: &[ScoredAccount]) -> Result<()> {
    let mut rows: Vec<&ScoredAccount> = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Review)
        .collect();
    // Decision difficulty: closer to 0.5 = harder call. Surface the
    // hardest decisions first so the user spends their attention there.
    rows.sort_by(|a, b| {
        decision_difficulty(a.keep_prob)
            .partial_cmp(&decision_difficulty(b.keep_prob))
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });

    writeln!(writer, "## Review ({})", rows.len()).context("md")?;
    writeln!(writer).context("md")?;
    writeln!(
        writer,
        "_Sorted by decision difficulty — hardest calls first._"
    )
    .context("md")?;
    writeln!(writer).context("md")?;
    if rows.is_empty() {
        writeln!(writer, "_None._").context("md")?;
        writeln!(writer).context("md")?;
        return Ok(());
    }

    let (cards, tail) = rows.split_at(rows.len().min(REVIEW_CARDS));
    for s in cards {
        write_card(writer, s)?;
    }
    if !tail.is_empty() {
        writeln!(writer, "### Remaining {} (one-line)", tail.len()).context("md")?;
        writeln!(writer).context("md")?;
        write_table(writer, tail.iter().copied())?;
        writeln!(writer).context("md")?;
    }
    Ok(())
}

fn write_keep_section(writer: &mut impl Write, scored: &[ScoredAccount]) -> Result<()> {
    let mut rows: Vec<&ScoredAccount> =
        scored.iter().filter(|s| s.bucket == Bucket::Keep).collect();
    rows.sort_by(|a, b| {
        a.keep_prob
            .partial_cmp(&b.keep_prob)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });

    writeln!(writer, "## Keep ({})", rows.len()).context("md")?;
    writeln!(writer).context("md")?;
    if rows.is_empty() {
        writeln!(writer, "_None._").context("md")?;
        return Ok(());
    }

    if rows.len() <= KEEP_EDGE_N * 2 {
        // Few enough to render in one table — skip the top/bottom split.
        write_table(writer, rows.iter().copied())?;
        return Ok(());
    }

    writeln!(writer, "### Bottom {KEEP_EDGE_N} (close to the boundary)").context("md")?;
    writeln!(writer).context("md")?;
    write_table(writer, rows.iter().take(KEEP_EDGE_N).copied())?;
    writeln!(writer).context("md")?;

    writeln!(writer, "### Top {KEEP_EDGE_N} (highest confidence)").context("md")?;
    writeln!(writer).context("md")?;
    write_table(writer, rows.iter().rev().take(KEEP_EDGE_N).copied())?;
    Ok(())
}

/// Write a per-account "card": handle + link, class + mutual + tenure
/// header line, top-3 score contributions, decision hint.
fn write_card(writer: &mut impl Write, s: &ScoredAccount) -> Result<()> {
    let f = &s.features;
    let display = f.display_name.as_deref().unwrap_or("");
    let display_segment = if display.is_empty() {
        String::new()
    } else {
        format!(" · \"{display}\"")
    };
    writeln!(
        writer,
        "### [`@{handle}`]({url}){display_segment} — keep_prob={prob:.3}",
        handle = f.username,
        url = profile_url(&f.username),
        prob = s.keep_prob,
    )
    .context("md card header")?;

    writeln!(writer, "{}", attribute_line(f)).context("md card attrs")?;
    writeln!(
        writer,
        "- Why: {}",
        contributions_inline(s, "no non-zero terms")
    )
    .context("md card why")?;
    writeln!(writer, "- Hint: _{}_", decision_hint(f, s.bucket)).context("md card hint")?;
    writeln!(writer).context("md")?;
    Ok(())
}

/// Compact attribute line under the card header — class, mutuality,
/// tenure, account-state badges. Skips empties so a personal mutual
/// 4-year follow doesn't render an empty `· · · ·` ladder.
fn attribute_line(f: &AccountFeatures) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(6);
    parts.push(f.account_class.as_str().to_string());
    parts.push(if f.is_mutual {
        "mutual".to_string()
    } else {
        "one-sided".to_string()
    });
    if let Some(d) = f.follow_tenure_days {
        parts.push(format_tenure(d));
    }
    if f.is_close_friend {
        parts.push("close friend".to_string());
    }
    if f.is_favorited {
        parts.push("favorited".to_string());
    }
    if f.is_keep_allowlisted {
        parts.push("allowlisted".to_string());
    }
    if f.is_restricted {
        parts.push("restricted".to_string());
    }
    if f.is_hide_story_from {
        parts.push("story hidden".to_string());
    }
    if f.is_removed_suggestion {
        parts.push("removed suggestion".to_string());
    }
    parts.join(" · ")
}

fn format_tenure(days: u32) -> String {
    if days >= 365 {
        let years = f64::from(days) / 365.25;
        format!("{years:.1}y follow")
    } else {
        format!("{days}d follow")
    }
}

/// `|keep_prob - 0.5|` — smaller is harder to decide. Used as the
/// Review section's sort key so the most ambiguous calls surface first.
fn decision_difficulty(p: f64) -> f64 {
    (p - 0.5).abs()
}

fn write_table<'a, I, W>(writer: &mut W, rows: I) -> Result<()>
where
    I: Iterator<Item = &'a ScoredAccount>,
    W: Write,
{
    writeln!(
        writer,
        "| handle | display name | keep_prob | mutual | dominant |"
    )
    .context("md table header")?;
    writeln!(writer, "|---|---|---|---|---|").context("md table sep")?;
    for s in rows {
        writeln!(
            writer,
            "| [`{handle}`]({url}) | {display} | {prob:.3} | {mutual} | `{dom}` |",
            handle = s.features.username,
            url = profile_url(&s.features.username),
            display = s.features.display_name.as_deref().unwrap_or(""),
            prob = s.keep_prob,
            mutual = if s.features.is_mutual { "yes" } else { "no" },
            dom = s.dominant_feature,
        )
        .context("md table row")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::AccountClass;
    use crate::scoring::Bucket;

    fn baseline_features(handle: &str) -> AccountFeatures {
        AccountFeatures {
            username: handle.to_owned(),
            display_name: None,
            account_class: AccountClass::default(),
            follow_tenure_days: Some(365),
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

    fn make_scored(handle: &str, keep_prob: f64, bucket: Bucket) -> ScoredAccount {
        ScoredAccount {
            features: baseline_features(handle),
            score_raw: 0.0,
            keep_prob,
            bucket,
            dominant_feature: "tenure",
            top_terms: [("tenure", 0.5), ("dm", -0.2), ("likes", -0.1)],
        }
    }

    fn render(scored: &[ScoredAccount]) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write_to(scored, &mut buf).expect("write");
        String::from_utf8(buf).expect("utf-8 md")
    }

    #[test]
    fn empty_input_renders_three_empty_sections() {
        let md = render(&[]);
        assert!(md.contains("Accounts scored: **0**"));
        assert!(md.contains("## Unfollow (0)"));
        assert!(md.contains("## Review (0)"));
        assert!(md.contains("## Keep (0)"));
    }

    #[test]
    fn review_section_sorts_by_decision_difficulty() {
        // Three Review accounts: 0.40 (|Δ|=0.10), 0.49 (|Δ|=0.01), 0.65 (|Δ|=0.15).
        // Hardest first → 0.49, 0.40, 0.65.
        let scored = vec![
            make_scored("far_high", 0.65, Bucket::Review),
            make_scored("close", 0.49, Bucket::Review),
            make_scored("far_low", 0.40, Bucket::Review),
        ];
        let md = render(&scored);
        let pos_close = md.find("`@close`").expect("close present");
        let pos_far_low = md.find("`@far_low`").expect("far_low present");
        let pos_far_high = md.find("`@far_high`").expect("far_high present");
        assert!(
            pos_close < pos_far_low && pos_far_low < pos_far_high,
            "Review must be sorted by |p - 0.5| ascending:\n{md}",
        );
    }

    #[test]
    fn unfollow_card_includes_link_class_and_hint() {
        let scored = vec![make_scored("dormant_acct", 0.20, Bucket::Unfollow)];
        let md = render(&scored);
        assert!(md.contains("[`@dormant_acct`](https://www.instagram.com/dormant_acct/)"));
        assert!(md.contains("personal · one-sided · 1.0y follow"));
        assert!(md.contains("Why: tenure (+0.50), dm (-0.20), likes (-0.10)"));
        assert!(md.contains("Hint:"));
    }

    // Comprehensive decision_hint branch coverage lives in
    // src/output/mod.rs alongside the function itself — single source
    // of truth for the precedence chain.
}
