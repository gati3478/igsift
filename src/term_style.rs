//! Terminal styling vocabulary: capability detection, a semantic color
//! palette, glyph sets with ASCII fallback, and pure box/bar renderers.
//! Every renderer takes `Caps` explicitly so behavior is testable without
//! a real terminal. The single styling site for `summary` and `labels` —
//! they share this module so their output cannot drift apart.

use crate::cli::ColorChoice;
use crate::scoring::Bucket;
use anstyle::{AnsiColor, Style};
use unicode_width::UnicodeWidthStr;

/// True when the active locale advertises UTF-8. Honors POSIX precedence for
/// the character-encoding category: `LC_ALL` overrides `LC_CTYPE` overrides
/// `LANG` — the first one that is **set and non-empty** decides, so forcing
/// `LC_ALL=C` correctly yields ASCII even when `LANG` is a UTF-8 locale
/// (otherwise igsift would emit Unicode that mojibakes on the C locale).
fn locale_is_utf8() -> bool {
    locale_is_utf8_from(|k| std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()))
}

/// Pure core of [`locale_is_utf8`], parameterized over the env lookup so the
/// POSIX precedence is unit-testable without mutating process environment.
fn locale_is_utf8_from(get: impl Fn(&str) -> Option<String>) -> bool {
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        match get(key) {
            Some(v) if !v.is_empty() => {
                let v = v.to_ascii_lowercase();
                return v.contains("utf-8") || v.contains("utf8");
            }
            _ => continue,
        }
    }
    false
}

/// Rendering capabilities, resolved once at the edge from the environment
/// and the `--color` choice.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub color: bool,
    pub unicode: bool,
    pub width: usize,
}

