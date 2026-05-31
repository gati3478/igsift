# Terminal Run-Summary Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat `println!` run-summary stdout with a polished, pipe-safe dashboard (header banner, colored bucket bars, compact histogram, side-by-side keep/unfollow cards, framed accuracy block).

**Architecture:** Two new modules — `term_style` (capability detection + theme + pure renderers) and `summary` (assembles the dashboard) — plus caps-aware edits to `labels::report`, a `--color` flag in `cli`, and a wiring change in `lib::run`. All rendering is pure given an explicit `Caps`; detection happens once at the edge. Presentation-only: no scoring/parser/output-writer changes.

**Tech Stack:** Rust edition 2024, `anstyle` (ANSI styling, promoted from transitive→direct dep via clap), `console` (terminal width, promoted from transitive→direct via indicatif), `std::io::IsTerminal` (TTY detection), `clap` derive (`--color` ValueEnum). Tests: `cargo nextest` + `assert_cmd`/`predicates`.

**Reference:** Design spec at `docs/specs/2026-05-31-terminal-summary-dashboard-design.md`.

---

## File Structure

| File                      | Responsibility                                                                                                                                                        |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/term_style.rs` (NEW) | `Caps`, `ColorChoice`, semantic palette + glyph sets, pure renderers (`bar`, `boxed`, `hrule`, `kv`). Knows nothing about scoring.                                    |
| `src/summary.rs` (NEW)    | `RunMeta` + `render()` — owns the dashboard layout. Consumes `ScoredAccount` + `Caps`. Hosts the relocated histogram.                                                 |
| `src/labels.rs` (EDIT)    | `compute()` unchanged (accuracy oracle); `report()` becomes caps-aware and renders via `term_style`.                                                                  |
| `src/cli.rs` (EDIT)       | Add `--color auto\|always\|never` (`ColorChoice` ValueEnum) to `RunArgs`.                                                                                             |
| `src/lib.rs` (EDIT)       | `run()` builds `Caps` once, calls `summary::render(...)`, passes `caps` to `labels::report`; inline bucket/top/bottom prints and `print_keep_prob_histogram` removed. |
| `Cargo.toml` (EDIT)       | Promote `anstyle` + `console` to direct deps.                                                                                                                         |
| `tests/cli.rs` (EDIT)     | Pipe-safety (ESC-free) + `--color never`/`always` integration tests.                                                                                                  |

`term_style` is the shared vocabulary so `summary` and `labels` cannot drift visually. `lib.rs::run` stays a thin orchestrator.

---

## Task 1: Promote dependencies + add ColorChoice enum

**Files:**

- Modify: `Cargo.toml` (`[dependencies]`)
- Modify: `src/cli.rs` (add `ColorChoice` enum + `--color` field)

- [ ] **Step 1: Add direct deps to `Cargo.toml`**

In the `[dependencies]` block (keep alphabetical with the existing entries — `anstyle` goes first, `console` after `clap`):

```toml
anstyle = "1.0"
```

and after the `clap = { ... }` line:

```toml
console = "0.16"
```

- [ ] **Step 2: Verify they resolve to the already-locked versions (no new compiles)**

Run: `cargo build --offline 2>&1 | grep -iE "compiling (anstyle|console)" || echo "no fresh compile — reused from lock"`
Expected: `no fresh compile — reused from lock` (versions 1.0.14 / 0.16.3 are already in `Cargo.lock`).

- [ ] **Step 3: Add the `ColorChoice` ValueEnum + `--color` flag in `src/cli.rs`**

Add near the `Preset` enum (after line ~38, before `Cli`):

```rust
/// When to emit ANSI color in the run summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    /// Color when stdout is a TTY, `NO_COLOR` is unset, and `TERM != dumb`.
    Auto,
    /// Always emit color, even when piped (useful for `| less -R`).
    Always,
    /// Never emit color.
    Never,
}
```

Add the field to `RunArgs` (after `rebuild_cache`, before the closing brace at line ~115):

```rust
    /// When to colorize the run summary. `auto` (default) enables color
    /// only on an interactive terminal with `NO_COLOR` unset.
    #[arg(long, value_enum, value_name = "WHEN", default_value = "auto")]
    pub color: ColorChoice,
