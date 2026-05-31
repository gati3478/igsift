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

use std::borrow::Cow;
use std::cmp::Ordering;
use std::io::Write;

use anyhow::{Context, Result};

use super::csv::profile_url;
use super::{HINT_ONE_SIDED, contributions_inline, decision_hint};
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

/// Width (in cells) of the ASCII proportion bars in the Summary block.
const BAR_WIDTH: usize = 30;

pub fn write_to(scored: &[ScoredAccount], mut writer: impl Write) -> Result<()> {
    let total = scored.len();
    let keep_count = scored.iter().filter(|s| s.bucket == Bucket::Keep).count();
    let review_count = scored.iter().filter(|s| s.bucket == Bucket::Review).count();
    let unfollow_count = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Unfollow)
        .count();

    let date = jiff::Zoned::now().date();
    writeln!(writer, "# Instagram following audit").context("md header")?;
    writeln!(writer).context("md")?;
    writeln!(writer, "_Generated {date} · igsift_").context("md")?;
    writeln!(writer).context("md")?;
    writeln!(writer, "## Summary").context("md")?;
    writeln!(writer).context("md")?;
    writeln!(writer, "**{total} accounts scored**").context("md")?;
    writeln!(writer).context("md")?;
    // Proportion bars give the keep/review/unfollow split a shape the eye
    // reads at a glance — four flat bullets couldn't. Skipped at total=0 so
    // the bar math never divides by zero (and an empty run has no shape).
    if total > 0 {
        writeln!(writer, "```text").context("md")?;
        for (label, count) in [
            ("Keep", keep_count),
            ("Review", review_count),
            ("Unfollow", unfollow_count),
        ] {
            writeln!(writer, "{}", proportion_line(label, count, total)).context("md")?;
        }
        writeln!(writer, "```").context("md")?;
        writeln!(writer).context("md")?;
    }

    write_unfollow_section(&mut writer, scored)?;
    write_review_section(&mut writer, scored)?;
    write_keep_section(&mut writer, scored)?;

    Ok(())
}

/// `keep_prob ∈ [0, 1]` → an integer percentage for human-facing display.
/// The CSV keeps the raw float for spreadsheet math; cards and tables here
/// read better as `keep 26%` than `keep_prob=0.256`.
fn pct(p: f64) -> u32 {
    (p * 100.0).round() as u32
}

