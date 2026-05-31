# Display-Width Correctness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the terminal dashboard width-correct for arbitrary Unicode content — measure _display columns_ (CJK = 2, combining/zero-width = 0) instead of codepoint count — so a non-ASCII `--config` path in the header banner (and any wide-char content) aligns cleanly instead of producing a ragged box.

**Architecture:** Promote the already-in-tree `unicode-width` crate to a direct dependency and add a `display_width` helper in `term_style`. Route the three content-measuring sites through it: `boxed` (title + body padding), `truncate_with_ellipsis` (column budget), and `summary`'s side-by-side card alignment. `bar` is unchanged (it generates width-1 cells, never measures content). `labels.rs` is untouched (fixed numeric format specifiers). Plain-text only — `display_width` must never be handed ANSI-painted strings (all measured strings in the codebase are unpainted: `boxed` receives plain `lines`, summary measures unpainted `boxed` output).

**Tech Stack:** Rust edition 2024, `unicode-width = "0.2"` (promoted from transitive → direct; already pinned at 0.2.2 in `Cargo.lock`, so zero new crates — same pattern as `anstyle`/`console`). Tests: `cargo nextest` + the existing term_style/summary unit tests.

**Reference:** Follows the merged dashboard (`docs/specs/2026-05-31-terminal-summary-dashboard-design.md`). This plan closes the "Known limitation: CJK/wide-char display width" item from the post-merge audit (Finding 2).

---

## Background: why each site changes (or doesn't)

