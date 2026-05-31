//! Terminal styling vocabulary: capability detection, a semantic color
//! palette, glyph sets with ASCII fallback, and pure box/bar renderers.
//! Every renderer takes `Caps` explicitly so behavior is testable without
//! a real terminal. The single styling site for `summary` and `labels` —
//! they share this module so their output cannot drift apart.

use crate::scoring::Bucket;
use anstyle::{AnsiColor, Style};

/// Rendering capabilities, resolved once at the edge from the environment
/// and the `--color` choice.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub color: bool,
    pub unicode: bool,
    pub width: usize,
}

impl Caps {
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
        let title_seg = if title_seg.chars().count() > inner {
            truncate_with_ellipsis(&title_seg, inner)
        } else {
            title_seg
        };
        let title_len = title_seg.chars().count();
        let fill = inner.saturating_sub(title_len);
        rows.push(format!("{tl}{title_seg}{}{tr}", h.to_string().repeat(fill)));

        // Body rows: `│ <body><pad>│`
        // Each body row = v(1) + space(1) + body + pad + v(1) = width chars total.
        // So body + pad must be exactly `inner - 1` chars (inner already excludes
        // the two side borders; we subtract the leading space).
        let body_width = inner.saturating_sub(1); // space before body is fixed
        for line in lines {
            let body = truncate_with_ellipsis(line, body_width);
            let body_len = body.chars().count();
            let pad = body_width.saturating_sub(body_len);
            rows.push(format!("{v} {body}{}{v}", " ".repeat(pad)));
        }

        // Bottom border: `╰──…──╯`
        rows.push(format!("{bl}{}{br}", h.to_string().repeat(inner)));
        rows
    }
}

/// Truncate `s` to at most `max` columns, appending `…` when cut.
fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