impl Caps {
    /// Resolve capabilities once from stdout + environment + `--color`.
    ///
    /// - `color`: `Always` → true; `Never` → false; `Auto` → stdout is a
    ///   TTY AND `NO_COLOR` is unset AND `TERM != "dumb"`.
    /// - `unicode`: a UTF-8 locale is advertised (`LC_ALL`/`LC_CTYPE`/`LANG`
    ///   contains "UTF-8"/"utf8"), or on Windows when `WT_SESSION` is set
    ///   (Windows Terminal). Otherwise ASCII.
    /// - `width`: terminal width via `console`, clamped to `[40, 100]`,
    ///   defaulting to 80 when undetectable (piped / non-TTY).
    pub fn detect(choice: ColorChoice) -> Self {
        use std::io::IsTerminal as _;

        let is_tty = std::io::stdout().is_terminal();
        let color = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                is_tty
                    && std::env::var_os("NO_COLOR").is_none()
                    && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
            }
        };

        let unicode = locale_is_utf8() || std::env::var_os("WT_SESSION").is_some();

        let width = console::Term::stdout()
            .size_checked()
            .map(|(_h, w)| usize::from(w))
            .unwrap_or(80)
            .clamp(40, 100);

        Self {
            color,
            unicode,
            width,
        }
    }

    /// Style for a bucket's glyph + emphasis. Green/yellow/red, semantic.
    pub fn bucket_style(&self, bucket: Bucket) -> Style {
        let c = match bucket {
            Bucket::Keep => AnsiColor::Green,
            Bucket::Review => AnsiColor::Yellow,
            Bucket::Unfollow => AnsiColor::Red,
        };
        Style::new().fg_color(Some(c.into()))
    }

    /// Convenience alias for [`Self::bucket_style`] with [`Bucket::Keep`].
    pub fn keep_style(&self) -> Style {
        self.bucket_style(Bucket::Keep)
    }

    /// Dim style for chrome (frames, rules, secondary text).
    pub fn dim_style(&self) -> Style {
        Style::new().dimmed()
    }

    /// Bold style for titles and counts.
    pub fn bold_style(&self) -> Style {
        Style::new().bold()
    }

    /// Wrap `text` in `style`'s ANSI codes, or return it unchanged when
    /// `color` is off. The single coloring chokepoint — nothing else in
    /// the crate emits escape bytes.
    pub fn paint(&self, text: &str, style: Style) -> String {
        if self.color {
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_owned()
        }
    }

    /// Success marker: `✓` (unicode) / `OK` (ascii).
    pub fn check_glyph(&self) -> &'static str {
        if self.unicode { "✓" } else { "OK" }
    }

    /// Failure marker: `✗` (unicode) / `X` (ascii).
    pub fn cross_glyph(&self) -> &'static str {
        if self.unicode { "✗" } else { "X" }
    }

    /// Filled glyph for a bucket dot. ASCII fallback when `unicode` is off.
    pub fn bucket_glyph(&self, bucket: Bucket) -> &'static str {
        match (bucket, self.unicode) {
            (Bucket::Keep, true) => "●",
            (Bucket::Keep, false) => "o",
            (Bucket::Review, true) => "◐",
            (Bucket::Review, false) => "*",
            (Bucket::Unfollow, true) => "○",
            (Bucket::Unfollow, false) => ".",
        }
    }

    /// Proportional horizontal bar of `width` cells. Under unicode, uses
    /// full block `█` plus an eighth-block remainder (`▏…▉`) so small
    /// values still register a sliver; ASCII uses `#`. Always padded with
    /// spaces to exactly `width` columns. `max == 0` → empty bar (no
    /// divide-by-zero). Never exceeds `width`.
    pub fn bar(&self, value: u32, max: u32, width: usize) -> String {
        if max == 0 || width == 0 {
            return " ".repeat(width);
        }
        let frac = (f64::from(value) / f64::from(max)).clamp(0.0, 1.0);
        let total_eighths = (frac * (width as f64) * 8.0).round() as usize;
        let full = (total_eighths / 8).min(width);
        let rem = total_eighths % 8;

        let mut s = String::with_capacity(width * 3);
        if self.unicode {
            s.push_str(&"█".repeat(full));
            if full < width && rem > 0 {
                // Fill increases with rem: ▏(1/8) .. ▉(7/8). Codepoints run heavy→light
                // in Unicode (▉=U+2589 .. ▏=U+258F), so the array lists them explicitly.
                let eighth = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'][rem - 1];
                s.push(eighth);
            }
        } else {
            s.push_str(&"#".repeat(full));
        }
        let used = s.chars().count();
        if used < width {
            s.push_str(&" ".repeat(width - used));
        }
        s
    }

    /// Frame `lines` in a titled card of total outer `width`. Returns the
    /// rows so two cards can be placed side by side by zipping their
    /// vectors. Body lines longer than the inner width are truncated with
    /// `…`. Frame glyphs fall back to `+ - |` under ASCII.
    pub fn boxed(&self, title: &str, lines: &[String], width: usize) -> Vec<String> {
        let (tl, tr, bl, br, h, v) = if self.unicode {
            ('╭', '╮', '╰', '╯', '─', '│')
        } else {
            ('+', '+', '+', '+', '-', '|')
        };
        // inner = space between the two side-border chars.
        let inner = width.saturating_sub(2);
        let mut rows = Vec::with_capacity(lines.len() + 2);

        // Top border: `╭─ Title ──…──╮`
        // title_seg = "─ Title " (h + space + title + space)
        let title_seg = format!("{h} {title} ");
        // Guard: clamp to `inner` so an overlong title never widens the top border.
        let title_seg = if display_width(&title_seg) > inner {
            truncate_with_ellipsis(&title_seg, inner, self.unicode)
        } else {
            title_seg
        };
        let title_len = display_width(&title_seg);
        let fill = inner.saturating_sub(title_len);
        rows.push(format!("{tl}{title_seg}{}{tr}", h.to_string().repeat(fill)));

        // Body rows: `│ <body><pad>│`
        // Each body row = v(1) + space(1) + body + pad + v(1) = width chars total.
        // So body + pad must be exactly `inner - 1` chars (inner already excludes
        // the two side borders; we subtract the leading space).
        let body_width = inner.saturating_sub(1); // space before body is fixed
        for line in lines {
            let body = truncate_with_ellipsis(line, body_width, self.unicode);
            let body_len = display_width(&body);
            let pad = body_width.saturating_sub(body_len);
            rows.push(format!("{v} {body}{}{v}", " ".repeat(pad)));
        }

        // Bottom border: `╰──…──╯`
        rows.push(format!("{bl}{}{br}", h.to_string().repeat(inner)));
        rows
    }
}

