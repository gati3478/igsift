# Terminal run-summary dashboard: a polished default stdout

**Status:** approved design, pre-implementation
**Date:** 2026-05-31
**Author:** Gati (design via brainstorming session)

## Problem

The `run` summary is correct but visually flat. Today it is a stack of
unstyled `println!` lines emitted directly from `lib.rs::run` and
`labels.rs::report`:

```text
bucket keep: 572
bucket review: 40
bucket unfollow: 37
keep_prob histogram:
  [0.0, 0.1):    0
  ...
top 10 keep candidates:
  ryrydelrey  keep_prob=1.000  bucket=keep  dominant=dm
  ...
confusion matrix:
                 bucket=keep  bucket=review  bucket=unfollow
  label=keep              40             12                0
  ...
```

The information is all there, but it reads like a debug dump: no hierarchy,
no color, fixed `[lo, hi)` notation that buries the one bucket that matters
(`[0.9, 1.0]` holds 513 of 649 accounts), and `top 10` / `bottom 10` stacked
vertically so the eye can't compare keeps against unfollows. For a one-shot
CLI whose entire output surface is this summary plus three written files, the
summary is the product's face. It should look considered.

This is a **presentation-only** change. No scoring, parsing, feature, or
output-writer behavior changes. The CSV/Markdown/HTML artifacts are untouched.

## Goals

- Replace the flat stdout dump with a **polished dashboard**: clear hierarchy,
  restrained color, proportional bars, and side-by-side comparison of the
  top keeps against the unfollow candidates.
- **Polish is the bar, not feature count.** The win is in the details —
  aligned columns, consistent padding, a color palette that means something
  (green = keep, yellow = review, red = unfollow), glyphs that degrade
  gracefully, bars that are proportional and never overflow the frame. A
  reader should feel the output was designed, not printed.
- **Never break a pipe.** Redirected or piped output is plain text with zero
  ANSI escape bytes, so `igsift run … | tee`, `> log.txt`, and CI logs stay
  clean and grep-able.
- **Degrade gracefully** on terminals without UTF-8 or color (notably the
  Windows x64/arm64 CI smoke legs) — ASCII fallbacks, no mojibake boxes.
- Keep the change **bounded and testable**: small focused modules, pure
  render functions, no new color crate.

## Non-goals

- No change to CSV / Markdown / HTML writers (`src/output/*`).
- No theme configuration in TOML, no user-selectable palettes (YAGNI).
- No new color/styling dependency — reuse what is already in the tree.
- No change to `--trace` output. It is a weight-tuning instrument, not a
  display surface, and stays exactly as it is.
- No change to the `-v` / `-vv` verbose per-source counts (the
  `following count: 4` family). They print above the summary as today.

## Background: what the redesign must not disturb

Audited against `tests/cli.rs`. The only load-bearing stdout assertions are:

| Assertion                                                              | Where              | Status under this change                                             |
| ---------------------------------------------------------------------- | ------------------ | -------------------------------------------------------------------- |
| `following count: 4` and the per-source count family                   | behind `-v`        | **Untouched** — verbose counts are a separate block, not the summary |
| `trace for "alice_synth"`, `close_friend_boost`                        | `--trace` output   | **Untouched** — trace is left as-is                                  |
| `wrote: …` paths                                                       | run + init         | **Preserved** — the closing confirmation lines stay                  |
| `check` output (`Validating export:`, `All sources parsed cleanly`, …) | `check` subcommand | **Untouched** — `check` is a different surface                       |

The default-level run summary (buckets / histogram / top-bottom / confusion
matrix) carries **no** `contains(...)` assertion today, so it is free to be
rewritten. New tests are added to lock in the rewritten contract (below).

## Design

### Module layout

Two new small modules plus light edits — keeping rendering out of the
already-large `lib.rs`, and following the existing pattern of focused
single-purpose files (`progress.rs`, `text.rs`).

```
src/
  term_style.rs   NEW  — capability detection + theme + reusable renderers
  summary.rs      NEW  — assembles the run-summary dashboard from scored data
  labels.rs       EDIT — render the accuracy block via term_style primitives
  lib.rs          EDIT — run() calls summary::render(...) instead of inline println!
  cli.rs          EDIT — add `--color auto|always|never` (default auto)
```

