//! Self-contained HTML audit report.
//!
//! Single file, no external assets, no server: inline CSS, vanilla JS,
//! double-click to open in a browser. The data is exactly what the
//! Markdown report has, presented as three filterable + sortable
//! tables (Unfollow / Review / Keep). Built for triage: type to
//! filter a section, click a header to sort, click a handle to open
//! the profile in Instagram.
//!
//! ## Why hand-rolled markup
//!
//! No template engine (`maud`, `minijinja`, …). The output is one
//! function over a single data shape; templating gains us nothing
//! over `writeln!` and adds a build-time dep. HTML escaping for the
//! handful of user-controlled fields (display names, hints) lives in
//! [`escape`] below — the standard `& < > "` substitutions.

use std::cmp::Ordering;
use std::io::Write;

use anyhow::{Context, Result};

use super::csv::profile_url;
use super::{contributions_inline, decision_hint};
use crate::scoring::{Bucket, ScoredAccount};

pub fn write_to(scored: &[ScoredAccount], mut writer: impl Write) -> Result<()> {
    let keep = scored.iter().filter(|s| s.bucket == Bucket::Keep).count();
    let review = scored.iter().filter(|s| s.bucket == Bucket::Review).count();
    let unfollow = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Unfollow)
        .count();

    let date = jiff::Zoned::now().date().to_string();
    let total = scored.len();

    writeln!(writer, "<!DOCTYPE html>").context("html doctype")?;
    writeln!(writer, "<html lang=\"en\">").context("html")?;
    writeln!(writer, "<head>").context("html")?;
    writeln!(writer, "<meta charset=\"utf-8\">").context("html")?;
    writeln!(
        writer,
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
    )
    .context("html")?;
    writeln!(writer, "<title>Following audit — {}</title>", escape(&date)).context("html")?;
    writeln!(writer, "<style>{STYLE}</style>").context("style")?;
    writeln!(writer, "</head>").context("html")?;
    writeln!(writer, "<body>").context("html")?;

    write_header(&mut writer, total, &date, keep, review, unfollow)?;

    writeln!(writer, "<main>").context("html")?;
    write_unfollow_section(&mut writer, scored)?;
    write_review_section(&mut writer, scored)?;
    write_keep_section(&mut writer, scored)?;
    writeln!(writer, "</main>").context("html")?;

    writeln!(writer, "<script>{SCRIPT}</script>").context("script")?;
    writeln!(writer, "</body></html>").context("html")?;
    Ok(())
}

fn write_header(
    writer: &mut impl Write,
    total: usize,
    date: &str,
    keep: usize,
    review: usize,
    unfollow: usize,
) -> Result<()> {
    writeln!(writer, "<header>").context("html")?;
    writeln!(writer, "<h1>Following audit</h1>").context("html")?;
    writeln!(
        writer,
        "<p class=\"meta\">{total} accounts scored on {}</p>",
        escape(date)
    )
    .context("html")?;
    writeln!(writer, "<div class=\"stats\">").context("html")?;
    for (cls, label, n) in [
        ("keep", "Keep", keep),
        ("review", "Review", review),
        ("unfollow", "Unfollow", unfollow),
    ] {
        writeln!(
            writer,
            "<div class=\"stat {cls}\"><div class=\"num\">{n}</div><div>{label}</div></div>"
        )
        .context("html")?;
    }
    writeln!(writer, "</div>").context("html")?;
    writeln!(writer, "</header>").context("html")?;
    Ok(())
}

fn write_unfollow_section(writer: &mut impl Write, scored: &[ScoredAccount]) -> Result<()> {
    let mut rows: Vec<&ScoredAccount> = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Unfollow)
        .collect();
    rows.sort_by(|a, b| {
        a.keep_prob
            .partial_cmp(&b.keep_prob)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });
    write_section(writer, "unfollow", "Unfollow", &rows)
}

fn write_review_section(writer: &mut impl Write, scored: &[ScoredAccount]) -> Result<()> {
    let mut rows: Vec<&ScoredAccount> = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Review)
        .collect();
    // Decision difficulty ascending — hardest calls first, mirrors MD.
    rows.sort_by(|a, b| {
        (a.keep_prob - 0.5)
            .abs()
            .partial_cmp(&(b.keep_prob - 0.5).abs())
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });
    write_section(writer, "review", "Review", &rows)
}

