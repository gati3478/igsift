//! Self-contained HTML audit report.
//!
//! Single file, no external assets, no server: inline CSS, vanilla JS,
//! double-click to open in a browser. The data is exactly what the
//! Markdown report has, presented as three filterable + sortable tables
//! (Unfollow / Review / Keep) plus a per-row triage affordance.
//!
//! ## Triage from the report
//!
//! Each row carries a keep/drop segmented control. Selections persist
//! client-side (`localStorage`) and feed a floating export bar that
//! Copies or Downloads an appendable plain-text block per list — the
//! user pastes it into `config/keeplist.txt` / `config/droplist.txt`. A
//! `file://` page can't write to disk, so collect-and-paste is the honest
//! model. Keep/drop are mutually exclusive per row, mirroring
//! `lists::ensure_disjoint`.
//!
//! ## Theme
//!
//! A header [`write_theme_switcher`] segmented control (Auto / Light / Dark,
//! an ARIA radiogroup) lets the reader override the OS preference; the choice
//! persists in `localStorage` (`igsift.theme.v1`). Theming is driven by a
//! `data-theme` attribute on `<html>`. The dark token set is emitted once by
//! [`dark_rules`] but under two selectors — `:root[data-theme="dark"]`
//! (explicit) and a `prefers-color-scheme: dark` media query scoped to
//! `:root[data-theme="auto"]` (system-tracking) — so manual and system dark
//! never drift. An anti-FOUC boot script stamps the persisted theme onto
//! `<html>` before the stylesheet paints; with JS disabled the markup default
//! (`auto`) plus the media query still pick the right theme.
//!
//! ## Why hand-rolled markup
//!
//! No template engine (`maud`, `minijinja`, …). The output is one
//! function over a single data shape; templating gains us nothing over
//! `writeln!` and adds a build-time dep. Rows are **server-rendered**:
//! all user-controlled fields pass through [`escape`] here, so the HTML
//! escaping is the security boundary regardless of what the JS does. The
//! JS reads handles back from `data-` attributes (browser-unescaped) only
//! to build the export text and clipboard payload.

use std::cmp::Ordering;
use std::io::Write;

use anyhow::{Context, Result};

use super::csv::profile_url;
use super::{contributions_inline, decision_hint};
use crate::scoring::{Bucket, ScoredAccount};

/// `keep_prob ∈ [0, 1]` → integer percentage for display. Matches the
/// Markdown writer's `pct`; the raw float stays reachable in the cell's
/// `title` (and drives sort via `data-p`) so rounding never reorders rows.
fn pct(p: f64) -> u32 {
    (p * 100.0).round() as u32
}

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
    writeln!(writer, "<html lang=\"en\" data-theme=\"auto\">").context("html")?;
    writeln!(writer, "<head>").context("html")?;
    writeln!(writer, "<meta charset=\"utf-8\">").context("html")?;
    writeln!(
        writer,
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
    )
    .context("html")?;
    // Anti-FOUC: stamp the persisted theme onto <html> before the stylesheet
    // paints, so a saved Dark choice never flashes light on load. Runs ahead
    // of <style>; with JS disabled the markup's data-theme="auto" + the
    // prefers-color-scheme media query still drive the right theme.
    writeln!(writer, "<script>{BOOT_SCRIPT}</script>").context("script")?;
    writeln!(writer, "<title>Following audit — {}</title>", escape(&date)).context("html")?;
    // Dark tokens are emitted twice from one source (`dark_rules`): once under
    // the explicit `[data-theme="dark"]` override, once inside the media query
    // scoped to `[data-theme="auto"]` so auto still defers to the OS. Single
    // source, two emit sites — no drift between manual and system dark.
    writeln!(
        writer,
        "<style>{STYLE}\n{dark}@media (prefers-color-scheme: dark){{\n{auto}}}\n</style>",
        dark = dark_rules(":root[data-theme=\"dark\"]"),
        auto = dark_rules(":root[data-theme=\"auto\"]"),
    )
    .context("style")?;
    writeln!(writer, "</head>").context("html")?;
    writeln!(writer, "<body>").context("html")?;

    writeln!(writer, "<div class=\"wrap\">").context("html")?;
    write_header(&mut writer, total, &date, keep, review, unfollow)?;

    writeln!(writer, "<main id=\"main\">").context("html")?;
    write_unfollow_section(&mut writer, scored)?;
    write_review_section(&mut writer, scored)?;
    write_keep_section(&mut writer, scored)?;
    writeln!(writer, "</main>").context("html")?;
    writeln!(writer, "</div>").context("html")?;

    write_export_bar(&mut writer)?;

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
    writeln!(writer, "<div class=\"head-row\">").context("html")?;
    writeln!(writer, "<div class=\"head-text\">").context("html")?;
    writeln!(
        writer,
        "<p class=\"eyebrow\">igsift · local-first following audit</p>"
    )
    .context("html")?;
    writeln!(writer, "<h1>Following audit</h1>").context("html")?;
    writeln!(writer, "</div>").context("html")?;
    write_theme_switcher(writer)?;
    writeln!(writer, "</div>").context("html")?;
    writeln!(
        writer,
        "<p class=\"meta\">{total} accounts scored on {} · no network, no automated unfollow — you act manually in Instagram.</p>",
        escape(date)
    )
    .context("html")?;
    writeln!(writer, "<div class=\"tiles\">").context("html")?;
    for (cls, label, n, sub) in [
        ("keep", "Keep", keep, "relationships worth holding"),
        ("review", "Review", review, "judgment calls — hardest first"),
        (
            "unfollow",
            "Unfollow",
            unfollow,
            "low signal — safe to drop",
        ),
    ] {
        writeln!(writer, "<div class=\"tile {cls}\">").context("html")?;
        writeln!(
            writer,
            "<div class=\"label\"><span class=\"dot\" aria-hidden=\"true\"></span>{label}</div>"
        )
        .context("html")?;
        writeln!(writer, "<div class=\"num\">{n}</div>").context("html")?;
        writeln!(writer, "<div class=\"sub\">{sub}</div>").context("html")?;
        writeln!(writer, "</div>").context("html")?;
    }
    writeln!(writer, "</div>").context("html")?;
    writeln!(writer, "</header>").context("html")?;
    Ok(())
}