`term_style` is the **shared vocabulary** so the two render sites
(`summary`, `labels`) cannot drift apart visually — the same box frames, the
same palette, the same bar glyph everywhere.

### `term_style.rs` — capabilities, theme, primitives

The seam that makes everything else pure and testable. Detection happens once
at the edge; every renderer takes `Caps` explicitly so tests pin behavior
without touching a real terminal.

```rust
/// Rendering capabilities, resolved once from the environment + --color flag.
pub struct Caps {
    pub color: bool,    // ANSI styling on?
    pub unicode: bool,  // box-drawing + dot glyphs, or ASCII fallback?
    pub width: usize,   // usable columns (already clamped, see below)
}

pub enum ColorChoice { Auto, Always, Never }   // from --color

impl Caps {
    /// Detect from stdout. `color`: Always → on; Never → off;
    /// Auto → stdout-is-TTY AND NO_COLOR unset AND TERM != "dumb".
    /// `unicode`: terminal advertises UTF-8 (via `console`), else false.
    /// `width`: detected terminal width, clamped to [40, 100]; falls back
    ///          to 80 when undetectable (piped/non-TTY).
    pub fn detect(choice: ColorChoice) -> Self;
}
```

Color is applied with `anstyle` `Style` values (already a transitive dep via
clap). TTY / `NO_COLOR` / width detection use `console` (already a transitive
dep via indicatif). Both are **promoted to direct dependencies** in
`Cargo.toml` — no new crates enter the tree, satisfying the project's
dep-conscious posture (`owo-colors` was deliberately removed; `anstyle` is
already paid for by clap).

**Palette** (semantic, not decorative):

| Role                      | Color  | Glyph (unicode / ascii) |
| ------------------------- | ------ | ----------------------- | --- |
| keep                      | green  | `●` / `o`               |
| review                    | yellow | `◐` / `*`               |
| unfollow                  | red    | `○` / `.`               |
| chrome (frames, rules)    | dim    | `╭─╮│╰╯` / `+-          | `   |
| emphasis (counts, titles) | bold   | —                       |

When `Caps.color == false`, every style is the identity (no escape bytes).
When `Caps.unicode == false`, glyphs and box characters use the ASCII column.

**Pure renderers** (the polish lives here):

- `bar(value: u32, max: u32, width: usize) -> String` — proportional fill
  using `█` (unicode) or `#` (ascii), with eighth-block sub-cell precision
  (`▏▎▍▌▋▊▉`) under unicode so small buckets still register. Never exceeds
  `width`; `max == 0` yields an empty bar (no divide-by-zero).
- `boxed(title: &str, lines: &[String], width: usize, caps: &Caps) -> Vec<String>`
  — frame a card with a titled top border, padded body lines, bottom border.
  Lines longer than the inner width are truncated with `…`. Returns rows so
  callers can place two cards side by side by zipping their row vectors.
- `hrule(width, caps)` and `kv(label, value, width)` helpers for aligned
  label/value rows.

All renderers are width-correct under both glyph sets — the eighth-block and
truncation logic is where the "designed, not printed" feel comes from, so it
gets dedicated tests.

### `summary.rs` — the dashboard

One entry point called from `run` after scoring:

```rust
pub struct RunMeta<'a> {
    pub total: usize,           // followings scored
    pub preset_or_config: &'a str,  // "balanced preset" | "config/scoring.toml" | "<path>"
    pub date: jiff::civil::Date,
}

pub fn render(scored: &[ScoredAccount], meta: &RunMeta, caps: &Caps);
```

Layout, top to bottom (the approved Dashboard silhouette):

1. **Header banner** — boxed: `649 followings · balanced preset · 2026-05-31`.
2. **Buckets panel** — one row per bucket: colored glyph, label, right-aligned
   count, proportional `bar()`, and `%` of total. Bars share one scale so the
   three rows are visually comparable.