fn write_keep_section(writer: &mut impl Write, scored: &[ScoredAccount]) -> Result<()> {
    let mut rows: Vec<&ScoredAccount> =
        scored.iter().filter(|s| s.bucket == Bucket::Keep).collect();
    rows.sort_by(|a, b| {
        // Highest keep_prob first — the inverse of the other buckets.
        // Reasoning: in Unfollow/Review the user reads top-down to make
        // decisions; in Keep the top of the list is the validation
        // surface ("these should obviously be keeps"), and the bottom
        // is the boundary ("could these have been Review?"). Both ends
        // are interesting — the filter UI lets the user explore.
        b.keep_prob
            .partial_cmp(&a.keep_prob)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });
    write_section(writer, "keep", "Keep", &rows)
}

fn write_section(
    writer: &mut impl Write,
    id: &str,
    label: &str,
    rows: &[&ScoredAccount],
) -> Result<()> {
    writeln!(writer, "<section data-bucket=\"{id}\">").context("html")?;
    writeln!(
        writer,
        "<h2>{label} <span class=\"count\">({})</span></h2>",
        rows.len()
    )
    .context("html")?;
    writeln!(writer, "<div class=\"controls\">").context("html")?;
    writeln!(
        writer,
        "<input type=\"search\" placeholder=\"Filter {label} — handle, name, or hint\" data-filter>",
    )
    .context("html")?;
    writeln!(writer, "</div>").context("html")?;

    if rows.is_empty() {
        writeln!(writer, "<p class=\"empty\">No accounts in this bucket.</p>").context("html")?;
        writeln!(writer, "</section>").context("html")?;
        return Ok(());
    }

    writeln!(writer, "<table>").context("html")?;
    writeln!(writer, "<thead><tr>").context("html")?;
    for (label, sort_kind) in [
        ("Handle", "text"),
        ("Display name", "text"),
        ("keep_prob", "num"),
        ("Mutual", "text"),
        ("Class", "text"),
        ("Tenure (d)", "num"),
        ("Why", "text"),
        ("Hint", "text"),
    ] {
        let num_class = if sort_kind == "num" {
            " class=\"num\""
        } else {
            ""
        };
        writeln!(
            writer,
            "<th{num_class} data-sort=\"{sort_kind}\">{label}</th>"
        )
        .context("html")?;
    }
    writeln!(writer, "</tr></thead>").context("html")?;

    writeln!(writer, "<tbody>").context("html")?;
    for s in rows {
        write_row(writer, s)?;
    }
    writeln!(writer, "</tbody>").context("html")?;
    writeln!(writer, "</table>").context("html")?;
    writeln!(writer, "</section>").context("html")?;
    Ok(())
}

fn write_row(writer: &mut impl Write, s: &ScoredAccount) -> Result<()> {
    let f = &s.features;
    let handle = &f.username;
    let url = profile_url(handle);
    let display = f.display_name.as_deref().unwrap_or("");
    let mutual = if f.is_mutual { "yes" } else { "no" };
    let class = f.account_class.as_str();
    let tenure_cell = f
        .follow_tenure_days
        .map(|d| d.to_string())
        .unwrap_or_default();
    let why = contributions_inline(s, "—");
    let hint = decision_hint(f, s.bucket);

    writeln!(writer, "<tr>").context("html")?;
    writeln!(
        writer,
        "<td class=\"handle\"><a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">@{}</a></td>",
        escape(handle)
    )
    .context("html")?;
    writeln!(writer, "<td>{}</td>", escape(display)).context("html")?;
    writeln!(writer, "<td class=\"num\">{:.3}</td>", s.keep_prob).context("html")?;
    writeln!(writer, "<td>{mutual}</td>").context("html")?;
    writeln!(writer, "<td>{class}</td>").context("html")?;
    writeln!(writer, "<td class=\"num\">{tenure_cell}</td>").context("html")?;
    writeln!(writer, "<td class=\"why\">{}</td>", escape(&why)).context("html")?;
    writeln!(writer, "<td class=\"hint\">{}</td>", escape(hint)).context("html")?;
    writeln!(writer, "</tr>").context("html")?;
    Ok(())
}

