# Review inert/faded sub-grouping — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the Review bucket in the Markdown and HTML reports into "Inert — never engaged" (skim in bulk) and "Faded — once engaged, now cold" (judgment calls), so the inert pile doesn't drown the ambiguous accounts.

**Architecture:** Pure output-layer change. Reuse `scoring::is_inert` (made `pub(crate)`) via a thin Review-gated helper `is_review_inert` in `output/mod.rs`. Markdown renders two subsections; HTML tags rows `data-inert` and adds a "Hide never-engaged" filter checkbox. No scoring, bucket, or CSV change.

**Tech Stack:** Rust (edition 2024), `cargo nextest`, the existing self-contained HTML report (inline CSS + vanilla JS, no deps).

**Spec:** [`docs/specs/2026-06-01-review-subgrouping-inert-faded-design.md`](../specs/2026-06-01-review-subgrouping-inert-faded-design.md)

---

## File structure

- `src/scoring.rs` — `is_inert` visibility only (`fn` → `pub(crate) fn`). No logic change.
- `src/output/mod.rs` — new `pub(super) fn is_review_inert` + its unit test. SSOT both writers call.
- `src/output/markdown.rs` — split `write_review_section` into Faded/Inert subsections; extract a `write_review_cards_and_tail` helper; update one existing test.
- `src/output/html.rs` — `data-inert` attribute + `never engaged` pill in `write_row`; "Hide never-engaged" checkbox in `write_section`; compose the JS filter; CSS for the pill + checkbox.
- `docs/DESIGN.md`, `CLAUDE.md`, spec `Status` — doc sync (final task).

**Key fixture fact (read before writing tests):** the `baseline()` / `baseline(..)` helpers in both `output/mod.rs` tests and `output/html.rs` tests produce a **zero-signal (inert)** account by default. To build a **faded** account in a test, set a non-zero behavioural signal, e.g. `likes_given = 3` **and** `likes_given_decayed = 1.0`. `is_inert` reads the lifetime raw counts (`likes_given`), so the raw field is what flips it; set the decayed field too so the card's "Why" line is realistic.

---

### Task 1: Surface `is_inert` and add the `is_review_inert` helper

**Files:**

- Modify: `src/scoring.rs` (the `fn is_inert` signature, ~line 349)
- Modify: `src/output/mod.rs` (add helper after `decision_hint`, ~line 200; add test in `mod tests`, ~line 261)

- [ ] **Step 1: Write the failing test**

In `src/output/mod.rs`, inside `mod tests` (after the `baseline()` fixture), add:

```rust
#[test]
fn is_review_inert_requires_review_bucket_and_zero_signal() {
    // baseline() is a zero-signal mutual personal account → inert.
    let f = baseline();
    assert!(
        is_review_inert(&f, Bucket::Review),
        "zero-signal account in Review must be review-inert"
    );
    // Same features, wrong bucket → not review-inert (the gate is honest).
    assert!(!is_review_inert(&f, Bucket::Keep));
    assert!(!is_review_inert(&f, Bucket::Unfollow));

    // Any single behavioural signal breaks inertness, even in Review.
    let mut engaged = baseline();
    engaged.likes_given = 1;
    assert!(
        !is_review_inert(&engaged, Bucket::Review),
        "one like is behavioural signal → faded, not inert"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p igsift is_review_inert_requires_review_bucket_and_zero_signal`
Expected: FAIL to **compile** — `cannot find function is_review_inert` and `is_inert` is private.

- [ ] **Step 3: Make `is_inert` callable from the output layer**

In `src/scoring.rs`, change the signature only (keep the doc comment and body):

```rust
pub(crate) fn is_inert(f: &AccountFeatures) -> bool {
```

- [ ] **Step 4: Add the `is_review_inert` helper**

In `src/output/mod.rs`, immediately after the `decision_hint` function (before `contributions_inline`), add:

```rust
/// `true` when a Review account carries no behavioural signal in any
/// direction — the "inert" half of the Review inert/faded split. Reuses
/// `scoring::is_inert` verbatim; the bucket gate keeps the predicate honest
/// (an inert-shaped account in Keep/Unfollow is not "Review-inert"). This is
/// the SSOT both writers call for the sub-grouping. "Faded" is never its own
/// predicate — it is the complement (`bucket == Review && !is_review_inert`),
/// so the two halves can't drift apart or double-count by construction.
pub(super) fn is_review_inert(f: &AccountFeatures, bucket: Bucket) -> bool {
    bucket == Bucket::Review && crate::scoring::is_inert(f)
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo nextest run -p igsift is_review_inert_requires_review_bucket_and_zero_signal`
Expected: PASS

- [ ] **Step 6: Verify no warnings**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean (no dead-code warning — the helper is used by the test now, and by the writers next).

- [ ] **Step 7: Commit**

```bash
git add src/scoring.rs src/output/mod.rs
git commit -m "feat(output): add is_review_inert helper; surface is_inert pub(crate)"
```

---

### Task 2: Markdown — split Review into Faded + Inert subsections

**Files:**

- Modify: `src/output/markdown.rs` (`write_review_section` ~lines 211-250; `use super::{...}` ~line 28; tests ~line 497)
- Test: `src/output/markdown.rs` `mod tests`

- [ ] **Step 1: Write the failing tests**

In `src/output/markdown.rs` `mod tests`, first add a faded-account helper after `make_scored`:

```rust
/// A Review account with real (decayed) engagement → faded, not inert.
fn make_faded(handle: &str, keep_prob: f64) -> ScoredAccount {
    let mut s = make_scored(handle, keep_prob, Bucket::Review);
    s.features.likes_given = 3;
    s.features.likes_given_decayed = 1.0;
    s
}
```

Then add the subsection tests:

```rust
#[test]
fn review_splits_into_faded_and_inert_when_inert_present() {
    // make_scored produces an inert account; make_faded adds a signal.
    let scored = vec![
        make_faded("faded_acct", 0.48),
        make_scored("inert_acct", 0.30, Bucket::Review),
    ];
    let md = render(&scored);
    assert!(
        md.contains("### Faded — once engaged, now cold (1)"),
        "faded subhead missing:\n{md}"
    );
    assert!(
        md.contains("### Inert — never engaged (1)"),
        "inert subhead missing:\n{md}"
    );
    // Faded precedes Inert.
    let pos_faded = md.find("### Faded").expect("faded subhead");
    let pos_inert = md.find("### Inert").expect("inert subhead");
    assert!(pos_faded < pos_inert, "Faded must precede Inert:\n{md}");
    // The faded account renders as a card (### card header); the inert
    // account renders in the inert one-line table (not as a card).
    let pos_faded_card = md.find("[`@faded_acct`]").expect("faded present");
    let pos_inert_row = md.find("`@inert_acct`").expect("inert present");
    assert!(
        pos_faded_card < pos_inert && pos_inert < pos_inert_row,
        "faded card above the Inert subhead, inert row below it:\n{md}"
    );
}

#[test]
fn review_stays_flat_when_no_inert() {
    // All Review accounts have signal → no Inert subhead, flat card list.
    let scored = vec![make_faded("a", 0.48), make_faded("b", 0.52)];
    let md = render(&scored);
    assert!(!md.contains("### Inert"), "no inert subhead when flat:\n{md}");
    assert!(!md.contains("### Faded"), "no faded subhead when flat:\n{md}");
    assert!(md.contains("## Review (2)"), "{md}");
}
```

Update the **existing** `review_section_sorts_by_decision_difficulty` test (currently uses `make_scored`, which is now inert and would render in the keep_prob-sorted Inert table, breaking the decision-difficulty assertion). Switch its three accounts to `make_faded` so they stay in the difficulty-sorted Faded cards:

```rust
#[test]
fn review_section_sorts_by_decision_difficulty() {
    // Three FADED Review accounts: 0.40 (|Δ|=0.10), 0.49 (|Δ|=0.01),
    // 0.65 (|Δ|=0.15). Hardest first → 0.49, 0.40, 0.65.
    let scored = vec![
        make_faded("far_high", 0.65),
        make_faded("close", 0.49),
        make_faded("far_low", 0.40),
    ];
    let md = render(&scored);
    let pos_close = md.find("`@close`").expect("close present");
    let pos_far_low = md.find("`@far_low`").expect("far_low present");
    let pos_far_high = md.find("`@far_high`").expect("far_high present");
    assert!(
        pos_close < pos_far_low && pos_far_low < pos_far_high,
        "Review must be sorted by |p - 0.5| ascending:\n{md}",
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p igsift review_splits_into_faded review_stays_flat`
Expected: FAIL — subheads not found (the writer hasn't been split yet).

- [ ] **Step 3: Add `is_review_inert` to the imports**

In `src/output/markdown.rs`, line ~28, extend the `use super::` line:

```rust
use super::{HINT_ONE_SIDED, contributions_inline, decision_hint, is_review_inert};
```

- [ ] **Step 4: Extract the cards-and-tail helper and rewrite `write_review_section`**

Replace the whole `write_review_section` function (lines ~211-250) with:

```rust
fn write_review_section(writer: &mut impl Write, scored: &[ScoredAccount]) -> Result<()> {
    let mut rows: Vec<&ScoredAccount> = scored
        .iter()
        .filter(|s| s.bucket == Bucket::Review)
        .collect();
    // Decision difficulty: closer to 0.5 = harder call. Surface the
    // hardest decisions first so the user spends their attention there.
    rows.sort_by(|a, b| {
        decision_difficulty(a.keep_prob)
            .partial_cmp(&decision_difficulty(b.keep_prob))
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });

    writeln!(writer, "## Review ({})", rows.len()).context("md")?;
    writeln!(writer).context("md")?;
    if rows.is_empty() {
        writeln!(
            writer,
            "_Sorted by decision difficulty — hardest calls first._"
        )
        .context("md")?;
        writeln!(writer).context("md")?;
        writeln!(writer, "_None._").context("md")?;
        writeln!(writer).context("md")?;
        return Ok(());
    }

    // Stable partition preserves the decision-difficulty order in `faded`.
    let (inert, faded): (Vec<&ScoredAccount>, Vec<&ScoredAccount>) = rows
        .iter()
        .copied()
        .partition(|s| is_review_inert(&s.features, s.bucket));

    // No inert accounts → flat list, exactly as before the split.
    if inert.is_empty() {
        writeln!(
            writer,
            "_Sorted by decision difficulty — hardest calls first._"
        )
        .context("md")?;
        writeln!(writer).context("md")?;
        write_review_cards_and_tail(writer, &faded)?;
        return Ok(());
    }

    // Faded subsection — the judgment calls, full card treatment.
    writeln!(writer, "### Faded — once engaged, now cold ({})", faded.len())
        .context("md")?;
    writeln!(writer).context("md")?;
    writeln!(
        writer,
        "_Sorted by decision difficulty — hardest calls first._"
    )
    .context("md")?;
    writeln!(writer).context("md")?;
    if faded.is_empty() {
        writeln!(writer, "_None._").context("md")?;
        writeln!(writer).context("md")?;
    } else {
        write_review_cards_and_tail(writer, &faded)?;
    }

    // Inert subsection — zero-signal accounts, compact table to skim in bulk.
    // Sorted keep_prob ascending (most-droppable first; Unfollow-adjacent).
    let mut inert = inert;
    inert.sort_by(|a, b| {
        a.keep_prob
            .partial_cmp(&b.keep_prob)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.features.username.cmp(&b.features.username))
    });
    writeln!(writer, "### Inert — never engaged ({})", inert.len()).context("md")?;
    writeln!(writer).context("md")?;
    writeln!(
        writer,
        "_Zero interaction in any direction — skim and bulk-act._"
    )
    .context("md")?;
    writeln!(writer).context("md")?;
    write_table(writer, inert.iter().copied())?;
    writeln!(writer).context("md")?;
    Ok(())
}

/// The Faded/flat Review rendering: top [`REVIEW_CARDS`] full cards, the
/// remainder as a one-line table. Shared by the no-inert flat path and the
/// Faded subsection so the card/tail split lives in one place.
fn write_review_cards_and_tail(
    writer: &mut impl Write,
    rows: &[&ScoredAccount],
) -> Result<()> {
    let (cards, tail) = rows.split_at(rows.len().min(REVIEW_CARDS));
    for s in cards {
        write_card(writer, s)?;
    }
    if !tail.is_empty() {
        writeln!(writer, "### Remaining {} (one-line)", tail.len()).context("md")?;
        writeln!(writer).context("md")?;
        write_table(writer, tail.iter().copied())?;
        writeln!(writer).context("md")?;
    }
    Ok(())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p igsift --lib output::markdown`
Expected: PASS — including the updated `review_section_sorts_by_decision_difficulty`.

- [ ] **Step 6: Commit**

```bash
git add src/output/markdown.rs
git commit -m "feat(markdown): split Review into Faded + Inert subsections"
```

---

### Task 3: HTML — `data-inert` attribute + `never engaged` pill on rows

**Files:**

- Modify: `src/output/html.rs` (`use super::{...}` ~line 47; `write_row` ~lines 347-423; tests ~line 1004)

- [ ] **Step 1: Write the failing tests**

In `src/output/html.rs` `mod tests`, add a faded helper after `baseline` and then the tests:

```rust
/// A Review account with real engagement → faded (not inert).
fn faded(handle: &str, keep_prob: f64) -> ScoredAccount {
    let mut s = baseline(handle, Bucket::Review, keep_prob);
    s.features.likes_given = 3;
    s.features.likes_given_decayed = 1.0;
    s
}

#[test]
fn inert_review_row_is_tagged_and_pilled() {
    let scored = vec![
        baseline("inert_acct", Bucket::Review, 0.30), // zero-signal → inert
        faded("faded_acct", 0.48),
    ];
    let html = render(&scored);
    // The inert row carries data-inert="1" and the muted pill.
    assert!(
        html.contains("data-inert=\"1\""),
        "inert row must carry data-inert=1: {html}"
    );
    assert!(
        html.contains("<span class=\"tag inert\">never engaged</span>"),
        "inert row must show the never-engaged pill: {html}"
    );
    // The faded row carries data-inert="0" and no pill.
    assert!(
        html.contains("data-inert=\"0\""),
        "faded row must carry data-inert=0: {html}"
    );
    // Exactly one pill (only the inert row).
    assert_eq!(
        html.matches("tag inert").count(),
        1,
        "exactly one never-engaged pill: {html}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p igsift inert_review_row_is_tagged_and_pilled`
Expected: FAIL — `data-inert` / pill not found.

- [ ] **Step 3: Add `is_review_inert` to the imports**

In `src/output/html.rs`, line ~47:

```rust
use super::{contributions_inline, decision_hint, is_review_inert};
```

- [ ] **Step 4: Emit `data-inert` and the pill in `write_row`**

In `write_row`, after `let p = pct(s.keep_prob);` (~line 357), add:

```rust
    let inert = is_review_inert(f, s.bucket);
```

Change the `<tr ...>` opening (the `writeln!` at ~line 362) to add the attribute:

```rust
    writeln!(
        writer,
        "<tr data-b=\"{bucket}\" data-h=\"{h}\" data-p=\"{raw}\" data-t=\"{t}\" data-m=\"{m}\" data-inert=\"{i}\">",
        bucket = s.bucket.as_str(),
        h = escape(handle),
        raw = s.keep_prob,
        t = tenure.map(|d| d.to_string()).unwrap_or_default(),
        m = u8::from(mutual),
        i = u8::from(inert),
    )
    .context("html")?;
```

Replace the hint cell `writeln!` (~line 412) with a pill-prefixed variant:

```rust
    if inert {
        writeln!(
            writer,
            "<td class=\"hint\"><span class=\"tag inert\">never engaged</span> {}</td>",
            escape(hint)
        )
        .context("html")?;
    } else {
        writeln!(writer, "<td class=\"hint\">{}</td>", escape(hint)).context("html")?;
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p igsift --lib output::html`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/output/html.rs
git commit -m "feat(html): tag inert Review rows with data-inert + never-engaged pill"
```

---

### Task 4: HTML — "Hide never-engaged" filter checkbox

**Files:**

- Modify: `src/output/html.rs` (`write_section` controls ~lines 276-290; `SCRIPT` filter block ~lines 788-802; CSS near `.tag` ~line 676 and `.controls` ~line 630; tests)

- [ ] **Step 1: Write the failing tests**

In `src/output/html.rs` `mod tests`, add:

```rust
#[test]
fn review_with_inert_renders_hide_toggle_only_there() {
    let scored = vec![
        baseline("inert_acct", Bucket::Review, 0.30),
        faded("faded_acct", 0.48),
        baseline("keep_acct", Bucket::Keep, 0.95), // zero-signal but Keep
    ];
    let html = render(&scored);
    // The checkbox renders, labelled with the inert count (1).
    assert!(
        html.contains("Hide never-engaged (1)"),
        "hide toggle with count missing: {html}"
    );
    // It appears exactly once — only the Review section, never Keep/Unfollow
    // (is_review_inert is false outside Review, so their inert count is 0).
    assert_eq!(
        html.matches("data-hide-inert").count(),
        1,
        "hide toggle must be Review-only: {html}"
    );
    // The composed filter predicate is wired in the script.
    assert!(
        SCRIPT.contains("data-hide-inert"),
        "filter JS must read the hide-inert checkbox"
    );
}

#[test]
fn review_without_inert_has_no_hide_toggle() {
    let scored = vec![faded("a", 0.48), faded("b", 0.52)];
    let html = render(&scored);
    assert!(
        !html.contains("data-hide-inert"),
        "no hide toggle when Review has no inert rows: {html}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p igsift review_with_inert_renders_hide review_without_inert_has_no_hide`
Expected: FAIL — `Hide never-engaged` / `data-hide-inert` not found.

- [ ] **Step 3: Render the checkbox in `write_section`**

In `write_section`, after the `data-shown` span `writeln!` (~line 289) and before `writeln!(writer, "</div>")` that closes `.controls` (~line 290), insert:

```rust
    // Review-only: a one-click collapse for the zero-signal pile. The
    // count auto-gates — is_review_inert is false outside Review, so
    // keep/unfollow sections compute 0 and skip the toggle.
    let inert_n = rows
        .iter()
        .filter(|s| is_review_inert(&s.features, s.bucket))
        .count();
    if inert_n > 0 {
        writeln!(
            writer,
            "<label class=\"hide-inert\"><input type=\"checkbox\" data-hide-inert> Hide never-engaged ({inert_n})</label>"
        )
        .context("html")?;
    }
```

- [ ] **Step 4: Compose the filter in `SCRIPT`**

Replace the `/* filter */` block (~lines 788-802) with a version that AND-composes the search query and the hide-inert checkbox:

```js
/* filter */
document.querySelectorAll("section").forEach(function (sec) {
    var input = sec.querySelector("input[type=search]");
    var shown = sec.querySelector("[data-shown]");
    var hideInert = sec.querySelector("input[data-hide-inert]");
    if (!input) return;
    function apply() {
        var q = input.value.toLowerCase(),
            hi = hideInert && hideInert.checked,
            n = 0;
        sec.querySelectorAll("tbody tr").forEach(function (tr) {
            var hit =
                tr.textContent.toLowerCase().indexOf(q) !== -1 &&
                !(hi && tr.dataset.inert === "1");
            tr.style.display = hit ? "" : "none";
            if (hit) n++;
        });
        if (shown) shown.textContent = n + " shown";
    }
    input.addEventListener("input", apply);
    if (hideInert) hideInert.addEventListener("change", apply);
});
```

- [ ] **Step 5: Add the CSS**

In the `<style>` block, after the `.tag` rule (the block starting at ~line 676), add:

```css
.tag.inert {
    background: var(--border-soft);
    color: var(--muted);
}
.hide-inert {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 0.8125rem;
    color: var(--muted);
    white-space: nowrap;
    cursor: pointer;
}
.hide-inert input {
    accent-color: var(--review-line);
    cursor: pointer;
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo nextest run -p igsift --lib output::html`
Expected: PASS

- [ ] **Step 7: Manual smoke check (optional but recommended)**

Run: `cargo run -- tests/fixtures/sample_export --out /tmp/igsift-smoke`
Then open `/tmp/igsift-smoke.html`, confirm the Review section shows "Hide never-engaged (N)", toggling it hides/shows the pilled rows, and the "N shown" count updates. (The fixture may have 0 inert Review rows — if so, the toggle is correctly absent; verify against the markdown `/tmp/igsift-smoke.md` Review subheads instead.)

- [ ] **Step 8: Commit**

```bash
git add src/output/html.rs
git commit -m "feat(html): add Hide never-engaged filter toggle to Review"
```

---

### Task 5: Documentation sync

**Files:**

- Modify: `docs/DESIGN.md` (Output section, after the droplist-quarantine bullet ~line 517)
- Modify: `CLAUDE.md` (Layout bullets for `markdown.rs` / `html.rs`)
- Modify: `docs/specs/2026-06-01-review-subgrouping-inert-faded-design.md` (Status line)

- [ ] **Step 1: DESIGN.md — document the split**

In `docs/DESIGN.md`, after the line ending `…doesn't read as a score anomaly.` (~line 517), add a new bullet:

```markdown
- The **Review** section splits into a **Faded — once engaged, now cold**
  subsection (full cards, hardest-call-first) and an **Inert — never engaged**
  subsection (compact table, skim in bulk), gated on `is_review_inert`
  (`output/mod.rs`, reusing `scoring::is_inert`). The split fires only when at
  least one inert account exists; an inert-free Review stays flat. The HTML
  report carries the same split as a per-row `data-inert` flag plus a
  "Hide never-engaged" filter toggle. The CSV is unchanged — an inert account
  is already `bucket=review, top_signal=tenure` with a low `keep_score`.
```

- [ ] **Step 2: CLAUDE.md — update the writer layout notes**

In `CLAUDE.md`, update the `markdown.rs` Layout line to mention the split. Find:

```
    markdown.rs                 # decision-oriented MD: keep-% cards + proportion-bar summary + droplist quarantine
```

Replace with:

```
    markdown.rs                 # decision-oriented MD: keep-% cards + proportion-bar summary + droplist quarantine + Review faded/inert split
```

And update the `html.rs` Layout line. Find:

```
    html.rs                     # self-contained HTML report (inline CSS+JS, no deps) + per-row keep/drop triage → localStorage → copy/paste to lists
```

Replace with:

```
    html.rs                     # self-contained HTML report (inline CSS+JS, no deps) + per-row keep/drop triage → localStorage → copy/paste to lists + Review never-engaged filter
```

- [ ] **Step 3: Flip the spec Status**

In `docs/specs/2026-06-01-review-subgrouping-inert-faded-design.md`, change:

```
**Status:** approved (design)
```

to:

```
**Status:** implemented
```

- [ ] **Step 4: Full verification gate**

Run each and confirm the stated expectation:

```bash
cargo fmt --all                            # Expected: no diff (or only the new code formatted)
cargo clippy --all-targets -- -D warnings  # Expected: clean
cargo nextest run                          # Expected: all tests pass (incl. tests/cli.rs fixture counts unchanged — this is presentation-only)
```

- [ ] **Step 5: Commit**

```bash
git add docs/DESIGN.md CLAUDE.md docs/specs/2026-06-01-review-subgrouping-inert-faded-design.md
git commit -m "docs: document Review faded/inert split; mark spec implemented"
```

---

## Post-implementation (not a code task)

- Update the memory note `project-igsift-review-subgrouping-followup` from DEFERRED → done (or delete it), and refresh `MEMORY.md`.
- Optional TUNING note: measure the faded/inert breakdown of the real export's 116-account Review (presentational only — bucket counts are unchanged).

---

## Self-review notes

- **Spec coverage:** Predicate surfacing (Task 1) ✓ · Markdown two subsections + empty-half + flat fallback (Task 2) ✓ · HTML data-inert + pill (Task 3) ✓ · HTML toggle, unchecked default, composed filter (Task 4) ✓ · CSV no-change (no task, by design) ✓ · decision_hint no-change (no task, by design) ✓ · docs (Task 5) ✓.
- **Type consistency:** `is_review_inert(&AccountFeatures, Bucket) -> bool` is defined in Task 1 and called identically in Tasks 2, 3, 4. `write_review_cards_and_tail(&mut impl Write, &[&ScoredAccount])` defined and used in Task 2 only. The `faded`/`make_faded` test helpers are named per-module (`make_faded` in markdown tests, `faded` in html tests) to match each module's existing helper naming (`make_scored` vs `baseline`).
- **Fixture-count tests:** `tests/cli.rs` asserts bucket counts, not section structure; this change moves nothing between buckets, so those assertions hold. Called out in Task 5 Step 4.