/// Theme switcher: a three-state segmented control (Auto / Light / Dark)
/// as an ARIA radiogroup. Auto is first-class — the report tracks
/// `prefers-color-scheme` by default, so dropping it would be a regression
/// for system-sync users. Roving `tabindex` (checked radio is the single
/// tab stop) + arrow-key navigation are wired in the bottom script; the
/// static markup ships Auto checked so a JS-disabled load is still coherent.
fn write_theme_switcher(writer: &mut impl Write) -> Result<()> {
    writeln!(
        writer,
        "<div class=\"theme\" role=\"radiogroup\" aria-label=\"Color theme\">"
    )
    .context("html")?;
    for (val, label, icon, title, checked) in [
        ("auto", "Auto", AUTO_ICON, "Match system", true),
        ("light", "Light", SUN_ICON, "Light", false),
        ("dark", "Dark", MOON_ICON, "Dark", false),
    ] {
        writeln!(
            writer,
            "<button class=\"theme-opt\" role=\"radio\" data-theme-set=\"{val}\" aria-checked=\"{checked}\" tabindex=\"{tab}\" title=\"{title}\">{icon}<span>{label}</span></button>",
            tab = if checked { 0 } else { -1 },
        )
        .context("html")?;
    }
    writeln!(writer, "</div>").context("html")?;
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
    write_section(
        writer,
        "unfollow",
        "Unfollow",
        "Lowest keep likelihood first — the safest drops sit at the top.",
        &rows,
    )
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
    write_section(
        writer,
        "review",
        "Review",
        "Hardest calls first — scores nearest the 50% line need your judgment.",
        &rows,
    )
}

fn write_keep_section(writer: &mut impl Write, scored: &[ScoredAccount]) -> Result<()> {
    let mut rows: Vec<&ScoredAccount> =
        scored.iter().filter(|s| s.bucket == Bucket::Keep).collect();
    rows.sort_by(|a, b| {
        // Highest keep_prob first — the inverse of the other buckets.
        // Reasoning: in Unfollow/Review the user reads top-down to make
        // decisions; in Keep the top is the validation surface ("these
        // should obviously be keeps") and the bottom is the boundary
        // ("could these have been Review?"). The filter/sort UI lets the
        // user explore either end.
        b.keep_prob
            .partial_cmp(&a.keep_prob)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });
    write_section(
        writer,
        "keep",
        "Keep",
        "Highest-confidence first. The bottom rows sit near the Review boundary — worth a glance.",
        &rows,
    )
}

fn write_section(
    writer: &mut impl Write,
    id: &str,
    label: &str,
    sub: &str,
    rows: &[&ScoredAccount],
) -> Result<()> {
    writeln!(writer, "<section data-bucket=\"{id}\">").context("html")?;
    writeln!(
        writer,
        "<div class=\"sec-head\"><h2>{label} <span class=\"pill\">{}</span></h2></div>",
        rows.len()
    )
    .context("html")?;
    writeln!(writer, "<p class=\"sec-sub\">{sub}</p>").context("html")?;

    writeln!(writer, "<div class=\"controls\">").context("html")?;
    writeln!(writer, "<label class=\"search\">{SEARCH_ICON}").context("html")?;
    writeln!(
        writer,
        "<input type=\"search\" placeholder=\"Filter {} — handle, name, or hint\" aria-label=\"Filter {label}\"></label>",
        label.to_lowercase()
    )
    .context("html")?;
    writeln!(
        writer,
        "<span class=\"shown\" data-shown>{} shown</span>",
        rows.len()
    )
    .context("html")?;
    writeln!(writer, "</div>").context("html")?;

    if rows.is_empty() {
        writeln!(
            writer,
            "<div class=\"tbl-wrap\"><p class=\"empty-state\">Nothing in this bucket.</p></div>"
        )
        .context("html")?;
        writeln!(writer, "</section>").context("html")?;
        return Ok(());
    }

    writeln!(writer, "<div class=\"tbl-wrap\">").context("html")?;
    writeln!(writer, "<table>").context("html")?;
    writeln!(writer, "<thead><tr>").context("html")?;
    // (label, sort-kind, extra-class). `num` columns right-align and sort
    // numerically (via data- attributes); `act` is the non-sortable
    // triage column.
    for (col_label, kind, extra) in [
        ("Handle", "text", ""),
        ("Display name", "text", ""),
        ("Keep likelihood", "num", "col-num"),
        ("Mutual", "num", ""),
        ("Class", "text", ""),
        ("Tenure (d)", "num", "col-num"),
        ("Why", "text", "col-why"),
        ("Hint", "text", ""),
        ("Triage", "act", "col-act"),
    ] {
        if kind == "act" {
            writeln!(writer, "<th class=\"{extra}\">{col_label}</th>").context("html")?;
        } else {
            let class = if extra.is_empty() {
                "sortable".to_string()
            } else {
                format!("{extra} sortable")
            };
            writeln!(
                writer,
                "<th class=\"{class}\" tabindex=\"0\" role=\"button\" aria-sort=\"none\" data-kind=\"{kind}\">{col_label}<span class=\"arrow\" aria-hidden=\"true\"></span></th>"
            )
            .context("html")?;
        }
    }
    writeln!(writer, "</tr></thead>").context("html")?;

    writeln!(writer, "<tbody>").context("html")?;
    for s in rows {
        write_row(writer, s)?;
    }
    writeln!(writer, "</tbody>").context("html")?;
    writeln!(writer, "</table>").context("html")?;
    writeln!(writer, "</div>").context("html")?;
    writeln!(writer, "</section>").context("html")?;
    Ok(())
}

fn write_row(writer: &mut impl Write, s: &ScoredAccount) -> Result<()> {
    let f = &s.features;
    let handle = &f.username;
    let url = profile_url(handle);
    let display = f.display_name.as_deref().unwrap_or("");
    let mutual = f.is_mutual;
    let class = f.account_class.as_str();
    let tenure = f.follow_tenure_days;
    let why = contributions_inline(s, "—");
    let hint = decision_hint(f, s.bucket);
    let p = pct(s.keep_prob);

    // data- attributes drive typed sort + the export payload. The handle
    // is HTML-escaped into the attribute; the browser un-escapes it on
    // `dataset` read, so the JS recovers the exact handle for the file.
    writeln!(
        writer,
        "<tr data-b=\"{bucket}\" data-h=\"{h}\" data-p=\"{raw}\" data-t=\"{t}\" data-m=\"{m}\">",
        bucket = s.bucket.as_str(),
        h = escape(handle),
        raw = s.keep_prob,
        t = tenure.map(|d| d.to_string()).unwrap_or_default(),
        m = u8::from(mutual),
    )
    .context("html")?;
    writeln!(
        writer,
        "<td class=\"handle\"><a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">@{}</a></td>",
        escape(&url),
        escape(handle)
    )
    .context("html")?;
    if display.is_empty() {
        writeln!(writer, "<td class=\"name empty\">—</td>").context("html")?;
    } else {
        writeln!(writer, "<td class=\"name\">{}</td>", escape(display)).context("html")?;
    }
    writeln!(
        writer,
        "<td class=\"score\" title=\"raw keep_prob = {raw:.3}\"><span class=\"score-cell\"><span class=\"score-pct\">{p}%</span><span class=\"score-bar\" aria-hidden=\"true\"><i style=\"width:{p}%\"></i></span></span></td>",
        raw = s.keep_prob,
    )
    .context("html")?;
    if mutual {
        writeln!(
            writer,
            "<td><span class=\"tag mutual-yes\">mutual</span></td>"
        )
        .context("html")?;
    } else {
        writeln!(writer, "<td><span class=\"tag\">one-way</span></td>").context("html")?;
    }
    writeln!(
        writer,
        "<td><span class=\"tag\">{}</span></td>",
        escape(class)
    )
    .context("html")?;
    writeln!(
        writer,
        "<td class=\"num\">{}</td>",
        tenure.map(|d| d.to_string()).unwrap_or_default()
    )
    .context("html")?;
    writeln!(writer, "<td class=\"why\">{}</td>", escape(&why)).context("html")?;
    writeln!(writer, "<td class=\"hint\">{}</td>", escape(hint)).context("html")?;
    writeln!(
        writer,
        "<td class=\"actions\"><span class=\"seg\" role=\"group\" aria-label=\"Triage @{h}\"><button class=\"keep\" data-toggle=\"keep\" aria-pressed=\"false\">{keep_icon}Keep</button><button class=\"drop\" data-toggle=\"drop\" aria-pressed=\"false\">{drop_icon}Drop</button></span></td>",
        h = escape(handle),
        keep_icon = KEEP_ICON,
        drop_icon = DROP_ICON,
    )
    .context("html")?;
    writeln!(writer, "</tr>").context("html")?;
    Ok(())
}