/// Minimal HTML entity escaping. Handles the four characters that can
/// break the document or attribute context. Single-quote isn't covered
/// because we never wrap attributes in single quotes.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Inline CSS. System-font stack, neutral palette, subtle bucket
/// tinting on the summary tiles, sticky table headers, monospace
/// handles. Kept compact — no Tailwind, no resets beyond what's
/// needed.
const STYLE: &str = "\
:root {
  --bg: #fafaf9;
  --surface: #ffffff;
  --fg: #1c1c1c;
  --muted: #78716c;
  --border: #e7e5e4;
  --keep-bg: #ecfdf5;
  --keep-fg: #065f46;
  --review-bg: #fef3c7;
  --review-fg: #92400e;
  --unfollow-bg: #fee2e2;
  --unfollow-fg: #991b1b;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #18181b;
    --surface: #27272a;
    --fg: #fafaf9;
    --muted: #a1a1aa;
    --border: #3f3f46;
    --keep-bg: #052e2b;
    --keep-fg: #6ee7b7;
    --review-bg: #422006;
    --review-fg: #fcd34d;
    --unfollow-bg: #450a0a;
    --unfollow-fg: #fca5a5;
  }
}
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; }
body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  background: var(--bg);
  color: var(--fg);
  line-height: 1.5;
}
header { padding: 2rem 2rem 1.5rem; border-bottom: 1px solid var(--border); }
h1 { margin: 0; font-size: 1.5rem; font-weight: 600; letter-spacing: -0.01em; }
.meta { margin: 0.25rem 0 1.25rem; color: var(--muted); font-size: 0.875rem; }
.stats { display: flex; gap: 0.75rem; flex-wrap: wrap; }
.stat {
  padding: 0.75rem 1.25rem;
  border-radius: 0.5rem;
  min-width: 7rem;
}
.stat.keep { background: var(--keep-bg); color: var(--keep-fg); }
.stat.review { background: var(--review-bg); color: var(--review-fg); }
.stat.unfollow { background: var(--unfollow-bg); color: var(--unfollow-fg); }
.stat .num { font-size: 1.5rem; font-weight: 600; font-variant-numeric: tabular-nums; }
section { padding: 2rem; }
section + section { border-top: 1px solid var(--border); }
h2 { margin: 0 0 0.75rem; font-size: 1.125rem; font-weight: 600; }
h2 .count { color: var(--muted); font-weight: 400; }
.controls { margin-bottom: 1rem; }
input[type='search'] {
  padding: 0.5rem 0.75rem;
  border: 1px solid var(--border);
  border-radius: 0.375rem;
  background: var(--surface);
  color: var(--fg);
  font-size: 0.875rem;
  width: 100%;
  max-width: 28rem;
}
input[type='search']:focus { outline: 2px solid var(--review-fg); outline-offset: -1px; }
.empty { color: var(--muted); font-style: italic; }
table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.875rem;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 0.375rem;
  overflow: hidden;
}
th, td {
  text-align: left;
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid var(--border);
  vertical-align: top;
}
tr:last-child td { border-bottom: none; }
th {
  background: var(--bg);
  font-weight: 600;
  font-size: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--muted);
  cursor: pointer;
  user-select: none;
  position: sticky;
  top: 0;
}
th:hover { color: var(--fg); }
th.sorted-asc::after { content: ' \\2191'; }
th.sorted-desc::after { content: ' \\2193'; }
td.num { font-variant-numeric: tabular-nums; text-align: right; white-space: nowrap; }
.handle a {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  color: var(--fg);
  text-decoration: none;
}
.handle a:hover { text-decoration: underline; color: var(--review-fg); }
.why { font-family: ui-monospace, 'SF Mono', Menlo, monospace; font-size: 0.75rem; color: var(--muted); }
.hint { color: var(--muted); font-style: italic; max-width: 24rem; }
";