```

- [ ] **Step 4: Verify it compiles and the flag appears in help**

Run: `cargo build && cargo run -- run --help | grep -A2 -- "--color"`
Expected: build succeeds; help shows `--color <WHEN>` with `[possible values: auto, always, never]`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/cli.rs
git commit -m "feat(cli): add --color flag; promote anstyle/console to direct deps"
```

---

## Task 2: `term_style` — Caps, ColorChoice bridge, palette, glyphs

**Files:**

- Create: `src/term_style.rs`
- Modify: `src/lib.rs` (add `pub mod term_style;` with the other module declarations)

- [ ] **Step 1: Write the failing test**

Create `src/term_style.rs` with only the test module first:

```rust
//! Terminal styling vocabulary: capability detection, a semantic color
//! palette, glyph sets with ASCII fallback, and pure box/bar renderers.
//! Every renderer takes `Caps` explicitly so behavior is testable without
//! a real terminal. The single styling site for `summary` and `labels` —
//! they share this module so their output cannot drift apart.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_flip_with_unicode() {
        let uni = Caps { color: false, unicode: true, width: 80 };
        let asc = Caps { color: false, unicode: false, width: 80 };
        assert_eq!(uni.bucket_glyph(Bucket::Keep), "●");
        assert_eq!(asc.bucket_glyph(Bucket::Keep), "o");
        assert_eq!(uni.bucket_glyph(Bucket::Unfollow), "○");
        assert_eq!(asc.bucket_glyph(Bucket::Unfollow), ".");
    }

    #[test]
    fn paint_is_noop_when_color_off() {
        let caps = Caps { color: false, unicode: true, width: 80 };
        let out = caps.paint("hello", caps.keep_style());
        assert_eq!(out, "hello");
        assert!(!out.contains('\u{1b}'), "no ESC bytes when color off");
    }

    #[test]
    fn paint_wraps_when_color_on() {
        let caps = Caps { color: true, unicode: true, width: 80 };
        let out = caps.paint("hi", caps.keep_style());
        assert!(out.contains('\u{1b}'), "ESC present when color on");
        assert!(out.ends_with("hi\u{1b}[0m") || out.contains("hi"));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Add `pub mod term_style;` to `src/lib.rs` (alongside the other `pub mod` lines near the top), then run:
Run: `cargo test --lib term_style 2>&1 | head -20`
Expected: FAIL — `Caps`, `Bucket`, etc. not found / does not compile.

- [ ] **Step 3: Implement the types + palette + glyphs**

Prepend above the test module in `src/term_style.rs`:

```rust
use crate::cli::ColorChoice;
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib term_style 2>&1 | tail -10`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/term_style.rs src/lib.rs
git commit -m "feat(term_style): Caps, semantic palette, glyph fallback, paint chokepoint"
```

---

## Task 3: `term_style` — pure renderers (`bar`, `boxed`, `hrule`)

**Files:**

- Modify: `src/term_style.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/term_style.rs`:

```rust
    #[test]
    fn bar_proportions() {
        let caps = Caps { color: false, unicode: true, width: 80 };
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
        let caps = Caps { color: false, unicode: false, width: 80 };
        let b = caps.bar(40, 40, 6);
        assert!(b.starts_with('#'), "ascii fill uses '#': {b:?}");
        assert!(!b.contains('█'));
    }

    #[test]
    fn boxed_frames_and_pads() {
        let caps = Caps { color: false, unicode: true, width: 80 };
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
        let caps = Caps { color: false, unicode: true, width: 80 };
        let rows = caps.boxed("T", &["x".repeat(50)], 12);
        assert!(rows[1].contains('…'), "overlong body line truncates: {:?}", rows[1]);
        assert!(rows[1].chars().count() <= 12);
    }

    #[test]
    fn boxed_ascii_fallback() {
        let caps = Caps { color: false, unicode: false, width: 80 };
        let rows = caps.boxed("T", &["a".to_string()], 10);
        assert!(rows[0].starts_with('+') && rows[0].contains('-'));
        assert!(rows[2].starts_with('+'));
        assert!(rows[1].starts_with('|'));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib term_style 2>&1 | head -20`
Expected: FAIL — `bar` / `boxed` not found.

- [ ] **Step 3: Implement the renderers**