fn write_export_bar(writer: &mut impl Write) -> Result<()> {
    writeln!(
        writer,
        "<div class=\"exportbar\" id=\"exportbar\" role=\"region\" aria-label=\"Triage selections\">"
    )
    .context("html")?;

    // Counts — big tabular number + muted label, never wrapping. A list with
    // zero selections dims (data-empty) so the eye lands on the one you're
    // building; renderBar() toggles that attribute.
    writeln!(writer, "<div class=\"eb-counts\">").context("html")?;
    writeln!(
        writer,
        "<span class=\"eb-count keep\" data-empty=\"true\"><span class=\"eb-dot\" aria-hidden=\"true\"></span><b id=\"kc\">0</b><span class=\"eb-count-label\">keeplist</span></span>"
    )
    .context("html")?;
    writeln!(
        writer,
        "<span class=\"eb-count drop\" data-empty=\"true\"><span class=\"eb-dot\" aria-hidden=\"true\"></span><b id=\"dc\">0</b><span class=\"eb-count-label\">droplist</span></span>"
    )
    .context("html")?;
    writeln!(writer, "</div>").context("html")?;
    writeln!(writer, "<div class=\"eb-sep\" aria-hidden=\"true\"></div>").context("html")?;

    // One segmented control per list, filled in that list's semantic color
    // (keep = green, drop = red). Copy is the primary action (click → paste);
    // the download glyph is the secondary "save as .txt" affordance.
    writeln!(writer, "<div class=\"eb-actions\">").context("html")?;
    for (list, label) in [("keep", "keeplist"), ("drop", "droplist")] {
        writeln!(
            writer,
            "<div class=\"eb-seg {list}\" role=\"group\" aria-label=\"Export {label}\">"
        )
        .context("html")?;
        writeln!(
            writer,
            "<button class=\"eb-btn eb-copy\" data-act=\"copy\" data-list=\"{list}\">{COPY_ICON}Copy {label}</button>"
        )
        .context("html")?;
        writeln!(
            writer,
            "<button class=\"eb-btn eb-dl\" data-act=\"download\" data-list=\"{list}\" aria-label=\"Download {label} as .txt\" title=\"Download {label} as .txt\">{DOWNLOAD_ICON}</button>"
        )
        .context("html")?;
        writeln!(writer, "</div>").context("html")?;
    }
    writeln!(writer, "</div>").context("html")?;

    writeln!(writer, "<div class=\"eb-sep\" aria-hidden=\"true\"></div>").context("html")?;
    writeln!(
        writer,
        "<button class=\"eb-clear\" id=\"clearAll\">Clear</button>"
    )
    .context("html")?;
    writeln!(writer, "</div>").context("html")?;
    writeln!(
        writer,
        "<div class=\"toast\" id=\"toast\" role=\"status\" aria-live=\"polite\"></div>"
    )
    .context("html")?;
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

/// Dark-theme CSS, parameterized by the root selector so the same bytes are
/// emitted under both `:root[data-theme="dark"]` (explicit override) and the
/// `prefers-color-scheme: dark` media query scoped to `:root[data-theme="auto"]`
/// (system-tracking). Single source → no drift between manual and system dark.
/// Covers the token block plus the two `.seg` pressed-button color tweaks that
/// previously lived in their own dark media query — without re-scoping those,
/// a light-system user who picks Dark would get white triage-button text.
fn dark_rules(root: &str) -> String {
    format!(
        "{root} {{ \
--bg:#161618; --surface:#1f1f22; --surface-2:#26262a; \
--fg:#f2f2f4; --fg-2:#c4c4c9; --muted:#9a9aa1; \
--border:#3a3a40; --border-soft:#2c2c31; \
--accent:#4ea1ff; --accent-weak:#16304d; \
--keep-fg:#6fd99b; --keep-bg:#11301f; --keep-line:#2e9e5b; \
--review-fg:#f0c265; --review-bg:#332406; --review-line:#d9920b; \
--unfollow-fg:#f0908f; --unfollow-bg:#361414; --unfollow-line:#d94a4a; \
--shadow:none; --seg-thumb:#34343a; }}\n\
{root} .seg button[aria-pressed=true].keep {{ color:#08160d; }}\n\
{root} .seg button[aria-pressed=true].drop {{ color:#1a0707; }}\n"
    )
}

/// Inline SVGs — outlined, 1-style icon set, sized to text via CSS.
const KEEP_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M3 8.5l3.2 3.2L13 5\"/></svg>";
const DROP_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M4 4l8 8M12 4l-8 8\"/></svg>";
const SEARCH_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" aria-hidden=\"true\"><circle cx=\"7\" cy=\"7\" r=\"4.5\"/><path d=\"M11 11l3 3\"/></svg>";
const COPY_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><rect x=\"5\" y=\"5\" width=\"8\" height=\"8\" rx=\"1.5\"/><path d=\"M3 11V4a1.5 1.5 0 011.5-1.5H11\"/></svg>";
const DOWNLOAD_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M8 2v8M4.5 7.5L8 11l3.5-3.5M3 13.5h10\"/></svg>";
/// Theme-switcher glyphs — same thin-stroke (1.6) chrome family as
/// search/copy. Sun = Light, crescent = Dark, half-filled circle = Auto
/// (the one intentional `fill`, so the system-sync mark reads at 15px).
const SUN_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><circle cx=\"8\" cy=\"8\" r=\"3\"/><path d=\"M8 1.5v1.5M8 13v1.5M1.5 8h1.5M13 8h1.5M3.4 3.4l1.05 1.05M11.55 11.55l1.05 1.05M12.6 3.4l-1.05 1.05M4.45 11.55L3.4 12.6\"/></svg>";
const MOON_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M13 9.2A5.2 5.2 0 016.8 3a5.2 5.2 0 100 10 5.2 5.2 0 006.2-3.8z\"/></svg>";
const AUTO_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><circle cx=\"8\" cy=\"8\" r=\"5.5\"/><path d=\"M8 2.5v11\"/><path d=\"M8 2.5a5.5 5.5 0 000 11z\" fill=\"currentColor\" stroke=\"none\"/></svg>";

/// Anti-FOUC boot script (emitted in `<head>`, ahead of `<style>`): copies a
/// valid persisted theme onto `<html>` before first paint. Guarded so a junk
/// localStorage value or a privacy-mode `getItem` throw degrades to the
/// markup default (`auto`). Kept tiny and dependency-free.
const BOOT_SCRIPT: &str = "(function(){try{var t=localStorage.getItem('igsift.theme.v1');if(t==='light'||t==='dark'||t==='auto'){document.documentElement.setAttribute('data-theme',t);}}catch(e){}})();";

/// Inline CSS. 8pt spacing grid, system font, semantic bucket color
/// applied as a thin top rule + the number (not full tile fills), real
/// dark mode (elevated surfaces lighter, borders replace shadows), visible
/// keyboard focus, `prefers-reduced-motion`. Kept compact — no framework.
const STYLE: &str = "\
:root {
  --bg: #f5f5f7; --surface: #ffffff; --surface-2: #fbfbfd;
  --fg: #1d1d1f; --fg-2: #515154; --muted: #6e6e73;
  --border: #d2d2d7; --border-soft: #e8e8ed;
  --accent: #0066cc; --accent-weak: #e6f0fb;
  --keep-fg: #1c6b3d; --keep-bg: #e9f6ee; --keep-line: #2e9e5b;
  --review-fg: #8a5a00; --review-bg: #fdf2dc; --review-line: #d9920b;
  --unfollow-fg: #a3282a; --unfollow-bg: #fce9e9; --unfollow-line: #d94a4a;
  --shadow: 0 1px 2px rgba(0,0,0,.04), 0 4px 16px rgba(0,0,0,.04);
  --radius: 12px; --radius-sm: 8px;
  --seg-thumb: var(--surface);
  --s1:4px; --s2:8px; --s3:12px; --s4:16px; --s5:24px; --s6:32px; --s8:48px;
  --font: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif;
  --mono: ui-monospace, 'SF Mono', SFMono-Regular, Menlo, Consolas, monospace;
}
/* Dark token sets are appended after this const by write_to() — emitted from
   dark_rules() under both :root[data-theme=dark] and the auto/media pairing. */
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; }
body { font-family: var(--font); background: var(--bg); color: var(--fg);
  line-height: 1.5; -webkit-font-smoothing: antialiased; text-rendering: optimizeLegibility; }