/// Vanilla JS for sort + filter. ~60 lines, no deps, no closures over
/// the DOM at construction time (each handler resolves its own
/// section / table / index at click time so the script is robust to
/// future template changes).
const SCRIPT: &str = "\
document.querySelectorAll('input[data-filter]').forEach(function (input) {
  input.addEventListener('input', function (e) {
    var q = e.target.value.toLowerCase();
    var section = e.target.closest('section');
    var rows = section.querySelectorAll('tbody tr');
    rows.forEach(function (row) {
      row.style.display = row.textContent.toLowerCase().indexOf(q) !== -1 ? '' : 'none';
    });
  });
});

document.querySelectorAll('th[data-sort]').forEach(function (th) {
  th.addEventListener('click', function () {
    var table = th.closest('table');
    var tbody = table.querySelector('tbody');
    var idx = Array.from(th.parentNode.children).indexOf(th);
    var isNum = th.dataset.sort === 'num';
    var asc = !th.classList.contains('sorted-asc');
    table.querySelectorAll('th').forEach(function (other) {
      other.classList.remove('sorted-asc', 'sorted-desc');
    });
    th.classList.add(asc ? 'sorted-asc' : 'sorted-desc');
    var rows = Array.from(tbody.querySelectorAll('tr'));
    rows.sort(function (a, b) {
      var av = a.children[idx].textContent.trim();
      var bv = b.children[idx].textContent.trim();
      if (isNum) {
        var an = parseFloat(av);
        var bn = parseFloat(bv);
        // NaN at the bottom regardless of direction so empty cells
        // don't pollute the top of an ascending sort.
        if (isNaN(an)) return 1;
        if (isNaN(bn)) return -1;
        return asc ? an - bn : bn - an;
      }
      return asc ? av.localeCompare(bv) : bv.localeCompare(av);
    });
    rows.forEach(function (r) { tbody.appendChild(r); });
  });
});
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{AccountClass, AccountFeatures};
    use crate::scoring::Bucket;

    fn baseline(handle: &str, bucket: Bucket, keep_prob: f64) -> ScoredAccount {
        ScoredAccount {
            features: AccountFeatures {
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
            },
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
        String::from_utf8(buf).expect("utf-8 html")
    }

    #[test]
    fn renders_three_bucket_sections_with_counts() {
        let scored = vec![
            baseline("u1", Bucket::Unfollow, 0.2),
            baseline("r1", Bucket::Review, 0.5),
            baseline("k1", Bucket::Keep, 0.9),
            baseline("k2", Bucket::Keep, 0.95),
        ];
        let html = render(&scored);
        assert!(html.contains("Unfollow <span class=\"count\">(1)</span>"));
        assert!(html.contains("Review <span class=\"count\">(1)</span>"));
        assert!(html.contains("Keep <span class=\"count\">(2)</span>"));
        assert!(html.contains("4 accounts scored"));
    }

    #[test]
    fn handle_links_to_canonical_profile_url() {
        let scored = vec![baseline("alice_synth", Bucket::Unfollow, 0.2)];
        let html = render(&scored);
        assert!(
            html.contains("href=\"https://www.instagram.com/alice_synth/\""),
            "expected canonical profile link: {html}",
        );
        assert!(
            html.contains("rel=\"noopener noreferrer\""),
            "external links must carry rel=noopener AND noreferrer \
             (noreferrer also strips the file:// path from the Referer \
             header, which Safari historically leaked)",
        );
    }

    #[test]
    fn escapes_html_special_characters_in_display_name() {
        let mut s = baseline("alice", Bucket::Keep, 0.9);
        s.features.display_name = Some("<script>alert(1)</script> & co.".to_owned());
        let html = render(&[s]);
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "raw script tag must be escaped: {html}",
        );
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&amp; co."));
    }

    #[test]
    fn empty_input_renders_three_empty_sections() {
        let html = render(&[]);
        assert!(html.contains("0 accounts scored"));
        assert!(html.contains("Unfollow <span class=\"count\">(0)</span>"));
        assert!(html.contains("No accounts in this bucket"));
    }
}