Add to `impl Caps` in `src/term_style.rs`:

```rust
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
                // ▏▎▍▌▋▊▉ are U+258F..U+2589 (1/8 .. 7/8).
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
        let inner = width.saturating_sub(2); // room for the two side borders
        let mut rows = Vec::with_capacity(lines.len() + 2);

        // Top border with embedded title: `╭─ Title ─…─╮`
        let title_seg = format!("{h} {title} ");
        let title_len = title_seg.chars().count();
        let fill = inner.saturating_sub(title_len);
        rows.push(format!("{tl}{title_seg}{}{tr}", h.to_string().repeat(fill)));

        for line in lines {
            let body = truncate_with_ellipsis(line, inner.saturating_sub(2));
            let pad = inner.saturating_sub(body.chars().count() + 1);
            rows.push(format!("{v} {body}{}{v}", " ".repeat(pad)));
        }

        rows.push(format!("{bl}{}{br}", h.to_string().repeat(inner)));
        rows
    }
```

Add a free helper at module scope (below the `impl`):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib term_style 2>&1 | tail -12`
Expected: PASS (all term_style tests, ~8).

- [ ] **Step 5: Commit**

```bash
git add src/term_style.rs
git commit -m "feat(term_style): proportional bar + boxed card renderers with ASCII fallback"
```

---

## Task 4: `term_style` — `Caps::detect` from environment + `--color`

**Files:**

- Modify: `src/term_style.rs`

- [ ] **Step 1: Write the failing tests** (deterministic branches only — TTY detection itself is not unit-tested; it is covered by the Task 9 pipe test)

Add to the `tests` module:

```rust
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
        assert!((40..=100).contains(&caps.width), "width clamped: {}", caps.width);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib term_style::tests::detect 2>&1 | head -15`
Expected: FAIL — `detect` not found.

- [ ] **Step 3: Implement `detect`**

Add to `impl Caps` in `src/term_style.rs`:

```rust
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

        Self { color, unicode, width }
    }
```

Add the locale helper at module scope:

```rust
/// True when the active locale env vars advertise UTF-8.
fn locale_is_utf8() -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"].iter().any(|k| {
        std::env::var(k)
            .map(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("utf-8") || v.contains("utf8")
            })
            .unwrap_or(false)
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib term_style 2>&1 | tail -12`
Expected: PASS (all term_style tests, ~11).

- [ ] **Step 5: Commit**

```bash
git add src/term_style.rs
git commit -m "feat(term_style): Caps::detect — TTY/NO_COLOR/locale/width resolution"
```

---

## Task 5: `summary` module — RunMeta + dashboard render

**Files:**

- Create: `src/summary.rs`
- Modify: `src/lib.rs` (add `pub mod summary;`)

- [ ] **Step 1a: Add a shared `#[cfg(test)]` `AccountFeatures` builder**

`AccountFeatures` has **no** `Default` derive — it is a ~30-field struct that
existing tests build with a full field literal (see `labels.rs:402` and
`scoring.rs:369 baseline_account`). To avoid a third copy of that literal, add
one crate-test-only builder in `src/features/aggregate.rs` and reuse it.

In `src/features/aggregate.rs`, inside (or adjacent to) the existing
`#[cfg(test)] mod tests`, add a `pub(crate)` builder. **Copy the
`AccountFeatures { … }` field block verbatim from `src/labels.rs:402` (the
canonical all-fields builder) so every field is present** — do not hand-type
the field list from memory, it drifts:

```rust
#[cfg(test)]
pub(crate) fn fake_features(username: &str) -> AccountFeatures {
    AccountFeatures {
        username: username.to_owned(),
        // … paste the remaining fields exactly as in labels.rs:402-… …
        // (display_name: None, account_class: AccountClass::Personal, … through
        //  the last field). All zero/false/None defaults; only `username` varies.
    }
}
```

Verify it compiles: `cargo build --lib 2>&1 | grep -E "error" | head` → no errors.

- [ ] **Step 1b: Write the failing tests**

Create `src/summary.rs`:

```rust
//! Assembles the `run` dashboard from scored accounts: header banner,
//! bucket panel, keep_prob histogram, side-by-side keep/unfollow cards.
//! Layout only — all styling primitives live in `crate::term_style`, the
//! shared vocabulary with `crate::labels`. Pure given an explicit `Caps`.

use crate::scoring::{Bucket, ScoredAccount};
use crate::term_style::Caps;

/// One-line run context shown in the header banner.
pub struct RunMeta<'a> {
    pub total: usize,
    pub config_label: &'a str,
    pub date: jiff::civil::Date,
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
        RunMeta { total: 3, config_label: "balanced preset", date: jiff::civil::date(2026, 5, 31) }
    }

    #[test]
    fn renders_counts_and_titles_no_color() {
        let caps = Caps { color: false, unicode: true, width: 80 };
        let out = render_to_string(&sample(), &meta(), &caps);
        assert!(out.contains("balanced preset"));
        assert!(out.contains("keep"));
        assert!(out.contains("Top keeps"));
        assert!(out.contains("Unfollow candidates"));
        assert!(out.contains("alice"));
        assert!(out.contains("carol"));
    }

    #[test]
    fn no_color_render_is_esc_free() {
        let caps = Caps { color: false, unicode: true, width: 80 };
        let out = render_to_string(&sample(), &meta(), &caps);
        assert!(!out.contains('\u{1b}'), "no ESC bytes when color off");
    }

    #[test]
    fn narrow_width_stacks_cards() {
        let caps = Caps { color: false, unicode: true, width: 60 };
        let out = render_to_string(&sample(), &meta(), &caps);
        // Both card titles still present, each on its own row region
        // (stacked, not side-by-side). A side-by-side render would place
        // both titles on the same line.
        let same_line = out
            .lines()
            .any(|l| l.contains("Top keeps") && l.contains("Unfollow candidates"));
        assert!(!same_line, "cards must stack at width 60");
    }
}
```

> **Note on the test helper:** `render_to_string` is a test-only sibling of the public `render` that returns the assembled string instead of printing it. This keeps `render` itself a thin `println!` wrapper while the layout stays unit-testable. Define both in Step 3.

- [ ] **Step 2: Run to verify failure**