/// One Summary proportion row: `Label ███░░  count  pct%`. `count` is
/// right-aligned so the numbers form a column across the three rows.
fn proportion_line(label: &str, count: usize, total: usize) -> String {
    let share = count as f64 / total as f64;
    let filled = (share * BAR_WIDTH as f64).round() as usize;
    let filled = filled.min(BAR_WIDTH);
    let bar: String = "█".repeat(filled) + &"░".repeat(BAR_WIDTH - filled);
    format!(
        "{label:<9}{bar}  {count:>4}  {}%",
        (share * 100.0).round() as u32
    )
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
    writeln!(writer, "_Strongest unfollow signal first._").context("md")?;
    writeln!(writer).context("md")?;

    // Droplisted accounts land in Unfollow because the user hand-flagged
    // them, not because they scored low — several sit at keep_prob ≈ 1.0.
    // Sorting them into the score-ordered list makes those high scores read
    // like anomalies. Quarantine them under their own subhead so the score
    // column stops appearing to contradict the bucket. Only split when at
    // least one forced row exists; the common (no-droplist) case stays flat.
    let forced: Vec<&ScoredAccount> = rows
        .iter()
        .copied()
        .filter(|s| s.features.is_droplisted)
        .collect();
    if forced.is_empty() {
        for s in &rows {
            write_card(writer, s)?;
        }
        return Ok(());
    }

    let scored_low: Vec<&ScoredAccount> = rows
        .iter()
        .copied()
        .filter(|s| !s.features.is_droplisted)
        .collect();
    writeln!(writer, "### Scored low ({})", scored_low.len()).context("md")?;
    writeln!(writer).context("md")?;
    if scored_low.is_empty() {
        writeln!(
            writer,
            "_None — every Unfollow here was forced by the droplist._"
        )
        .context("md")?;
        writeln!(writer).context("md")?;
    }
    for s in &scored_low {
        write_card(writer, s)?;
    }
    writeln!(writer, "### Forced by droplist ({})", forced.len()).context("md")?;
    writeln!(writer).context("md")?;
    writeln!(
        writer,
        "_These overrode their score — you flagged them in `config/droplist.txt`._"
    )
    .context("md")?;
    writeln!(writer).context("md")?;
    for s in &forced {
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
    let intro = if rows.len() > KEEP_EDGE_N * 2 {
        "_The confident keeps. The saturated middle is omitted — only the boundary and the top are shown._"
    } else {
        "_The confident keeps — closest to the boundary first._"
    };
    writeln!(writer, "{intro}").context("md")?;
    writeln!(writer).context("md")?;

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
    let display = md_escape(f.display_name.as_deref().unwrap_or(""));
    let display_segment = if display.is_empty() {
        String::new()
    } else {
        format!(" · \"{display}\"")
    };
    writeln!(
        writer,
        "### [`@{handle}`]({url}){display_segment} — keep {pct}%",
        handle = f.username,
        url = profile_url(&f.username),
        pct = pct(s.keep_prob),
    )
    .context("md card header")?;

    writeln!(writer, "{}", attribute_line(f)).context("md card attrs")?;
    writeln!(
        writer,
        "- Why: {}",
        contributions_inline(s, "no non-zero terms")
    )
    .context("md card why")?;
    // Suppress the hint when it would only restate the `one-sided` badge
    // already on the attribute line above — pure redundancy on the most
    // common card shape. Every other hint (dormant, droplist, restricted,
    // recent engagement, …) carries information the badges don't, so it stays.
    let hint = decision_hint(f, s.bucket);
    if hint != HINT_ONE_SIDED {
        writeln!(writer, "- Hint: _{hint}_").context("md card hint")?;
    }
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
    if f.is_keeplisted {
        parts.push("keeplisted".to_string());
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

/// Escape the Markdown metacharacters that corrupt a table cell or card
/// line when they appear in a free-form `display_name`: `|` ends a table
/// column and a newline ends the row. Both are realistic in Instagram
/// display names (e.g. `Sarah | Photographer`). Handles are not escaped —
/// Instagram restricts them to `[A-Za-z0-9._]`. Borrows through when the
/// input is already safe.
fn md_escape(s: &str) -> Cow<'_, str> {
    if s.contains(['|', '\n', '\r']) {
        Cow::Owned(s.replace(['\n', '\r'], " ").replace('|', "\\|"))
    } else {
        Cow::Borrowed(s)
    }
}

fn write_table<'a, I, W>(writer: &mut W, rows: I) -> Result<()>
where
    I: Iterator<Item = &'a ScoredAccount>,
    W: Write,
{
    writeln!(
        writer,
        "| handle | display name | keep | mutual | top signal |"
    )
    .context("md table header")?;
    writeln!(writer, "|---|---|---|---|---|").context("md table sep")?;
    for s in rows {
        writeln!(
            writer,
            "| [`{handle}`]({url}) | {display} | {pct}% | {mutual} | {dom} |",
            handle = s.features.username,
            url = profile_url(&s.features.username),
            display = md_escape(s.features.display_name.as_deref().unwrap_or("")),
            pct = pct(s.keep_prob),
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
        // Zero accounts must not panic the proportion bars (no div-by-zero).
        assert!(md.contains("**0 accounts scored**"));
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
    fn unfollow_card_includes_link_class_and_percentage() {
        let scored = vec![make_scored("dormant_acct", 0.20, Bucket::Unfollow)];
        let md = render(&scored);
        assert!(md.contains("[`@dormant_acct`](https://www.instagram.com/dormant_acct/)"));
        // keep_prob is rendered as a human percentage, not a bare float.
        assert!(md.contains("keep 20%"), "{md}");
        assert!(md.contains("personal · one-sided · 1.0y follow"));
        assert!(md.contains("Why: tenure (+0.50), dm (-0.20), likes (-0.10)"));
        // The baseline account is one-sided, so its hint
        // ("one-sided — …") merely restates the `one-sided` badge already
        // on the attribute line and must be suppressed.
        assert!(
            !md.contains("Hint:"),
            "redundant one-sided hint must be suppressed: {md}"
        );
    }

    #[test]
    fn card_shows_non_redundant_hint() {
        // A mutual but dormant account: the "dormant" hint carries info the
        // attribute line (personal · mutual · …) does NOT, so it must render.
        let mut s = make_scored("dormant_mutual", 0.30, Bucket::Unfollow);
        s.features.is_mutual = true;
        let md = render(std::slice::from_ref(&s));
        assert!(
            md.contains("Hint: _dormant"),
            "non-redundant hint must render: {md}",
        );
    }

    // Comprehensive decision_hint branch coverage lives in
    // src/output/mod.rs alongside the function itself — single source
    // of truth for the precedence chain.

    #[test]
    fn pipe_in_display_name_does_not_break_table() {
        // A display name containing `|` would otherwise split the table
        // row into extra columns. It must be escaped so the row keeps its
        // five-column shape.
        let mut scored = make_scored("photog", 0.95, Bucket::Keep);
        scored.features.display_name = Some("Sarah | Photographer".to_owned());
        // Enough Keep rows to force the table path (≤ KEEP_EDGE_N*2 renders
        // a single table directly).
        let md = render(std::slice::from_ref(&scored));
        assert!(
            md.contains("Sarah \\| Photographer"),
            "pipe in display name must be backslash-escaped: {md}",
        );
    }

    #[test]
    fn summary_lists_per_bucket_counts() {
        // The Summary block reports each bucket count via `== Bucket::X`.
        // Asymmetric counts so a `==`→`!=` mutation flips the number.
        let scored = vec![
            make_scored("k1", 0.90, Bucket::Keep),
            make_scored("k2", 0.92, Bucket::Keep),
            make_scored("r1", 0.50, Bucket::Review),
            make_scored("u1", 0.10, Bucket::Unfollow),
        ];
        let md = render(&scored);
        // New Summary: headline + proportion bars carrying count + share.
        // 2 keep / 1 review / 1 unfollow of 4 → 50% / 25% / 25%.
        assert!(md.contains("**4 accounts scored**"), "{md}");
        assert!(md.contains("Keep"), "{md}");
        assert!(md.contains("2  50%"), "{md}");
        assert!(md.contains("1  25%"), "{md}");
    }

    #[test]
    fn droplisted_unfollow_rows_are_quarantined_under_their_own_subhead() {
        // A high-scoring account forced into Unfollow by the droplist must
        // not sit in the score-sorted list looking like a score anomaly.
        // It belongs under a "Forced by droplist" subsection.
        let mut forced = make_scored("forced_high", 0.99, Bucket::Unfollow);
        forced.features.is_droplisted = true;
        let lowscore = make_scored("genuinely_low", 0.15, Bucket::Unfollow);
        let md = render(&[forced, lowscore]);
        assert!(md.contains("### Scored low"), "{md}");
        assert!(md.contains("### Forced by droplist"), "{md}");
        // The forced row appears after the "Forced by droplist" heading,
        // not in the scored-low block.
        let pos_forced_head = md.find("Forced by droplist").expect("subhead");
        let pos_forced_card = md.find("@forced_high").expect("forced card");
        assert!(
            pos_forced_card > pos_forced_head,
            "forced row must be under its subhead: {md}"
        );
    }
}