/// Display width of `s` in terminal columns: CJK/wide chars count as 2,
/// combining/zero-width as 0. Plain text ONLY — never pass an ANSI-painted
/// string (escape sequences would be miscounted, since `[32m` is printable).
/// All layout measurement in this module and in `summary` goes through this,
/// so a non-ASCII handle or `--config` path can't skew a box or a column.
pub(crate) fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Replace control characters (TAB, newline, ESC, …) in arbitrary display
/// content with `?`. Such bytes are legal in Unix paths (e.g. a `--config`
/// path), but rendered raw they break a box — a newline splits the line, a
/// TAB jumps to a tab stop, neither matching any column count. Sanitize at the
/// boundary so both our width math and the terminal stay honest.
pub(crate) fn sanitize_display(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Truncate `s` to at most `max` DISPLAY columns, appending an ellipsis
/// marker when cut (`…` under unicode, `...` under ASCII). A wide char that
/// would overflow the budget is dropped whole (never split mid-cell), so the
/// result is always `<= max` columns.
fn truncate_with_ellipsis(s: &str, max: usize, unicode: bool) -> String {
    if display_width(s) <= max {
        return s.to_owned();
    }
    let marker = if unicode { "…" } else { "..." };
    let marker_w = display_width(marker); // 1 or 3 columns
    if max <= marker_w {
        // No room for content; emit as much of the marker as fits by columns.
        return take_columns(marker, max);
    }
    let kept = take_columns(s, max - marker_w);
    format!("{kept}{marker}")
}

/// Greedily take whole chars from `s` until adding the next would exceed
/// `budget` display columns. Measures via [`display_width`] — the same metric
/// `boxed` re-measures with — so the two never disagree (a per-char width
/// table would diverge from the string-level one on control chars and
/// grapheme clusters). Never splits a multi-column char.
fn take_columns(s: &str, budget: usize) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        out.push(ch);
        if display_width(&out) > budget {
            out.pop();
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_never_disables_color() {
        let caps = Caps::detect(ColorChoice::Never);
        assert!(!caps.color);
    }

    #[test]
    fn detect_always_enables_color() {
        let caps = Caps::detect(ColorChoice::Always);
        assert!(caps.color);
    }

    #[test]
    fn detect_width_is_sane() {
        let caps = Caps::detect(ColorChoice::Never);
        assert!(
            (40..=100).contains(&caps.width),
            "width clamped: {}",
            caps.width
        );
    }

    #[test]
    fn glyphs_flip_with_unicode() {
        let uni = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let asc = Caps {
            color: false,
            unicode: false,
            width: 80,
        };
        assert_eq!(uni.bucket_glyph(Bucket::Keep), "●");
        assert_eq!(asc.bucket_glyph(Bucket::Keep), "o");
        assert_eq!(uni.bucket_glyph(Bucket::Review), "◐");
        assert_eq!(asc.bucket_glyph(Bucket::Review), "*");
        assert_eq!(uni.bucket_glyph(Bucket::Unfollow), "○");
        assert_eq!(asc.bucket_glyph(Bucket::Unfollow), ".");
    }

    #[test]
    fn paint_is_noop_when_color_off() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let out = caps.paint("hello", caps.keep_style());
        assert_eq!(out, "hello");
        assert!(!out.contains('\u{1b}'), "no ESC bytes when color off");
    }

    #[test]
    fn paint_wraps_when_color_on() {
        let caps = Caps {
            color: true,
            unicode: true,
            width: 80,
        };
        let out = caps.paint("hi", caps.keep_style());
        assert!(out.contains('\u{1b}'), "ESC present when color on");
        assert!(
            out.ends_with("hi\u{1b}[0m"),
            "text should be wrapped with a reset suffix"
        );
    }

    #[test]
    fn bar_proportions() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        // Full: width 10, value == max → 10 full blocks.
        assert_eq!(caps.bar(40, 40, 10).chars().count(), 10);
        // Empty: value 0 → no fill, padded to width with spaces.
        assert_eq!(caps.bar(0, 40, 10), " ".repeat(10));
        // max == 0 must not divide by zero — empty bar.
        assert_eq!(caps.bar(5, 0, 10), " ".repeat(10));
        // Never exceeds width.
        assert!(caps.bar(100, 40, 10).chars().count() <= 10);
    }

    #[test]
    fn bar_ascii_fallback() {
        let caps = Caps {
            color: false,
            unicode: false,
            width: 80,
        };
        let b = caps.bar(40, 40, 6);
        assert!(b.starts_with('#'), "ascii fill uses '#': {b:?}");
        assert!(!b.contains('█'));
    }

    #[test]
    fn boxed_frames_and_pads() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let rows = caps.boxed("Title", &["ab".to_string()], 12);
        assert_eq!(rows.len(), 3, "top + 1 body + bottom");
        assert!(rows[0].starts_with('╭') && rows[0].contains("Title"));
        assert!(rows[2].starts_with('╰'));
        // Inner content width is consistent across rows.
        let w = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == w), "ragged box");
    }

    #[test]
    fn boxed_truncates_overlong_line() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let rows = caps.boxed("T", &["x".repeat(50)], 12);
        assert!(
            rows[1].contains('…'),
            "overlong body line truncates: {:?}",
            rows[1]
        );
        assert!(rows[1].chars().count() <= 12);
    }

    #[test]
    fn boxed_ascii_fallback() {
        let caps = Caps {
            color: false,
            unicode: false,
            width: 80,
        };
        let rows = caps.boxed("T", &["a".to_string()], 10);
        assert!(rows[0].starts_with('+') && rows[0].contains('-'));
        assert!(rows[2].starts_with('+'));
        assert!(rows[1].starts_with('|'));
    }

    #[test]
    fn boxed_ascii_truncation_uses_dots_not_ellipsis() {
        let caps = Caps {
            color: false,
            unicode: false,
            width: 80,
        };
        let rows = caps.boxed("T", &["x".repeat(40)], 14);
        assert!(
            rows[1].contains("..."),
            "ascii truncation uses '...': {:?}",
            rows[1]
        );
        assert!(!rows[1].contains('…'), "no unicode ellipsis in ascii mode");
        // all rows still equal width
        let w = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == w));
    }

    #[test]
    fn boxed_long_title_stays_within_width() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let rows = caps.boxed("A very long card title indeed", &["x".into()], 16);
        let expected_width = 16;
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(
                row.chars().count(),
                expected_width,
                "row {i} has wrong width: {row:?}"
            );
        }
    }

    #[test]
    fn status_glyphs_flip_with_unicode() {
        let uni = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let asc = Caps {
            color: false,
            unicode: false,
            width: 80,
        };
        assert_eq!(uni.check_glyph(), "✓");
        assert_eq!(asc.check_glyph(), "OK");
        assert_eq!(uni.cross_glyph(), "✗");
        assert_eq!(asc.cross_glyph(), "X");
    }

    #[test]
    fn bar_renders_subcell_sliver() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let b = caps.bar(1, 100, 10);
        assert!(
            b.contains('▏'),
            "tiny fraction shows an eighth-block sliver: {b:?}"
        );
        assert!(!b.contains('█'), "too small for a full block: {b:?}");
        assert_eq!(b.chars().count(), 10);
    }

    #[test]
    fn sanitize_display_replaces_control_chars() {
        assert_eq!(sanitize_display("a\tb\nc"), "a?b?c");
        assert_eq!(sanitize_display("normal/path.toml"), "normal/path.toml");
        assert_eq!(sanitize_display("配置.toml"), "配置.toml"); // non-control unicode kept
        assert_eq!(sanitize_display("x\u{1b}[0m"), "x?[0m"); // ESC stripped
    }

    #[test]
    fn boxed_stays_rectangular_with_control_chars() {
        // Defense-in-depth: even if a control char reaches boxed, take_columns
        // and the re-measure share one metric, so rows stay equal display width
        // (content is sanitized upstream, but the primitive must not break).
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        let rows = caps.boxed("T", &["aa\tbb\tcc\tdd\tee\tff\tgg\thh".to_string()], 16);
        let widths: Vec<usize> = rows.iter().map(|r| display_width(r)).collect();
        assert!(
            widths.iter().all(|&w| w == 16),
            "rows must share one display width: {widths:?}"
        );
    }

    #[test]
    fn locale_precedence_lc_all_overrides_lang() {
        fn lookup(map: &[(&str, &str)], k: &str) -> Option<String> {
            map.iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| (*v).to_owned())
        }
        // LC_ALL=C wins over a UTF-8 LANG → ASCII.
        let m = [("LC_ALL", "C"), ("LANG", "en_US.UTF-8")];
        assert!(!locale_is_utf8_from(|k| lookup(&m, k)));
        // LC_CTYPE wins over LANG when LC_ALL is unset.
        let m = [("LC_CTYPE", "en_US.UTF-8"), ("LANG", "C")];
        assert!(locale_is_utf8_from(|k| lookup(&m, k)));
        // An empty higher-precedence var is skipped, not treated as "C".
        let m = [("LC_ALL", ""), ("LANG", "C.UTF-8")];
        assert!(locale_is_utf8_from(|k| lookup(&m, k)));
        // Falls back to LANG when the others are unset.
        let m = [("LANG", "fr_FR.utf8")];
        assert!(locale_is_utf8_from(|k| lookup(&m, k)));
        // Nothing set → ASCII.
        let m: [(&str, &str); 0] = [];
        assert!(!locale_is_utf8_from(|k| lookup(&m, k)));
    }

    #[test]
    fn display_width_counts_columns_not_codepoints() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("王小明"), 6); // 3 CJK chars × 2 cols
        assert_eq!(display_width("café"), 4); // precomposed é = 1 col
        assert_eq!(display_width("e\u{0301}"), 1); // e + combining acute = 1 col
        assert_eq!(display_width("😀"), 2); // emoji = 2 cols
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn boxed_wide_chars_keep_equal_display_width() {
        let caps = Caps {
            color: false,
            unicode: true,
            width: 80,
        };
        // CJK title + CJK body — codepoint counts differ from display widths.
        let rows = caps.boxed("账号", &["王小明的账号".to_string()], 20);
        let widths: Vec<usize> = rows.iter().map(|r| display_width(r)).collect();
        assert!(
            widths.iter().all(|&w| w == widths[0]),
            "all rows must share one DISPLAY width, got {widths:?}"
        );
        assert_eq!(widths[0], 20, "box should be exactly the requested 20 cols");
    }

    #[test]
    fn truncate_respects_display_columns_for_wide_chars() {
        // 5 CJK chars = 10 cols. Truncate to 6 cols: marker `…` (1 col) →
        // budget 5 → 2 CJK chars (4 cols) + `…`, total 5 cols ≤ 6.
        let out = truncate_with_ellipsis("王小明的号", 6, true);
        assert!(out.ends_with('…'), "keeps the ellipsis: {out:?}");
        assert!(
            display_width(&out) <= 6,
            "must not exceed 6 display cols, got {} ({out:?})",
            display_width(&out)
        );
        // A wide char is never split mid-cell.
        assert!(!out.contains('的') || display_width(&out) <= 6);
        // Short-enough strings are returned unchanged.
        assert_eq!(truncate_with_ellipsis("王", 6, true), "王");
    }
}
