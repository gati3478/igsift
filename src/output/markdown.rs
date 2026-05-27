//! Markdown summary writer.
//!
//! Designed for skim-review before opening the CSV. The two tables —
//! lowest-`keep_prob` (unfollow candidates) and highest-`keep_prob`
//! (keep validation) — surface 20 accounts each, with the dominant
//! feature that drove each call so the user can spot misranked
//! accounts at a glance.
//!
//! ## "Candidates" vs. bucket membership
//!
//! The bottom-20 table lists the 20 lowest `keep_prob` accounts
//! regardless of bucket — they are *candidates* for unfollow. Some
//! may have landed in `Review` (close to the cutoff, or boosted by
//! `is_close_friend` / `is_favorited`). The `bucket` column shows
//! the actual call so the user sees both the ranking and the gate.

use std::cmp::Ordering;
use std::io::Write;

use anyhow::{Context, Result};

use super::csv::profile_url;
use crate::scoring::{Bucket, ScoredAccount};

const TOP_N: usize = 20;

pub fn write_to(scored: &[ScoredAccount], mut writer: impl Write) -> Result<()> {
    let keep = scored.iter().filter(|s| s.bucket == Bucket::Keep).count();
    let review = scored.iter().filter(|s| s.bucket == Bucket::Review).count();
    let unfollow = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Unfollow)
        .count();

    let mut by_prob: Vec<&ScoredAccount> = scored.iter().collect();
    // Stable tie-break on handle so accounts at identical keep_prob
    // (saturated boosts produce many 1.000s) render deterministically.
    by_prob.sort_by(|a, b| {
        a.keep_prob
            .partial_cmp(&b.keep_prob)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });

    writeln!(writer, "# ig-mgr following audit").context("md header")?;
    writeln!(writer).context("md")?;
    writeln!(writer, "## Summary").context("md")?;
    writeln!(writer).context("md")?;
    writeln!(writer, "- Accounts scored: **{}**", scored.len()).context("md")?;
    writeln!(writer, "- Keep: **{keep}**").context("md")?;
    writeln!(writer, "- Review: **{review}**").context("md")?;
    writeln!(writer, "- Unfollow: **{unfollow}**").context("md")?;
    writeln!(writer).context("md")?;

    writeln!(writer, "## Bottom {TOP_N} (unfollow candidates)").context("md")?;
    writeln!(writer).context("md")?;
    write_table(&mut writer, by_prob.iter().take(TOP_N))?;
    writeln!(writer).context("md")?;

    writeln!(writer, "## Top {TOP_N} (keep validation)").context("md")?;
    writeln!(writer).context("md")?;
    write_table(&mut writer, by_prob.iter().rev().take(TOP_N))?;

    Ok(())
}

fn write_table<'a, I, W>(writer: &mut W, rows: I) -> Result<()>
where
    I: Iterator<Item = &'a &'a ScoredAccount>,
    W: Write,
{
    writeln!(
        writer,
        "| handle | display name | keep_prob | bucket | dominant |"
    )
    .context("md table header")?;
    writeln!(writer, "|---|---|---|---|---|").context("md table sep")?;
    let mut any = false;
    for s in rows {
        any = true;
        writeln!(
            writer,
            "| [`{handle}`]({url}) | {display} | {prob:.3} | {bucket} | `{dom}` |",
            handle = s.features.username,
            url = profile_url(&s.features.username),
            display = s.features.display_name.as_deref().unwrap_or(""),
            prob = s.keep_prob,
            bucket = s.bucket.as_str(),
            dom = s.dominant_feature,
        )
        .context("md table row")?;
    }
    if !any {
        // Keep the section consistent — a zero-row table is valid GFM
        // but renders awkwardly; emit a placeholder instead.
        writeln!(writer, "| _no accounts_ |  |  |  |  |").context("md placeholder")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_renders_placeholder_tables() {
        let empty: Vec<ScoredAccount> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        write_to(&empty, &mut buf).expect("empty write");
        let md = String::from_utf8(buf).expect("utf-8 md");
        assert!(md.contains("Accounts scored: **0**"));
        assert!(md.contains("Bottom 20"));
        assert!(md.contains("Top 20"));
        assert!(md.contains("_no accounts_"));
    }
}