| Site                                 | Today                                                                                           | Change?                                                                                                                                                               |
| ------------------------------------ | ----------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `term_style::bar`                    | builds N block/space chars, each 1 display col → output is already exactly `width` display cols | **No** — generates fixed-width output, measures no content. The internal `used = s.chars().count()` at the bar's padding step is correct (all bar chars are width-1). |
| `term_style::boxed` title            | `title_seg.chars().count()` + truncate-to-`inner`                                               | **Yes** — title may carry wide chars (via a `--config` path embedded by the caller's header line)                                                                     |
| `term_style::boxed` body padding     | `body_len = body.chars().count()`; `pad = inner - body_len - 1`                                 | **Yes** — body lines are arbitrary strings                                                                                                                            |
| `term_style::truncate_with_ellipsis` | takes `max - marker_len` **chars**                                                              | **Yes** — taking N chars can exceed N display cols with wide chars                                                                                                    |
| `summary` side-by-side alignment     | `left_cols`/`lpad` via `chars().count()`                                                        | **Yes** — pad the left column by display-width difference so the right card aligns                                                                                    |
| `labels::report`                     | fixed `{:>11}` numeric specifiers on ASCII                                                      | **No** — no content-width measurement                                                                                                                                 |

ANSI safety: none of the measured strings are color-painted. `boxed` receives plain `lines` (card rows are plain `card_row(s)` output; the banner header is plain before `boxed`), and `summary` measures `boxed`'s plain output. Color is applied _after_ layout. So `display_width` only ever sees plain text — never ESC bytes (which `unicode-width` would miscount, since `[32m` is printable). Do NOT call `display_width` on a `caps.paint(...)` result.

---

## Task 1: Promote `unicode-width` + add `display_width` helper

**Files:**

- Modify: `Cargo.toml` (`[dependencies]`)
- Modify: `src/term_style.rs` (add helpers + tests)

- [ ] **Step 1: Add the direct dep to `Cargo.toml`**

In `[dependencies]`, alphabetically (after `toml`, before `zip` — adjust to actual ordering):

```toml
unicode-width = "0.2"
```

- [ ] **Step 2: Verify no new crate enters the tree**

Run: `cargo build --offline 2>&1 | grep -iE "compiling unicode-width" || echo "reused from lock (no new crate)"`
Expected: it may compile `unicode-width` once as it becomes a direct artifact, but `git diff Cargo.lock` must show **no new `[[package]]` entry** — only `igsift`'s own dependency list gaining `unicode-width` (it is already pinned at 0.2.2). Confirm with: `git diff Cargo.lock | grep -E "^\+name = " || echo "no new packages"` → `no new packages`.

- [ ] **Step 3: Write the failing test** in the `tests` module of `src/term_style.rs`:

```rust
    #[test]
    fn display_width_counts_columns_not_codepoints() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("王小明"), 6); // 3 CJK chars × 2 cols
        assert_eq!(display_width("café"), 4); // precomposed é = 1 col
        // combining acute (e + U+0301) is 1 column, 2 codepoints
        assert_eq!(display_width("e\u{0301}"), 1);
        assert_eq!(display_width(""), 0);
    }
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo test --lib term_style::tests::display_width 2>&1 | head` → FAIL (`display_width` not found).

- [ ] **Step 5: Implement the helpers**

Add the import at the top of `src/term_style.rs` (with the other `use` lines):

```rust
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
```

Add at module scope (near `truncate_with_ellipsis`):

```rust
/// Display width of `s` in terminal columns: CJK/wide chars count as 2,
/// combining/zero-width as 0. Plain text ONLY — never pass an ANSI-painted
/// string (escape sequences would be miscounted). All layout measurement in
/// this module and in `summary` goes through this, so a non-ASCII handle or
/// `--config` path can't skew a box or a column.
pub(crate) fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Display width of a single char (0 for control/combining).
fn char_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}
```

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo test --lib term_style 2>&1 | tail -8` → all pass. `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` → clean. (If `char_width` is unused until Task 3, that's a dead-code warning under `-D warnings` — add `#[allow(dead_code)]` ONLY if you split tasks; preferably implement Task 3's use in the same branch so it's live. If committing Task 1 alone, gate `char_width` introduction to Task 3 instead and add only `display_width` here.)

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/term_style.rs
git commit -m "feat(term_style): add display_width helper; promote unicode-width to direct dep"
```

> NOTE on Step 6's dead-code caveat: to avoid the unused-`char_width` warning when committing Task 1 in isolation, introduce `char_width` in **Task 3** (where its first use lives) rather than here. This plan assumes Tasks 1–4 land before any `clippy -D warnings` gate is enforced on an intermediate commit; if you commit per-task, move the `char_width` definition to Task 3.

---

## Task 2: Make `boxed` display-width-correct

**Files:**

- Modify: `src/term_style.rs` (`boxed`)

- [ ] **Step 1: Write the failing test** in the `tests` module:

```rust
    // A helper that asserts every row of a card is exactly `cols` DISPLAY
    // columns wide (not codepoints).
    #[test]
    fn boxed_wide_chars_keep_equal_display_width() {
        let caps = Caps { color: false, unicode: true, width: 80 };
        // CJK title + CJK body line. Char counts differ from display widths.
        let rows = caps.boxed("账号", &["王小明的账号".to_string()], 20);
        let widths: Vec<usize> = rows.iter().map(|r| display_width(r)).collect();
        assert!(
            widths.iter().all(|&w| w == widths[0]),
            "all rows must share one DISPLAY width, got {widths:?}"
        );
        assert_eq!(widths[0], 20, "box should be exactly the requested 20 cols");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib term_style::tests::boxed_wide 2>&1 | head -20` → FAIL (rows have unequal display width, or != 20, because the current code pads by codepoint count).

- [ ] **Step 3: Implement** — in `boxed`, replace the three codepoint measurements with `display_width`:

The title segment width + overflow guard:

```rust
        let title_seg = format!("{h} {title} ");
        let title_seg = if display_width(&title_seg) > inner {
            truncate_with_ellipsis(&title_seg, inner, self.unicode)
        } else {
            title_seg
        };
        let title_len = display_width(&title_seg);
        let fill = inner.saturating_sub(title_len);
```

The body line padding:

```rust
        for line in lines {
            let body = truncate_with_ellipsis(line, inner.saturating_sub(2), self.unicode);
            let body_len = display_width(&body);
            let pad = inner.saturating_sub(body_len + 1);
            rows.push(format!("{v} {body}{}{v}", " ".repeat(pad)));
        }
```

(The border rows `{tl}{title_seg}{fill×h}{tr}` and `{bl}{inner×h}{br}` are built from `h`/corner chars which are all 1 display col, so their `h.to_string().repeat(...)` arithmetic stays in terms of `inner`/`fill` columns — correct as-is. The `bar`-internal `chars().count()` is unrelated and stays.)

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --lib term_style 2>&1 | tail -12` → all pass, including the existing `boxed_frames_and_pads`, `boxed_truncates_overlong_line`, `boxed_long_title_stays_within_width`, `boxed_ascii_*` (those use ASCII where `display_width == chars().count()`, so they still hold) and the new `boxed_wide_chars_keep_equal_display_width`.

> The existing `boxed_*` tests assert `rows.iter().all(|r| r.chars().count() == w)`. For ASCII content `chars().count() == display_width`, so they pass unchanged. Leave them as-is (they pin the ASCII contract); the new test pins the wide-char contract via `display_width`.

- [ ] **Step 5: Commit**

```bash
git add src/term_style.rs
git commit -m "fix(term_style): boxed pads by display width, not codepoint count"
```

---

## Task 3: Make `truncate_with_ellipsis` display-width-aware

**Files:**

- Modify: `src/term_style.rs` (`truncate_with_ellipsis`; add `char_width` here if deferred from Task 1)

- [ ] **Step 1: Write the failing test** in the `tests` module:

```rust
    #[test]
    fn truncate_respects_display_columns_for_wide_chars() {
        // 5 CJK chars = 10 display cols. Truncate to 6 cols (unicode marker
        // `…` = 1 col → budget 5 cols → 2 CJK chars = 4 cols, then `…`).
        let s = "王小明的号";
        let out = truncate_with_ellipsis(s, 6, true);
        assert!(out.ends_with('…'), "keeps the ellipsis: {out:?}");
        assert!(
            display_width(&out) <= 6,
            "must not exceed 6 display cols, got {} ({out:?})",
            display_width(&out)
        );
        // Short-enough strings are returned unchanged.
        assert_eq!(truncate_with_ellipsis("王", 6, true), "王");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib term_style::tests::truncate_respects 2>&1 | head -20` → FAIL (current code takes `max - marker_len` _chars_ = 5 chars = 10 cols, exceeding 6).

- [ ] **Step 3: Implement** — rewrite `truncate_with_ellipsis` to budget by display columns (add `char_width` from Task 1 here if you deferred it):

```rust
/// Truncate `s` to at most `max` DISPLAY columns, appending an ellipsis
/// marker when cut (`…` under unicode, `...` under ASCII). Wide chars that
/// would overflow the budget are dropped whole (never split), so the result
/// is always `<= max` columns.
fn truncate_with_ellipsis(s: &str, max: usize, unicode: bool) -> String {
    if display_width(s) <= max {
        return s.to_owned();
    }
    let marker = if unicode { "…" } else { "..." };
    let marker_w = display_width(marker); // 1 or 3
    if max <= marker_w {
        // No room for content; emit as much of the marker as fits by columns.
        let mut w = 0;
        let mut out = String::new();
        for ch in marker.chars() {
            let cw = char_width(ch);
            if w + cw > max {
                break;
            }
            w += cw;
            out.push(ch);
        }
        return out;
    }
    let budget = max - marker_w;
    let mut w = 0;
    let mut kept = String::new();
    for ch in s.chars() {
        let cw = char_width(ch);
        if w + cw > budget {
            break;
        }
        w += cw;
        kept.push(ch);
    }
    format!("{kept}{marker}")
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --lib term_style 2>&1 | tail -14` → all pass, including the existing ASCII truncation tests (`boxed_truncates_overlong_line` asserts `…` present and `chars().count() <= 12`; for ASCII content the display-width budget yields the same cut, so it holds) and the new wide-char test. `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` → clean (`char_width` now used).

> If `boxed_truncates_overlong_line` (ASCII `"x".repeat(50)` at width 12) changes cut position: it shouldn't — ASCII chars are 1 col, so `display_width == chars().count()` and the budget math is identical. If it fails, recheck the marker-width arithmetic.

- [ ] **Step 5: Commit**

```bash
git add src/term_style.rs
git commit -m "fix(term_style): truncate by display columns so wide chars don't overflow"
```

---

## Task 4: Make `summary` alignment display-width-based + wide-content tests

**Files:**

- Modify: `src/summary.rs` (side-by-side alignment + the narrow-width test + a new wide-header test)

- [ ] **Step 1: Write the failing test** in `summary`'s `tests` module:

```rust
    #[test]
    fn wide_config_label_keeps_banner_box_aligned() {
        // A non-ASCII --config path lands in the header line. The banner box
        // must stay rectangular (all box rows equal DISPLAY width).
        use crate::term_style::display_width;
        let caps = Caps { color: false, unicode: true, width: 80 };
        let meta = RunMeta {
            total: 3,
            config_label: "/数据/配置.toml",
            date: jiff::civil::date(2026, 5, 31),
        };
        let out = render_to_string(&sample(), &meta, &caps);
        // The banner is the boxed region at the top: its three border/body
        // rows (lines starting with the box corners/sides) must share one
        // display width.
        let banner_rows: Vec<&str> = out
            .lines()
            .take_while(|l| !l.is_empty())
            .collect();
        let widths: Vec<usize> = banner_rows.iter().map(|r| display_width(r)).collect();
        assert!(
            widths.iter().all(|&w| w == widths[0]),
            "banner rows must share one display width with a wide config label: {widths:?}"
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib summary::tests::wide_config 2>&1 | head -20` → FAIL (the middle banner row with the CJK label is wider in display cols than the borders, because `boxed`'s caller built it but the side-by-side/summary path or pre-Task-2 boxed measured codepoints). NOTE: if Tasks 2–3 already landed, the banner box itself is now correct and this test may PASS immediately — in that case it serves as a regression lock; keep it and proceed.

- [ ] **Step 3: Implement** — update the side-by-side alignment in `render_to_string` to measure display width. Replace:

```rust
        let left_cols = left.iter().map(|r| r.chars().count()).max().unwrap_or(0);
```

with:

```rust
        let left_cols = left
            .iter()
            .map(|r| crate::term_style::display_width(r))
            .max()
            .unwrap_or(0);
```

and replace:

```rust
            let lpad = left_cols.saturating_sub(l.chars().count());
```

with:

```rust
            let lpad = left_cols.saturating_sub(crate::term_style::display_width(&l));
```

- [ ] **Step 4: Update the narrow-width test to assert display width**

In `panels_fit_within_narrow_width`, change the per-line check from `line.chars().count()` to `crate::term_style::display_width(line)` (import or fully-qualify), so it asserts the true on-screen width:

```rust
        for line in out.lines() {
            let w = crate::term_style::display_width(line);
            assert!(w <= 40, "line exceeds width 40 ({w} cols): {line:?}");
        }
```

(The sample data is ASCII so this passes identically; the change makes the test honest about what "width" means.)

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test --lib summary 2>&1 | tail -12` → all pass (existing 6 + the new `wide_config_label_keeps_banner_box_aligned`). Full lib: `cargo nextest run --lib 2>&1 | tail -6`.

- [ ] **Step 6: Commit**

```bash
git add src/summary.rs
git commit -m "fix(summary): align side-by-side cards by display width; assert true width in tests"
```

---

## Task 5: Final gate + real non-ASCII `--config` look

**Files:** none (verification only)

- [ ] **Step 1: Format + lint + full suite**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5 && cargo nextest run 2>&1 | tail -4`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 2: Supply-chain gate (confirm no new crate slipped in)**

Run: `cargo deny check advisories bans sources 2>&1 | tail -6`
Expected: pass — `unicode-width` was already transitive, so no new advisory/ban/source.

- [ ] **Step 3: Manual look — a real non-ASCII config path**

```bash
mkdir -p /tmp/igsift-wide && cp config/presets/balanced.toml "/tmp/igsift-wide/配置.toml"
cargo run --quiet -- run tests/fixtures/sample_export --out /tmp/wide_out --config "/tmp/igsift-wide/配置.toml" --color always 2>&1 | head -4
```

Eyeball: the boxed `igsift` banner top/middle/bottom borders line up flush on the right edge despite the CJK path in the header line (before this work, the middle row would jut past the borders). Then confirm the ASCII/no-color path is still clean:

```bash
LC_ALL=C cargo run --quiet -- run tests/fixtures/sample_export --out /tmp/wide_ascii --config "/tmp/igsift-wide/配置.toml" --color never 2>&1 | head -4 | cat -v
```

Eyeball: ASCII box `+--+`/`|`, and the CJK path renders as whatever the terminal shows but the box stays rectangular by column count (note: under a true non-UTF-8 terminal the CJK bytes themselves are the user's display concern; our job is that the _layout math_ is column-correct).

- [ ] **Step 4: Clean up the probe dir**

Run: `rm -rf /tmp/igsift-wide`

---

## Self-Review Notes

- **Spec coverage:** dep promotion (Task 1), `display_width`/`char_width` (Task 1/3), `boxed` title+body (Task 2), `truncate_with_ellipsis` budget (Task 3), `summary` alignment + honest width test (Task 4), gates + manual non-ASCII look (Task 5). `bar` correctly exempt; `labels.rs` correctly untouched.
- **ANSI safety:** `display_width` is only ever called on plain strings (`boxed` inputs, unpainted `boxed` output in `summary`). Documented in the helper's doc-comment and the Background table. No call site passes a `caps.paint(...)` result.
- **Backward-compat:** all existing ASCII tests hold because `display_width(ascii) == chars().count()`. New tests pin the wide-char contract.
- **Type/name consistency:** `display_width(&str) -> usize` (pub(crate)), `char_width(char) -> usize` (private), used uniformly in `term_style` and `summary`.
- **Dead-code caveat:** if committing per-task, introduce `char_width` in Task 3 (its first use) to avoid an unused-fn warning under `-D warnings` on an intermediate commit.
