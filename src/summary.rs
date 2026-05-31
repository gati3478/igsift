//! Assembles the `run` dashboard from scored accounts: header banner,
//! bucket panel, keep_prob histogram, side-by-side keep/unfollow cards.
//! Layout only — all styling primitives live in `crate::term_style`, the
//! shared vocabulary with `crate::labels`. Pure given an explicit `Caps`.

use std::fmt::Write as _;

use crate::scoring::{Bucket, ScoredAccount};
use crate::term_style::Caps;

/// One-line run context shown in the header banner.
pub struct RunMeta<'a> {
    pub total: usize,
    pub config_label: &'a str,
    pub date: jiff::civil::Date,
}

/// Print the dashboard to stdout.
pub fn render(scored: &[ScoredAccount], meta: &RunMeta, caps: &Caps) {
    print!("{}", render_to_string(scored, meta, caps));
}

/// Assemble the dashboard as a single string. Separated from `render` so
/// the layout is unit-testable without capturing stdout.
fn render_to_string(scored: &[ScoredAccount], meta: &RunMeta, caps: &Caps) -> String {
    let mut o = String::new();
    let w = caps.width;

    // --- Header banner ---
    let sep = if caps.unicode { "·" } else { "-" };
    let header = format!(
        "{} followings {sep} {} {sep} {}",
        meta.total, meta.config_label, meta.date
    );
    for row in caps.boxed("igsift", &[header], w.min(64)) {
        let _ = writeln!(o, "{}", caps.paint(&row, caps.dim_style()));
    }
    o.push('\n');

    // --- Bucket panel ---
    let (keep, review, unfollow) = bucket_counts(scored);
    let max = keep.max(review).max(unfollow).max(1);
    // Bar shrinks to fit narrow terminals (chrome around the bar is 27 cols:
    // glyph/label/count on the left, two gutters, and ` 100.0%` on the right),
    // capped at 26 so it doesn't sprawl on wide ones.
    let bar_w = caps.width.saturating_sub(27).min(26);
    o.push_str("  Buckets\n");
    for (bucket, label, count) in [
        (Bucket::Keep, "keep", keep),
        (Bucket::Review, "review", review),
        (Bucket::Unfollow, "unfollow", unfollow),
    ] {
        let glyph = caps.paint(caps.bucket_glyph(bucket), caps.bucket_style(bucket));
        let pct = 100.0 * f64::from(count) / (meta.total.max(1) as f64);
        let bar = caps.paint(&caps.bar(count, max, bar_w), caps.bucket_style(bucket));
        let _ = writeln!(o, "  {glyph} {label:<8} {count:>4}  {bar}  {pct:>4.1}%");
    }
    o.push('\n');

    // --- Histogram ---
    o.push_str(&histogram(scored, caps));
    o.push('\n');

    // --- Cards ---
    let mut by_prob: Vec<&ScoredAccount> = scored.iter().collect();
    by_prob.sort_by(|a, b| {
        b.keep_prob
            .partial_cmp(&a.keep_prob)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top: Vec<String> = by_prob.iter().take(10).map(|s| card_row(s)).collect();
    let bottom: Vec<String> = by_prob.iter().rev().take(10).map(|s| card_row(s)).collect();

    let card_w = if w >= 72 { (w - 1) / 2 } else { w.min(40) };
    let left = caps.boxed("Top keeps", &top, card_w);
    let right = caps.boxed("Unfollow candidates", &bottom, card_w);

    if w >= 72 {
        let left_cols = left.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let rows = left.len().max(right.len());
        // `left` and `right` always have equal row count (both `boxed` of a
        // take(10) slice); the `get(i).unwrap_or_default()` + lpad keeps columns
        // aligned if that ever stops holding.
        for i in 0..rows {
            let l = left.get(i).cloned().unwrap_or_default();
            let r = right.get(i).cloned().unwrap_or_default();
            let lpad = left_cols.saturating_sub(l.chars().count());
            let _ = writeln!(o, "{l}{} {r}", " ".repeat(lpad));
        }
    } else {
        for row in left.iter().chain(right.iter()) {
            o.push_str(row);
            o.push('\n');
        }
    }

    o
}

fn bucket_counts(scored: &[ScoredAccount]) -> (u32, u32, u32) {
    let mut keep = 0;
    let mut review = 0;
    let mut unfollow = 0;
    for s in scored {
        match s.bucket {
            Bucket::Keep => keep += 1,
            Bucket::Review => review += 1,
            Bucket::Unfollow => unfollow += 1,
        }
    }
    (keep, review, unfollow)
}

fn card_row(s: &ScoredAccount) -> String {
    format!(
        "{:<20} {:.3}  {}",
        s.features.username, s.keep_prob, s.dominant_feature
    )
}

/// Compact keep_prob histogram. 10 half-open buckets `[i/10,(i+1)/10)`,
/// last inclusive. Leading empty buckets are skipped so the chart starts
/// where the data does. Bars proportional to the fullest bucket.
fn histogram(scored: &[ScoredAccount], caps: &Caps) -> String {
    let mut counts = [0u32; 10];
    for s in scored {
        let idx = ((s.keep_prob * 10.0).floor() as usize).min(9);
        counts[idx] += 1;
    }
    let max = counts.iter().copied().max().unwrap_or(0);
    let first = counts.iter().position(|&c| c > 0).unwrap_or(0);
    // Chrome around the bar is 12 cols (`  0.9  ` prefix + ` 9999` suffix);
    // shrink to fit, capped at 28.
    let bar_w = caps.width.saturating_sub(12).min(28);

    let mut o = String::from("  keep_prob distribution\n");
    for (i, &c) in counts.iter().enumerate().skip(first) {
        let lo = i as f64 / 10.0;
        let bar = caps.bar(c, max, bar_w);
        let _ = writeln!(o, "  {lo:.1}  {bar} {c:>4}");
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::aggregate::fake_features;

    // `dominant_feature` is `&'static str` and `top_terms` is a fixed
    // `[(&'static str, f64); 3]` — see `scoring::ScoredAccount`.
    fn fake_scored(name: &str, prob: f64, bucket: Bucket, dom: &'static str) -> ScoredAccount {
        ScoredAccount {
            features: fake_features(name),
            score_raw: 0.0,
            keep_prob: prob,
            bucket,
            dominant_feature: dom,
            top_terms: [("", 0.0); 3],
        }
    }

    fn sample() -> Vec<ScoredAccount> {
        vec![
            fake_scored("alice", 1.0, Bucket::Keep, "dm"),
            fake_scored("bob", 0.55, Bucket::Review, "tenure"),
            fake_scored("carol", 0.2, Bucket::Unfollow, "tenure"),
        ]
    }

    fn meta() -> RunMeta<'static> {
        RunMeta {
            total: 3,
            config_label: "balanced preset",
            date: jiff::civil::date(2026, 5, 31),
        }
    }

    #[test]
    fn renders_counts_and_titles_no_color() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let out = render_to_string(&sample(), &meta(), &caps);
        assert!(out.contains("balanced preset"));
        assert!(out.contains("keep"));
        assert!(out.contains("Top keeps"));
        assert!(out.contains("Unfollow candidates"));
        assert!(out.contains("alice"));
        assert!(out.contains("carol"));
    }

    #[test]
    fn panels_fit_within_narrow_width() {
        // At the width floor (Caps::detect clamps to 40), every line —
        // banner, bucket panel, histogram, stacked cards — must stay within
        // the budget so nothing soft-wraps and garbles the dashboard.
        let caps = Caps {
            color: false,
            unicode: true,
            width: 40,
        };
        let out = render_to_string(&sample(), &meta(), &caps);
        for line in out.lines() {
            assert!(
                line.chars().count() <= 40,
                "line exceeds width 40 ({} cols): {line:?}",
                line.chars().count(),
            );
        }
    }

    #[test]
    fn no_color_render_is_esc_free() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let out = render_to_string(&sample(), &meta(), &caps);
        assert!(!out.contains('\u{1b}'), "no ESC bytes when color off");
    }

    #[test]
    fn ascii_header_is_pure_ascii() {
        let caps = Caps {
            color: false,
            unicode: false,
            width: 80,
        };
        let out = render_to_string(&sample(), &meta(), &caps);
        // The banner line (first non-empty content) must be ASCII-only —
        // no `·`, no box-drawing, no `…`.
        let banner = out.lines().find(|l| l.contains("followings")).unwrap();
        assert!(
            banner.is_ascii(),
            "ascii-mode banner must be pure ASCII: {banner:?}"
        );
    }

    #[test]
    fn narrow_width_stacks_cards() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 60,
        };
        let out = render_to_string(&sample(), &meta(), &caps);
        let same_line = out
            .lines()
            .any(|l| l.contains("Top keeps") && l.contains("Unfollow candidates"));
        assert!(!same_line, "cards must stack at width 60");
    }
}