3. **keep_prob histogram** — compact. Bars proportional to the fullest bucket.
   The dominant `[0.9, 1.0]` bucket leads; the smaller buckets follow in a
   tighter grouping. Empty leading buckets (`0.0`, `0.1` here) are omitted so
   the chart starts where the data does.
4. **Side-by-side cards** — `Top keeps` (left) and `Unfollow candidates`
   (right), each a `boxed()` card of up to 10 rows `handle · keep_prob ·
dominant`. Built by zipping the two row vectors; **stacks vertically** when
   `caps.width < 72`.
5. **Accuracy block** (only when `config/labels.txt` is present) — rendered by
   `labels.rs` via `term_style`: `46/58 (79.3%)`, a `✓ no hard mismatches`
   (green) or `✗ N hard mismatches` (red) line, and the two `label=keep` /
   `label=drop` rows. When labels are absent, the existing one-line
   "accuracy report skipped" note, restyled dim.
6. **Closing** — the unchanged `wrote: …` lines for the three artifacts.

`print_keep_prob_histogram` moves out of `lib.rs` into `summary.rs`.

### `labels.rs` edit

`labels.rs` keeps **computing** the confusion matrix, agreement, and hard
mismatches (its job as the accuracy oracle is unchanged). Only its rendering
is rerouted through `term_style` primitives so the accuracy block matches the
frame and palette of the panels above it. The computation and the
confusion-matrix unit tests in `labels.rs` are preserved; if any test asserts
on the exact old print string, it is updated to assert on the computed report
values instead (data, not formatting).

### `cli.rs` edit

Add `--color <auto|always|never>` (clap `ValueEnum`, default `auto`) to
`RunArgs`. `auto` is the detection path above; `always` forces color even when
piped (for `| less -R`); `never` forces plain. Maps to `ColorChoice`.

### `lib.rs::run` edit

The inline bucket/top/bottom `println!` blocks and the
`print_keep_prob_histogram` call collapse into:

```rust
let caps = term_style::Caps::detect(args.color.into());
let meta = summary::RunMeta { total: scored.len(), preset_or_config, date };
summary::render(&scored, &meta, &caps);
// labels report + wrote: lines as today, the report now caps-aware
```

## Error handling

Rendering is infallible — it writes to stdout and cannot fail the run. Width
detection failure falls back to 80 columns. There is no new fallible path; the
function signatures return `()`, not `Result`. Scoring/parse/IO errors upstream
are unaffected.

## Testing

Integration-first, matching project convention.

**`term_style.rs` unit tests** (`#[cfg(test)] mod tests`):

- `bar()` proportions: full, empty (`max == 0`), partial with eighth-block
  precision, and never exceeds `width`.
- `boxed()`: inner width and padding correct; over-long line truncates with
  `…`; row count = body + 2 borders.
- Glyph selection flips with `Caps.unicode` (dot/box characters).
- **`Caps { color: false, .. }` produces output with zero `\x1b` bytes** —
  the core no-color invariant, asserted directly.

**`summary.rs` unit tests:**

- Render a small fixed `scored` slice with `Caps { color:false, unicode:true,
width:80 }`; assert the bucket counts, header, and both card titles appear.
- Assert the no-color render is ESC-free.
- Narrow-width (`width:60`) render: assert cards stack (one card title per
  line region) rather than overflowing.

**`tests/cli.rs` additions:**

- **Pipe-safety:** run the binary against the fixture with stdout captured
  (assert_cmd is non-TTY by default) and assert the output contains no `0x1b`
  byte. Locks in the pipe-safe contract end to end.
- **`--color never`** explicitly: also ESC-free.
- Optionally **`--color always`**: output _does_ contain an ESC byte, pinning
  the override.

**Preserved, must stay green:** the `-v` per-source count assertions, the
`--trace` assertions, the `wrote:` lines, and the full `check` surface. Final
gate: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
`cargo nextest run` all clean.

## Rollout

Single change, no migration. The redesigned summary ships as the new default;
existing scripts that parse stdout were already steered to the CSV (the
machine-readable contract), and the `wrote:` lines they rely on are preserved.
`NO_COLOR` and `--color never` give anyone the old plain-text behavior.