.wrap { max-width: 1180px; margin: 0 auto; padding: 0 var(--s5); }
:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: 4px; }
:focus:not(:focus-visible) { outline: none; }
header { padding: var(--s8) 0 var(--s5); }
.eyebrow { font-size: .75rem; font-weight: 600; letter-spacing: .08em;
  text-transform: uppercase; color: var(--muted); margin: 0 0 var(--s2); }
h1 { margin: 0; font-size: 2rem; font-weight: 600; letter-spacing: -.02em; }
.meta { margin: var(--s2) 0 0; color: var(--muted); font-size: .9375rem; }
.head-row { display:flex; align-items:flex-start; justify-content:space-between; gap: var(--s4); }
.head-text { min-width: 0; }
/* Theme switcher: lifted-thumb segmented control. Neutral (not accent-filled
   like the triage .seg) because theme is chrome, not a consequential action —
   accent stays reserved for links/actions per the 60-30-10 discipline. */
/* Track is recessed (--border-soft), thumb is an elevated surface — in light
   mode a white thumb on a near-white --surface-2 track is invisible (~1.01:1),
   so the track must sit a step below the thumb in both themes. */
.theme { display:inline-flex; align-items:center; gap:2px; padding:3px; flex:none;
  background: var(--border-soft); border:1px solid var(--border); border-radius: var(--radius-sm); }
.theme-opt { display:inline-flex; align-items:center; gap: var(--s1); height:32px;
  padding:0 var(--s3); border:0; background:transparent; cursor:pointer; font: inherit;
  font-size:.8125rem; font-weight:500; color: var(--muted); border-radius:6px;
  transition: color .12s ease-out, background .12s ease-out, transform .12s ease-out; }
.theme-opt svg { width:15px; height:15px; flex:none; }
.theme-opt:hover { color: var(--fg-2); }
.theme-opt[aria-checked=true] { background: var(--seg-thumb); color: var(--fg);
  box-shadow: 0 1px 2px rgba(0,0,0,.16); }
.theme-opt:active { transform: scale(.97); }
.theme-opt:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.theme-opt:focus:not(:focus-visible) { outline: none; }
.tiles { display: grid; grid-template-columns: repeat(3, 1fr); gap: var(--s4); margin-top: var(--s5); }
.tile { background: var(--surface); border: 1px solid var(--border-soft);
  border-radius: var(--radius); box-shadow: var(--shadow);
  padding: var(--s4) var(--s5); position: relative; overflow: hidden; }
.tile::before { content:''; position:absolute; inset:0 0 auto 0; height:3px; }
.tile.keep::before { background: var(--keep-line); }
.tile.review::before { background: var(--review-line); }
.tile.unfollow::before { background: var(--unfollow-line); }
.tile .label { display:flex; align-items:center; gap: var(--s2);
  font-size: .8125rem; font-weight: 500; color: var(--fg-2); }
.tile .dot { width:8px; height:8px; border-radius:50%; flex:none; }
.tile.keep .dot { background: var(--keep-line); }
.tile.review .dot { background: var(--review-line); }
.tile.unfollow .dot { background: var(--unfollow-line); }
.tile .num { font-size: 2.25rem; font-weight: 600; line-height: 1.1;
  margin-top: var(--s2); font-variant-numeric: tabular-nums; letter-spacing: -.02em; }
.tile .sub { font-size: .8125rem; color: var(--muted); margin-top: var(--s1); }
main { padding-bottom: 120px; }
section { margin-top: var(--s8); }
.sec-head { display:flex; align-items:baseline; gap: var(--s3); flex-wrap: wrap; margin-bottom: var(--s2); }
h2 { margin:0; font-size: 1.375rem; font-weight: 600; letter-spacing: -.01em;
  display:flex; align-items:center; gap: var(--s3); }
h2 .pill { font-size: .8125rem; font-weight: 600; padding: 2px 10px;
  border-radius: 999px; font-variant-numeric: tabular-nums; }
section[data-bucket=keep] h2 .pill { background: var(--keep-bg); color: var(--keep-fg); }
section[data-bucket=review] h2 .pill { background: var(--review-bg); color: var(--review-fg); }
section[data-bucket=unfollow] h2 .pill { background: var(--unfollow-bg); color: var(--unfollow-fg); }
.sec-sub { color: var(--muted); font-size: .9375rem; margin: 0; }
.controls { display:flex; align-items:center; gap: var(--s3); margin: var(--s4) 0 var(--s3); flex-wrap: wrap; }
.search { position: relative; flex: 1 1 320px; max-width: 30rem; }
.search svg { position:absolute; left: 12px; top: 50%; transform: translateY(-50%);
  width:16px; height:16px; color: var(--muted); pointer-events:none; }
