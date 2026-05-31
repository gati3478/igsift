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
}