Add `pub mod summary;` to `src/lib.rs`, then run:
Run: `cargo test --lib summary 2>&1 | head -20`
Expected: FAIL — `render_to_string` / `render` not found. (The `fake_scored` helper compiles because it delegates to `fake_features` from Step 1a; if it doesn't, the pasted field block in Step 1a is incomplete — diff it against `src/labels.rs:402`.)

- [ ] **Step 3: Implement `render` + `render_to_string`**

Add to `src/summary.rs` (above the test module):

```rust
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
    let header = format!(
        "{} followings · {} · {}",
        meta.total, meta.config_label, meta.date
    );
    for row in caps.boxed("igsift", &[header], w.min(64)) {
        let _ = std::fmt::Write::write_fmt(&mut o, format_args!("{}\n", caps.paint(&row, caps.dim_style())));
    }
    o.push('\n');

    // --- Bucket panel ---
    let (keep, review, unfollow) = bucket_counts(scored);
    let max = keep.max(review).max(unfollow).max(1);
    o.push_str("  Buckets\n");
    for (bucket, label, count) in [
        (Bucket::Keep, "keep", keep),
        (Bucket::Review, "review", review),
        (Bucket::Unfollow, "unfollow", unfollow),
    ] {
        let glyph = caps.paint(caps.bucket_glyph(bucket), caps.bucket_style(bucket));
        let pct = 100.0 * f64::from(count) / (meta.total.max(1) as f64);
        let bar = caps.bar(count, max, 26);
        let _ = std::fmt::Write::write_fmt(
            &mut o,
            format_args!("  {glyph} {label:<8} {count:>4}  {bar}  {pct:>4.1}%\n"),
        );
    }
    o.push('\n');

    // --- Histogram ---
    o.push_str(&histogram(scored, caps));
    o.push('\n');

    // --- Cards ---
    let mut by_prob: Vec<&ScoredAccount> = scored.iter().collect();
    by_prob.sort_by(|a, b| b.keep_prob.partial_cmp(&a.keep_prob).unwrap_or(std::cmp::Ordering::Equal));
    let top: Vec<String> = by_prob.iter().take(10).map(|s| card_row(s)).collect();
    let bottom: Vec<String> = by_prob.iter().rev().take(10).map(|s| card_row(s)).collect();

    let card_w = if w >= 72 { (w - 1) / 2 } else { w.min(40) };
    let left = caps.boxed("Top keeps", &top, card_w);
    let right = caps.boxed("Unfollow candidates", &bottom, card_w);

    if w >= 72 {
        // Side by side: zip rows, pad the left card to a fixed column.
        let left_cols = left.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let rows = left.len().max(right.len());
        for i in 0..rows {
            let l = left.get(i).cloned().unwrap_or_default();
            let r = right.get(i).cloned().unwrap_or_default();
            let lpad = left_cols.saturating_sub(l.chars().count());
            let _ = std::fmt::Write::write_fmt(
                &mut o,
                format_args!("{l}{} {r}\n", " ".repeat(lpad)),
            );
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
    format!("{:<20} {:.3}  {}", s.features.username, s.keep_prob, s.dominant_feature)
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

    let mut o = String::from("  keep_prob distribution\n");
    for (i, &c) in counts.iter().enumerate().skip(first) {
        let lo = i as f64 / 10.0;
        let bar = caps.bar(c, max, 28);
        let _ = std::fmt::Write::write_fmt(&mut o, format_args!("  {lo:.1}  {bar} {c:>4}\n"));
    }
    o
}
```

> The `histogram` and `card_row` helpers read `s.keep_prob`, `s.features.username`, and `s.dominant_feature` (a `&'static str` — interpolates fine). No `AccountFeatures` field is touched beyond `username`, so the Step 1a builder fully covers the test surface.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib summary 2>&1 | tail -12`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/summary.rs src/lib.rs
git commit -m "feat(summary): dashboard render — banner, bucket panel, histogram, cards"
```

---

## Task 6: Make `labels::report` caps-aware

**Files:**

- Modify: `src/labels.rs:249` (the `report` fn signature + body)

- [ ] **Step 1: Update the existing labels test expectations**

Check whether any test in `src/labels.rs` asserts on `report`'s exact printed strings:
Run: `grep -nE "fn report|report\(|confusion matrix|agreement:" src/labels.rs`
Expected: `compute()` has tests on the `ReportData` values; `report()` itself is print-only. If a test calls `report(...)` with two args, update the call to three (add `&caps`). If a test asserts on the old print format, retarget it at `compute(...)` return values instead.

- [ ] **Step 2: Change `report` to take `&Caps` and render via term_style**

Edit the signature at `src/labels.rs:249`:

```rust
pub fn report(labels: &LabelSet, scored: &[ScoredAccount], caps: &crate::term_style::Caps) {
```

Replace the agreement/hard-mismatch print block so the headline uses color and the framed style matches the dashboard. Keep the confusion-matrix numbers and the `[label=keep ∩ …]` legend identical (they are the documented accuracy contract). Concretely, wrap the agreement line and the mismatch verdict:

```rust
    let pct_line = format!(
        "agreement: {}/{} ({:.1}%)",
        data.agreed, data.scored_total, data.agreement_pct,
    );
    println!("{}  [label=keep ∩ bucket=keep + label=drop ∩ bucket=unfollow]",
        caps.paint(&pct_line, caps.bold_style()));
```

and the verdict (replacing the existing `if data.hard_mismatches.is_empty()` head):

```rust
    if data.hard_mismatches.is_empty() {
        println!("{}", caps.paint("✓ no hard mismatches", caps.bucket_style(Bucket::Keep)));
    } else {
        let head = format!("✗ {} hard mismatch(es)", data.hard_mismatches.len());
        println!("{}", caps.paint(&head, caps.bucket_style(Bucket::Unfollow)));
        // ... keep the existing per-mismatch detail lines unchanged ...
    }
```

> When `unicode` is off, `✓`/`✗` should fall back to `OK`/`X`. Add a tiny inline branch using `caps.unicode` for those two glyphs, or extend `term_style` with `caps.check_glyph()` / `caps.cross_glyph()` if you prefer the vocabulary in one place (recommended — keeps glyph choices out of `labels`).

- [ ] **Step 3: Verify the lib compiles (callers updated in Task 7)**

Run: `cargo build --lib 2>&1 | grep -E "error|report" | head`
Expected: only an arity error at the `labels::report(...)` call site in `lib.rs` (fixed next task). No errors inside `labels.rs` itself.

- [ ] **Step 4: Run the labels unit tests**

Run: `cargo test --lib labels 2>&1 | tail -10`
Expected: PASS — `compute()` tests unchanged; any `report` call updated to pass `&caps`.

- [ ] **Step 5: Commit**

```bash
git add src/labels.rs
git commit -m "refactor(labels): render accuracy block via term_style (caps-aware)"
```

---

## Task 7: Wire `summary::render` into `lib::run`

**Files:**

- Modify: `src/lib.rs:599-652` (replace inline bucket/top/bottom prints + histogram call; update `labels::report` call)
- Modify: `src/lib.rs:697-719` (delete the relocated `print_keep_prob_histogram`)

- [ ] **Step 1: Replace the inline summary block**

In `run()`, replace lines 599–645 (from `let keep_count = …` through the end of the `bottom 10` loop) and the `print_keep_prob_histogram(&scored);` call with:

```rust
    let caps = crate::term_style::Caps::detect(args.color.into());
    let config_label = config_label(&args);
    let meta = summary::RunMeta {
        total: scored.len(),
        config_label: &config_label,
        date: jiff::Zoned::now().date(),
    };
    summary::render(&scored, &meta, &caps);
```

- [ ] **Step 2: Update the `labels::report` call**

At `src/lib.rs:650`, change:

```rust
        Some(label_set) => labels::report(&label_set, &scored, &caps),
```

(the `None` arm's "accuracy report skipped" `println!` stays, optionally wrapped in `caps.paint(..., caps.dim_style())`).

- [ ] **Step 3: Add the `config_label` helper + delete the old histogram fn**

Add near `resolve_output_stem`:

```rust
/// Human label for the resolved scoring source, shown in the summary
/// header. Mirrors the resolution precedence in `config::read_scoring_config`
/// (preset flag → explicit --config path → default chain).
fn config_label(args: &RunArgs) -> String {
    if let Some(p) = args.preset {
        format!("{} preset", p.name())
    } else if let Some(c) = &args.config {
        c.display().to_string()
    } else {
        "default config".to_string()
    }
}
```

Delete `fn print_keep_prob_histogram` (lines ~697–719) — it now lives in `summary::histogram`.

- [ ] **Step 4: Add the `ColorChoice → ColorChoice` bridge (cli → term_style)**

`args.color.into()` needs `From<cli::ColorChoice>` for `term_style`'s expected type. Since `term_style::Caps::detect` takes `cli::ColorChoice` directly (Task 4 imports it), `args.color` is already the right type — replace `args.color.into()` with `args.color`. Verify the import in `term_style.rs` is `use crate::cli::ColorChoice;` and `detect(args.color)` compiles.

- [ ] **Step 5: Build + run against the fixture**

Run: `cargo build && cargo run -- run tests/fixtures/sample_export --out /tmp/smoke 2>&1 | tail -30`
Expected: the new dashboard renders (boxed banner, bucket panel, histogram, two cards, accuracy block, `wrote:` lines). No panics.

- [ ] **Step 6: Run the full unit suite**

Run: `cargo nextest run --lib 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/lib.rs
git commit -m "feat(run): render the dashboard summary; relocate histogram to summary"
```

---

## Task 8: Integration tests — pipe-safety + `--color`

**Files:**

- Modify: `tests/cli.rs`

- [ ] **Step 1: Write the failing tests**

Add to `tests/cli.rs` (use the existing `igsift()` helper + `out_stem`):

```rust
#[test]
fn summary_is_esc_free_when_piped() {
    // assert_cmd runs with stdout NOT a TTY → Auto must resolve to no color.
    let output = igsift()
        .arg(sample_export())
        .arg("--out")
        .arg(out_stem("pipe_safe"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains('\u{1b}'),
        "piped summary must contain no ANSI ESC bytes:\n{stdout}"
    );
}

#[test]
fn color_never_is_esc_free() {
    let output = igsift()
        .arg(sample_export())
        .arg("--out")
        .arg(out_stem("color_never"))
        .arg("--color")
        .arg("never")
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(!String::from_utf8_lossy(&output.stdout).contains('\u{1b}'));
}

#[test]
fn color_always_emits_esc() {
    let output = igsift()
        .arg(sample_export())
        .arg("--out")
        .arg(out_stem("color_always"))
        .arg("--color")
        .arg("always")
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(
        String::from_utf8_lossy(&output.stdout).contains('\u{1b}'),
        "--color always must force ANSI ESC bytes even when piped"
    );
}
```

- [ ] **Step 2: Run to verify they pass (implementation already exists)**

Run: `cargo nextest run --test cli summary_is_esc_free color_never color_always 2>&1 | tail -15`
Expected: PASS (3 tests). If `summary_is_esc_free` fails, `Caps::detect(Auto)` is emitting color off a non-TTY — re-check the `is_terminal()` guard in Task 4.

- [ ] **Step 3: Confirm the locked-in assertions still hold**

Run: `cargo nextest run --test cli 2>&1 | tail -20`
Expected: PASS — including the `-v` count family, `--trace`, and `wrote:` assertions.

- [ ] **Step 4: Commit**

```bash
git add tests/cli.rs
git commit -m "test(cli): pipe-safety + --color never/always contract"
```

---

## Task 9: Final gate — fmt, clippy, full suite, manual look

**Files:** none (verification only)

- [ ] **Step 1: Format**

Run: `cargo fmt --all`
Then: `git diff --stat` — review any reformatting, stage if clean.

- [ ] **Step 2: Clippy (warnings are errors, mirrors CI)**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings. Likely nits to pre-empt: prefer `writeln!`/`write!` over the verbose `std::fmt::Write::write_fmt` calls if clippy flags them; collapse any `format!` + `push_str` clippy suggests.

- [ ] **Step 3: Full test suite**

Run: `cargo nextest run 2>&1 | tail -15`
Expected: all green.

- [ ] **Step 4: Supply-chain gate (new direct deps were already in-tree, but confirm)**

Run: `cargo deny check advisories bans sources 2>&1 | tail -15`
Expected: pass — `anstyle`/`console` were already transitive, so no new advisories/bans/sources.

- [ ] **Step 5: Manual look in a real terminal (color + unicode path)**

Run: `cargo run -- run tests/fixtures/sample_export --out /tmp/look`
Eyeball: boxed banner, three colored bucket rows with proportional bars, histogram starting at the first non-empty bucket, two side-by-side cards, green `✓ no hard mismatches`. Then ASCII/no-color path:
Run: `NO_COLOR=1 cargo run -- run tests/fixtures/sample_export --out /tmp/look2 | cat -v | head -30`
Eyeball: no `^[` escapes, `+--+` frames if your locale is non-UTF-8 (or force by unsetting `LANG`).

- [ ] **Step 6: Final commit (only if fmt produced changes)**

```bash
git add -A
git commit -m "style: cargo fmt after dashboard summary"
```

---

## Self-Review Notes

- **Spec coverage:** module layout (Task 2/5/6), `Caps`+detection (Task 2/4), palette+glyph fallback (Task 2), `bar`/`boxed` polish (Task 3), `--color` flag (Task 1), pipe-safety + ASCII fallback (Task 3/4/8), labels caps-aware render (Task 6), `lib` wiring + histogram relocation (Task 7), preserved `-v`/`--trace`/`wrote:`/`check` (Task 8 Step 3), dep promotion with no new crate (Task 1 + Task 9 Step 4). All spec sections map to a task.
- **Type consistency:** `Caps { color, unicode, width }`, `ColorChoice { Auto, Always, Never }`, `bar(value,max,width)`, `boxed(title,lines,width)`, `RunMeta { total, config_label, date }`, `summary::render`/`render_to_string`, `labels::report(labels, scored, caps)` — names are consistent across Tasks 2–8.
- **Verified struct shapes (de-risked):** `ScoredAccount` = `{ features, score_raw: f64, keep_prob: f64, bucket: Bucket, dominant_feature: &'static str, top_terms: [(&'static str, f64); 3] }` (`src/scoring.rs:75`). `AccountFeatures` has **no** `Default` derive, so Task 5 Step 1a adds a shared `#[cfg(test)] fake_features` builder (copied from the canonical `labels.rs:402` literal) rather than `AccountFeatures::default()`. `Bucket = { Keep, Review, Unfollow }`. The test helper is aligned to these.