input[type=search] { width:100%; padding: 10px 12px 10px 36px; font-size: .9375rem;
  font-family: var(--font); color: var(--fg); background: var(--surface);
  border: 1px solid var(--border); border-radius: var(--radius-sm); }
input[type=search]::placeholder { color: var(--muted); }
.shown { font-size: .8125rem; color: var(--muted); font-variant-numeric: tabular-nums; white-space:nowrap; }
.tbl-wrap { background: var(--surface); border: 1px solid var(--border-soft);
  border-radius: var(--radius); box-shadow: var(--shadow); overflow-x: auto; }
table { width:100%; border-collapse: collapse; font-size: .875rem; }
thead th { position: sticky; top: 0; z-index: 2; background: var(--surface-2);
  text-align: left; font-weight: 600; font-size: .6875rem; letter-spacing: .06em;
  text-transform: uppercase; color: var(--muted); padding: var(--s3) var(--s3);
  white-space: nowrap; border-bottom: 1px solid var(--border); }
thead th.sortable { cursor: pointer; user-select: none; }
thead th.sortable:hover { color: var(--fg); }
thead th .arrow { opacity: 0; margin-left: 4px; font-size: .75rem; }
thead th[aria-sort=ascending] .arrow, thead th[aria-sort=descending] .arrow { opacity: 1; color: var(--accent); }
thead th[aria-sort=ascending] .arrow::after { content:'\\2191'; }
thead th[aria-sort=descending] .arrow::after { content:'\\2193'; }
thead th.col-num { text-align: right; }
thead th.col-act { text-align: right; right: 0; z-index: 3; box-shadow: -8px 0 8px -8px rgba(0,0,0,.18); }
tbody td { padding: var(--s3) var(--s3); border-bottom: 1px solid var(--border-soft);
  vertical-align: middle; color: var(--fg-2); }
tbody tr:last-child td { border-bottom: none; }
tbody tr:hover td { background: var(--surface-2); }
/* selected rows: color + a left rule so the keep/drop distinction
   survives without color (the toggle icon + aria-pressed carry it too). */
tbody tr.sel-keep td { background: var(--keep-bg); }
tbody tr.sel-drop td { background: var(--unfollow-bg); }
tbody tr.sel-keep td:first-child { box-shadow: inset 3px 0 0 var(--keep-line); }
tbody tr.sel-drop td:first-child { box-shadow: inset 3px 0 0 var(--unfollow-line); }
.handle a { font-family: var(--mono); font-size: .8125rem; font-weight: 500; color: var(--fg); text-decoration: none; }
.handle a:hover { color: var(--accent); text-decoration: underline; }
.name.empty { color: var(--muted); }
td.score { text-align: right; white-space: nowrap; }
.score-cell { display:inline-flex; align-items:center; gap: var(--s2); justify-content:flex-end; }
.score-pct { font-variant-numeric: tabular-nums; font-weight: 600; min-width: 3ch; text-align: right; color: var(--fg); }
.score-bar { width: 44px; height: 6px; border-radius: 3px; background: var(--border-soft); overflow: hidden; flex: none; }
.score-bar > i { display:block; height:100%; border-radius: 3px; }
tr[data-b=keep] .score-bar > i { background: var(--keep-line); }
tr[data-b=review] .score-bar > i { background: var(--review-line); }
tr[data-b=unfollow] .score-bar > i { background: var(--unfollow-line); }
td.num { text-align: right; font-variant-numeric: tabular-nums; white-space:nowrap; color: var(--fg-2); }
.tag { display:inline-flex; align-items:center; gap:4px; font-size:.75rem; padding: 1px 8px;
  border-radius: 999px; border:1px solid var(--border); color: var(--fg-2);
  background: var(--surface-2); white-space:nowrap; }
.mutual-yes { color: var(--keep-fg); border-color: var(--keep-line); background: var(--keep-bg); }
.why { font-family: var(--mono); font-size: .75rem; color: var(--muted); white-space: nowrap; }
.hint { color: var(--muted); max-width: 22rem; }
td.actions { text-align: right; white-space: nowrap;
  position: sticky; right: 0; background: var(--surface);
  box-shadow: -8px 0 8px -8px rgba(0,0,0,.18); }
tbody tr:hover td.actions { background: var(--surface-2); }
tbody tr.sel-keep td.actions { background: var(--keep-bg); }
tbody tr.sel-drop td.actions { background: var(--unfollow-bg); }
.seg { display:inline-flex; border:1px solid var(--border); border-radius: 999px; overflow:hidden; background: var(--surface); }
.seg button { appearance:none; border:0; background:transparent; cursor:pointer; font: inherit;
  font-size:.75rem; font-weight:500; color: var(--fg-2); padding: 5px 11px;
  display:inline-flex; align-items:center; gap:5px; min-height: 30px;
  transition: background .12s ease, color .12s ease; }
.seg button + button { border-left: 1px solid var(--border); }
.seg button svg { width:13px; height:13px; }
.seg button:hover { background: var(--surface-2); color: var(--fg); }
.seg button[aria-pressed=true].keep { background: var(--keep-line); color:#fff; }
.seg button[aria-pressed=true].drop { background: var(--unfollow-line); color:#fff; }
/* The dark pressed-button text tweaks now ship via dark_rules() so they fire
   for the manual [data-theme=dark] override, not just system dark. */
.empty-state { padding: var(--s8) var(--s5); text-align:center; color: var(--muted); font-style: italic; }
.exportbar { position: fixed; left: 50%; bottom: var(--s5);
  transform: translateX(-50%) translateY(160%); z-index: 50; display:flex;
  align-items:center; gap: var(--s4); background: var(--surface);
  border:1px solid var(--border);
  box-shadow: 0 12px 32px -8px rgba(0,0,0,.30), 0 4px 12px -4px rgba(0,0,0,.18);
  border-radius: var(--radius); padding: var(--s3) var(--s4);
  transition: transform .28s cubic-bezier(.2,.8,.2,1), opacity .28s ease;
  opacity: 0; max-width: calc(100vw - var(--s6)); }
.exportbar.show { transform: translateX(-50%) translateY(0); opacity: 1; }
.eb-counts { display:flex; align-items:center; gap: var(--s4); padding-left: var(--s1); }
.eb-count { display:inline-flex; align-items:baseline; gap: var(--s2);
  white-space: nowrap; font-size:.8125rem; color: var(--muted); }
.eb-count b { font-size:1.0625rem; font-weight:600; font-variant-numeric: tabular-nums;
  color: var(--fg); line-height:1; }
.eb-count-label { letter-spacing:.01em; }
.eb-dot { width:7px; height:7px; border-radius:50%; align-self:center; flex:none; }
.eb-count.keep .eb-dot { background: var(--keep-line); }
.eb-count.drop .eb-dot { background: var(--unfollow-line); }
.eb-count[data-empty=true] { opacity:.45; }
.eb-sep { width:1px; align-self:stretch; margin: calc(-1 * var(--s1)) 0; background: var(--border-soft); }
.eb-actions { display:flex; align-items:center; gap: var(--s3); }
.eb-seg { display:inline-flex; align-items:stretch; border-radius: var(--radius-sm); overflow:hidden; }
.eb-btn { appearance:none; border:0; font: inherit; font-size:.8125rem; font-weight:600;
  cursor:pointer; display:inline-flex; align-items:center; justify-content:center;
  gap: var(--s2); min-height:38px; transition: background .12s ease, filter .12s ease; }
.eb-btn svg { width:15px; height:15px; flex:none; }
.eb-copy { padding: 0 var(--s4); color:#fff; }
.eb-seg.keep .eb-copy { background: var(--keep-line); }
.eb-seg.drop .eb-copy { background: var(--unfollow-line); }
.eb-copy:hover { filter: brightness(1.06); }
.eb-dl { padding: 0 var(--s3); box-shadow: inset 1px 0 0 rgba(255,255,255,.25); }
.eb-seg.keep .eb-dl { background: var(--keep-bg); color: var(--keep-fg); }
.eb-seg.drop .eb-dl { background: var(--unfollow-bg); color: var(--unfollow-fg); }
.eb-seg.keep .eb-dl:hover { background: color-mix(in srgb, var(--keep-bg) 80%, var(--keep-line)); }
.eb-seg.drop .eb-dl:hover { background: color-mix(in srgb, var(--unfollow-bg) 80%, var(--unfollow-line)); }
.eb-clear { appearance:none; border:0; background:transparent; font: inherit;
  font-size:.8125rem; font-weight:500; color: var(--muted); cursor:pointer;
  padding: var(--s2) var(--s3); border-radius: var(--radius-sm); min-height:38px;
  transition: background .12s ease, color .12s ease; }
.eb-clear:hover { color: var(--fg); background: var(--surface-2); }
.eb-btn:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; border-radius: var(--radius-sm); position: relative; z-index: 1; }
.eb-clear:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.toast { position: fixed; left:50%; bottom: 92px; transform: translateX(-50%) translateY(10px);
  background: var(--fg); color: var(--bg); font-size:.8125rem; font-weight:500;
  padding: 8px 16px; border-radius: 999px; opacity:0; pointer-events:none;
  transition: opacity .2s ease, transform .2s ease; z-index:60; }
.toast.show { opacity:1; transform: translateX(-50%) translateY(0); }
@media (prefers-reduced-motion: reduce) { * { transition: none !important; animation: none !important; } }
@media (max-width: 860px) {
  .tiles { grid-template-columns: 1fr; }
  .why, thead th.col-why, td.why { display:none; }
  .head-row { align-items: center; }
  .theme-opt { height:44px; padding:0 var(--s2); }
  .exportbar { left: var(--s3); right: var(--s3); transform: translateY(160%);
    flex-wrap: wrap; justify-content:center; gap: var(--s3); max-width:none;
    padding: var(--s3); bottom: var(--s3); }
  .exportbar.show { transform: translateY(0); }
  .eb-sep { display:none; }
  .eb-counts { width:100%; justify-content:center; }
  .eb-actions { flex-wrap: wrap; justify-content:center; }
}
@media (max-width: 460px) {
  /* Very narrow: control floats above the title, labels go visually-hidden
     (SR + title tooltip keep them) so it never wraps under the eyebrow. */
  .head-row { flex-wrap: wrap; }
  .theme { order:-1; align-self: flex-end; }
  .theme-opt { width:44px; padding:0; justify-content:center; }
  .theme-opt span { position:absolute; width:1px; height:1px; overflow:hidden;
    clip:rect(0 0 0 0); clip-path: inset(50%); white-space:nowrap;
    border:0; padding:0; margin:-1px; }
}
";

/// Vanilla JS: filter (with live shown-count), keyboard-operable sort
/// (Enter/Space, `aria-sort`), per-row keep/drop toggles (mutually
/// exclusive, `localStorage`-persisted), and the floating export bar
/// (Copy primary + Download per list, Clear). No deps. Rows are
/// server-rendered, so the script never injects user data as HTML — it
/// reads handles from `data-` attributes (browser-unescaped) only to
/// build the export text / clipboard payload.
const SCRIPT: &str = "\
'use strict';
var STORE = 'igsift.triage.v1';
function loadSel(){ try { return JSON.parse(localStorage.getItem(STORE)) || {}; } catch(e){ return {}; } }
function saveSel(){ try { localStorage.setItem(STORE, JSON.stringify(sel)); } catch(e){} }
var sel = loadSel();

/* filter */
document.querySelectorAll('section').forEach(function(sec){
  var input = sec.querySelector('input[type=search]');
  var shown = sec.querySelector('[data-shown]');
  if(!input) return;
  input.addEventListener('input', function(){
    var q = input.value.toLowerCase(), n = 0;
    sec.querySelectorAll('tbody tr').forEach(function(tr){
      var hit = tr.textContent.toLowerCase().indexOf(q) !== -1;
      tr.style.display = hit ? '' : 'none';
      if(hit) n++;
    });
    if(shown) shown.textContent = n + ' shown';
  });
});

/* sort */
function cellVal(tr, idx, kind){
  if(kind === 'num'){
    if(idx === 2) return parseFloat(tr.dataset.p);
    if(idx === 3) return parseInt(tr.dataset.m, 10);
    if(idx === 5){ var v = parseFloat(tr.dataset.t); return isNaN(v) ? -Infinity : v; }
  }
  if(idx === 0) return (tr.dataset.h || '').toLowerCase();
  var cell = tr.children[idx];
  return (cell ? cell.textContent : '').trim().toLowerCase();
}
document.querySelectorAll('thead th.sortable').forEach(function(th){
  function run(){
    var table = th.closest('table'), tbody = table.querySelector('tbody');
    var idx = Array.prototype.indexOf.call(th.parentNode.children, th);
    var kind = th.dataset.kind;
    var asc = th.getAttribute('aria-sort') !== 'ascending';
    table.querySelectorAll('th').forEach(function(o){ o.setAttribute('aria-sort','none'); });
    th.setAttribute('aria-sort', asc ? 'ascending' : 'descending');
    var rows = Array.prototype.slice.call(tbody.querySelectorAll('tr'));
    rows.sort(function(a,b){
      var av = cellVal(a, idx, kind), bv = cellVal(b, idx, kind);
      if(kind === 'num') return asc ? av-bv : bv-av;
      return asc ? String(av).localeCompare(bv) : String(bv).localeCompare(av);
    });
    rows.forEach(function(r){ tbody.appendChild(r); });
  }
  th.addEventListener('click', run);
  th.addEventListener('keydown', function(e){ if(e.key==='Enter'||e.key===' '){ e.preventDefault(); run(); } });
});

/* per-row keep/drop toggles (mutually exclusive) */
function syncRow(tr){
  var h = tr.dataset.h, state = sel[h];
  tr.classList.toggle('sel-keep', state === 'keep');
  tr.classList.toggle('sel-drop', state === 'drop');
  tr.querySelectorAll('button[data-toggle]').forEach(function(b){
    b.setAttribute('aria-pressed', String(state === b.dataset.toggle));
  });
}
var main = document.getElementById('main');
main.addEventListener('click', function(e){
  var btn = e.target.closest('button[data-toggle]');
  if(!btn) return;
  var tr = btn.closest('tr'), h = tr.dataset.h, want = btn.dataset.toggle;
  if(sel[h] === want) delete sel[h]; else sel[h] = want;
  saveSel(); syncRow(tr); renderBar();
});

/* export bar — resolve all anchors once; renderBar runs on every toggle */
var bar = document.getElementById('exportbar');
var kc = document.getElementById('kc'), dc = document.getElementById('dc');
var kCount = kc.closest('.eb-count'), dCount = dc.closest('.eb-count');
function listOf(kind){ return Object.keys(sel).filter(function(h){ return sel[h] === kind; }).sort(); }
function renderBar(){
  var k = listOf('keep'), d = listOf('drop');
  kc.textContent = k.length;
  dc.textContent = d.length;
  // Dim the list with no selections so the active one stands out.
  kCount.setAttribute('data-empty', String(k.length === 0));
  dCount.setAttribute('data-empty', String(d.length === 0));
  bar.classList.toggle('show', (k.length + d.length) > 0);
}
function fileText(kind){
  var file = kind === 'keep' ? 'keeplist.txt' : 'droplist.txt';
  var header = '# igsift ' + (kind==='keep'?'keeplist':'droplist')
    + ' — append to config/' + file + '\\n# one handle per line\\n';
  return header + listOf(kind).join('\\n') + '\\n';
}
function toast(msg){
  var t = document.getElementById('toast');
  t.textContent = msg; t.classList.add('show');
  clearTimeout(t._t); t._t = setTimeout(function(){ t.classList.remove('show'); }, 1800);
}
bar.addEventListener('click', function(e){
  var btn = e.target.closest('button[data-act]');
  if(!btn) return;
  if(btn.dataset.act === 'copy' || btn.dataset.act === 'download'){
    var kind = btn.dataset.list, name = kind==='keep' ? 'keeplist' : 'droplist';
    if(listOf(kind).length === 0){ toast('No ' + name + ' selections yet'); return; }
    if(btn.dataset.act === 'copy'){
      navigator.clipboard.writeText(fileText(kind)).then(
        function(){ toast('Copied ' + listOf(kind).length + ' handles — paste into config/' + name + '.txt'); },
        function(){ toast('Copy failed — use .txt instead'); }
      );
    } else {
      var blob = new Blob([fileText(kind)], { type:'text/plain' });
      var a = document.createElement('a');
      a.href = URL.createObjectURL(blob); a.download = name + '.append.txt';
      document.body.appendChild(a); a.click(); a.remove();
      setTimeout(function(){ URL.revokeObjectURL(a.href); }, 1000);
      toast('Downloaded ' + name + '.append.txt');
    }
  }
});
document.getElementById('clearAll').addEventListener('click', function(){
  sel = {}; saveSel();
  document.querySelectorAll('tbody tr').forEach(syncRow);
  renderBar(); toast('Cleared all selections');
});

/* restore persisted selections on load */
document.querySelectorAll('tbody tr').forEach(syncRow);
renderBar();

/* theme switcher — radiogroup: one tab stop (the checked radio), arrows move
   selection and apply immediately (selection-follows-focus), wrapping at ends.
   Persists to igsift.theme.v1; the head boot script already applied it. */
var TKEY = 'igsift.theme.v1';
var tg = document.querySelector('.theme');
/* persist=false on the load-time reconcile so a never-touched switcher doesn't
   pin 'auto' into localStorage; only real user actions write. */
function setTheme(v, persist){
  document.documentElement.setAttribute('data-theme', v);
  if(persist){ try { localStorage.setItem(TKEY, v); } catch(e){} }
  tg.querySelectorAll('[role=radio]').forEach(function(b){
    var on = b.dataset.themeSet === v;
    b.setAttribute('aria-checked', String(on));
    b.tabIndex = on ? 0 : -1;
  });
}
tg.addEventListener('click', function(e){
  var b = e.target.closest('[role=radio]');
  if(b) setTheme(b.dataset.themeSet, true);
});
tg.addEventListener('keydown', function(e){
  var ks = ['ArrowRight','ArrowDown','ArrowLeft','ArrowUp'];
  if(ks.indexOf(e.key) < 0) return;
  e.preventDefault();
  var opts = Array.prototype.slice.call(tg.querySelectorAll('[role=radio]'));
  var i = opts.findIndex(function(b){ return b.getAttribute('aria-checked') === 'true'; });
  var d = (e.key === 'ArrowRight' || e.key === 'ArrowDown') ? 1 : -1;
  var n = (i + d + opts.length) % opts.length;
  setTheme(opts[n].dataset.themeSet, true); opts[n].focus();
});
/* reconcile aria-checked/tabindex with whatever the boot script set on <html> */
setTheme(document.documentElement.getAttribute('data-theme') || 'auto', false);
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
        assert!(html.contains("<section data-bucket=\"unfollow\">"));
        assert!(html.contains("<section data-bucket=\"review\">"));
        assert!(html.contains("<section data-bucket=\"keep\">"));
        // Section pill counts.
        assert!(html.contains("Unfollow <span class=\"pill\">1</span>"));
        assert!(html.contains("Review <span class=\"pill\">1</span>"));
        assert!(html.contains("Keep <span class=\"pill\">2</span>"));
        assert!(html.contains("4 accounts scored"));
    }

    #[test]
    fn header_stat_tiles_show_per_bucket_counts() {
        // Tiles compute keep/review/unfollow via `== Bucket::X`. Asymmetric
        // counts (3/1/1) so a `==`→`!=` mutation changes each tile number.
        let scored = vec![
            baseline("k1", Bucket::Keep, 0.90),
            baseline("k2", Bucket::Keep, 0.92),
            baseline("k3", Bucket::Keep, 0.95),
            baseline("r1", Bucket::Review, 0.50),
            baseline("u1", Bucket::Unfollow, 0.10),
        ];
        let html = render(&scored);
        assert!(html.contains("<div class=\"tile keep\">"));
        assert!(html.contains("<div class=\"num\">3</div>"));
        assert!(html.contains("<div class=\"num\">1</div>"));
    }

    #[test]
    fn keep_likelihood_renders_as_percentage_with_raw_in_title() {
        // The user-facing value is an integer percent; the raw float is
        // preserved in the title (power users) AND in data-p (exact sort).
        let scored = vec![baseline("acct", Bucket::Keep, 0.873)];
        let html = render(&scored);
        assert!(
            html.contains("Keep likelihood"),
            "column header renamed: {html}"
        );
        assert!(html.contains(">87%<"), "percent cell: {html}");
        assert!(
            html.contains("title=\"raw keep_prob = 0.873\""),
            "raw float in title: {html}"
        );
        assert!(
            html.contains("data-p=\"0.873\""),
            "exact float for sort: {html}"
        );
    }

    #[test]
    fn rows_carry_keep_drop_toggles_and_store_key() {
        let scored = vec![baseline("acct", Bucket::Unfollow, 0.2)];
        let html = render(&scored);
        assert!(html.contains("data-toggle=\"keep\""));
        assert!(html.contains("data-toggle=\"drop\""));
        assert!(html.contains("aria-pressed=\"false\""));
        // The client-side store the toggles write to.
        assert!(html.contains("igsift.triage.v1"));
    }

    #[test]
    fn export_bar_markup_is_fully_wired() {
        // The export bar is driven entirely by JS that keys off a fixed set
        // of IDs and data-attributes. The JS can't be unit-tested here, so
        // pin the contract markup: dropping any of these breaks the feature
        // at runtime but would otherwise pass silently.
        let html = render(&[baseline("acct", Bucket::Unfollow, 0.2)]);

        // The element IDs renderBar()/clearAll/listOf resolve. A missing
        // #kc/#dc throws on load (getElementById(...).textContent); a missing
        // #clearAll throws when wiring its listener.
        for id in [
            "id=\"exportbar\"",
            "id=\"kc\"",
            "id=\"dc\"",
            "id=\"clearAll\"",
        ] {
            assert!(html.contains(id), "missing {id}: {html}");
        }

        // All four action buttons: the click handler dispatches on
        // (data-act, data-list). The pair must be emitted adjacent in this
        // order (see write_export_bar).
        for (act, list) in [
            ("copy", "keep"),
            ("download", "keep"),
            ("copy", "drop"),
            ("download", "drop"),
        ] {
            assert!(
                html.contains(&format!("data-act=\"{act}\" data-list=\"{list}\"")),
                "missing {act}/{list} button: {html}",
            );
        }

        // Copy is the emphasized primary; the download button is icon-only,
        // so it must carry an accessible label.
        assert!(html.contains("class=\"eb-btn eb-copy\" data-act=\"copy\""));
        assert!(html.contains("aria-label=\"Download keeplist as .txt\""));

        // renderBar() calls kc.closest('.eb-count') — pin that #kc/#dc sit
        // inside an .eb-count wrapper, and that both ship dimmed at rest.
        assert!(html.contains("class=\"eb-count keep\" data-empty=\"true\""));
        assert!(html.contains("class=\"eb-count drop\" data-empty=\"true\""));
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
    fn escapes_double_quote_in_display_name() {
        // The attribute-context guard. Deleting the `"` arm from `escape`
        // would let a display name break out of an HTML attribute.
        let mut s = baseline("alice", Bucket::Keep, 0.9);
        s.features.display_name = Some("Sarah \"Q\" Connor".to_owned());
        let html = render(&[s]);
        assert!(
            html.contains("Sarah &quot;Q&quot; Connor"),
            "double quote must be escaped to &quot;: {html}",
        );
    }

    #[test]
    fn escapes_special_characters_in_handle_href() {
        // The handle flows into the `href` attribute AND the `data-h`
        // attribute via `escape`. A handle carrying `"` (schema drift / a
        // corrupted export; IG's own charset never emits it) would
        // otherwise break out of either attribute. Pin both.
        let s = baseline("a\"b", Bucket::Keep, 0.9);
        let html = render(&[s]);
        assert!(
            html.contains("https://www.instagram.com/a&quot;b/"),
            "handle in href must be escaped: {html}",
        );
        assert!(
            !html.contains("/a\"b/"),
            "raw double-quote must not reach the href attribute: {html}",
        );
        assert!(
            !html.contains("data-h=\"a\"b\""),
            "raw double-quote must not break the data-h attribute: {html}",
        );
    }

    #[test]
    fn html_root_defaults_to_auto_theme() {
        // The emitted <html> ships data-theme="auto" so a JS-disabled view
        // still tracks the system via the prefers-color-scheme media query.
        let html = render(&[baseline("a", Bucket::Keep, 0.9)]);
        assert!(
            html.contains("<html lang=\"en\" data-theme=\"auto\">"),
            "root must default to auto theme: {html}",
        );
    }

    #[test]
    fn theme_switcher_is_a_three_state_radiogroup_with_auto_selected() {
        let html = render(&[baseline("a", Bucket::Keep, 0.9)]);
        assert!(html.contains("role=\"radiogroup\""), "radiogroup: {html}");
        for v in ["auto", "light", "dark"] {
            assert!(
                html.contains(&format!("data-theme-set=\"{v}\"")),
                "missing {v} radio: {html}",
            );
        }
        // Exactly one radio is checked at rest, and it's Auto (radiogroup
        // contract: one-of-N). aria-pressed (triage) is a separate attribute.
        assert_eq!(
            html.matches("aria-checked=\"true\"").count(),
            1,
            "exactly one theme radio may be checked: {html}",
        );
        assert!(
            html.contains("data-theme-set=\"auto\" aria-checked=\"true\""),
            "auto must be the checked default: {html}",
        );
    }

    #[test]
    fn theme_boot_script_precedes_stylesheet_to_avoid_flash() {
        // Anti-FOUC: the persisted choice must land on <html> before the
        // stylesheet paints, else a saved Dark flashes light on load.
        let html = render(&[baseline("a", Bucket::Keep, 0.9)]);
        let boot = html
            .find("igsift.theme.v1")
            .expect("theme localStorage key present");
        let style = html.find("<style>").expect("stylesheet present");
        assert!(
            boot < style,
            "theme boot script must precede <style>: boot={boot} style={style}",
        );
    }

    #[test]
    fn dark_tokens_apply_to_manual_override_not_only_system_dark() {
        // The regression a naive manual switcher introduces: dark tokens stay
        // trapped inside @media(prefers-color-scheme:dark), so picking Dark on
        // a light system does nothing. Pin that the dark token block is also
        // emitted under the explicit [data-theme="dark"] selector, and that
        // auto still defers to the media query.
        let html = render(&[baseline("a", Bucket::Keep, 0.9)]);
        assert!(
            html.contains(":root[data-theme=\"dark\"]"),
            "explicit dark override selector missing: {html}",
        );
        assert!(
            html.contains(":root[data-theme=\"auto\"]"),
            "auto-defers-to-system selector missing: {html}",
        );
        assert!(html.contains("@media (prefers-color-scheme: dark)"));
        // The dark surface value must appear under the explicit-dark selector,
        // not solely the media query — assert it occurs at least twice (once
        // per emit site) so a single-site regression trips.
        assert!(
            html.matches("--surface:#1f1f22").count() >= 2,
            "dark --surface must be emitted for both [data-theme=dark] and auto/media: {html}",
        );
        // New shared token the segmented thumb keys off.
        assert!(html.contains("--seg-thumb"), "seg-thumb token: {html}");
    }

    #[test]
    fn empty_input_renders_three_empty_sections() {
        let html = render(&[]);
        assert!(html.contains("0 accounts scored"));
        assert!(html.contains("Unfollow <span class=\"pill\">0</span>"));
        assert!(html.contains("Nothing in this bucket"));
    }
}
