# Review sub-grouping — inert vs faded — design

**Date:** 2026-06-01
**Status:** implemented
**Related:** [`2026-06-01-inert-account-floor-design.md`](2026-06-01-inert-account-floor-design.md),
[`2026-06-01-dead-mutual-review-gate-design.md`](2026-06-01-dead-mutual-review-gate-design.md)

## Problem

The inert-account Unfollow floor (merged 2026-06-01) grew the owner's Review
bucket to ~116 accounts (522/116/11 keep/review/unfollow). That pile is now a
**wall**: it mixes two populations the owner reads at very different speeds.

- **Inert** — zero behavioural signal in any direction. Most were floored
  here from Unfollow precisely _because_ they carry no evidence to drop; their
  `keep_prob` is pure tenure. The owner skims these in bulk ("do I still want
  to follow this account I've never interacted with?") and acts fast.
- **Faded** — had real signal once (a DM thread, likes, a reaction, an inbound
  request, or a keep-side demotion like dead-mutual / non-reciprocal
  close-tie), now decayed below the keep cut. These are the **genuinely
  ambiguous** calls that deserve the owner's attention.

Rendered as one undifferentiated list, the ~26+ inert accounts drown the faded
ones. The fix is a presentation-layer split so the inert pile reads fast and in
bulk without burying the judgment calls.

## Non-goals

- **Not** a scoring or bucket-assignment change. No new gate, no weight, no
  threshold. Every account stays in exactly the bucket `assign_bucket`
  produced. This is pure presentation.
- **Not** a new predicate. The split reuses `scoring::is_inert` — the existing
  SSOT — verbatim. No second definition of "inert" is introduced.
- **Not** a CSV contract change. The CSV header is the inter-run diff contract;
  the split is a help-me-skim-the-report feature and the reports are
  Markdown/HTML. See "CSV — no change" below.

## Design

One principle: **within Review, separate "never engaged" from "once engaged,
now cold" so the owner spends attention on the ambiguous calls.**

### Predicate — reuse `is_inert`, gated on Review

`scoring::is_inert` is already the SSOT for this split (its doc comment names
this follow-up explicitly) and is deliberately class-agnostic. Two changes:

1. Change `scoring::is_inert` visibility from private → **`pub(crate)`**. No
   logic change.
2. Add a thin shared helper in `src/output/mod.rs`, beside `decision_hint` and
   `contributions_inline` (the existing shared-writer SSOT site):

    ```rust
    /// `true` when a Review account carries no behavioural signal in any
    /// direction — the "inert" half of the Review inert/faded split. Reuses
    /// scoring::is_inert verbatim; the bucket gate keeps the predicate honest
    /// (an inert account in Keep/Unfollow is not "Review-inert"). SSOT for both
    /// writers' sub-grouping. "Faded" is the complement: Review && !this.
    pub(super) fn is_review_inert(f: &AccountFeatures, bucket: Bucket) -> bool {
        bucket == Bucket::Review && crate::scoring::is_inert(f)
    }
    ```

    Both writers call this helper, so the Review-gate lives in exactly one
    place. **Faded** is never named as its own predicate — it is the complement
    (`bucket == Review && !is_review_inert`), so the two halves can't drift apart
    or double-count by construction.

**Why a `pub(crate)` fn, not a bool on `ScoredAccount`.** Inertness is fully
derivable from `AccountFeatures`; carrying a denormalized `is_inert` field on
`ScoredAccount` would add state that can fall out of sync with the predicate.
The shared-fn approach matches how `decision_hint` is already surfaced to both
writers.

**Why literal `is_inert`, no carve-out for keeplisted/restricted.** A
keeplisted- or restricted-floored account with _zero_ lifetime behavioural
signal is rare (you do not keeplist an account you have never liked, DM'd, or
even viewed). Carving such accounts out of "inert" would buy almost nothing on
real data, and it would break the subsection titles: the complement could no
longer honestly be called "**once engaged**" if it contained never-engaged
flagged accounts. The clean behavioural dichotomy keeps the titles accurate.
Either way the account is still listed in the report — only its placement
(compact table vs. card) differs.

### Markdown — two subsections

Split `write_review_section` into two labelled subsections, **only when at
least one inert account exists** (mirroring the existing droplist-quarantine
convention in `write_unfollow_section`; an inert-free Review stays a flat list,
unchanged from today):

- `### Faded — once engaged, now cold (N)` — the judgment calls. **Keeps the
  current treatment verbatim**: sorted by decision difficulty
  (`|keep_prob − 0.5|` ascending), top `REVIEW_CARDS` (30) full-rationale cards,
  remainder as a one-line table. The card population shrinks naturally to the
  faded subset.
- `### Inert — never engaged (N)` — the skim pile. **Compact one-line table
  only**, no cards. Sorted by `keep_prob` ascending (most-droppable first —
  these are Unfollow-adjacent). Reuses the existing `write_table` renderer.

Section ordering: Faded first (attention), Inert second (skim). The
`## Review (N)` total and its intro line are unchanged; the two subheads carry
the per-half counts. When the split fires but one half is empty (e.g. an
all-inert Review), that subhead still renders with its `(0)` count and a
`_None._` line — mirroring the empty-`### Scored low (0)` block in
`write_unfollow_section` — so the partition is always legible.

### HTML — filter toggle

The Review section is already a searchable/sortable table with per-row data
attributes. Add:

1. A `data-inert="1"|"0"` attribute on each Review `<tr>` (set from
   `is_review_inert`). Non-Review rows are unaffected.
2. A checkbox in the Review section's existing controls: **"Hide never-engaged
   (N)"**, **unchecked by default** — show-all so no account silently
   disappears; one click collapses the inert pile. Wired into the existing
   client-side filter predicate (which already AND-composes search + bucket),
   so search/sort continue to work over whichever rows are visible.
3. A muted `never engaged` pill on inert rows for at-a-glance distinction,
   styled with the existing tag/pill CSS.

The toggle and pill render only in the Review section. Keep/Unfollow are
untouched.

### CSV — no change

The CSV header (`username,…,bucket,keep_score,…,top_signal,reply_skew,
dm_inbound_replies`) is the inter-run diff contract. The inert/faded split is a
report-skim affordance, not a new datum — analogous to `pct()` being a
human-report-only rendering while the CSV keeps the raw `keep_prob` float. An
inert account is already identifiable in the CSV (`bucket=review`,
`top_signal=tenure`, low `keep_score`). If spreadsheet-level filtering is wanted
later, a `review_class` column can be added as a deliberate, documented contract
bump — out of scope here.

### `decision_hint` — no change

Inert accounts already fall through `decision_hint`'s precedence chain to honest
descriptors ("tenure-only — no engagement signal" for the mutual fallback,
"one-sided …" for non-mutual). The grouping conveys inertness structurally; a
new hint row would be redundant with the subsection header (Markdown) and the
pill (HTML).

## Testing

Output-layer tests only; no scoring tests change.

- **`output/mod.rs`** — `is_review_inert`: a unit test pinning that it is
  `true` only for a zero-signal account **and** `bucket == Review`, and `false`
  for the same features in Keep/Unfollow, and `false` for a Review account with
  any single signal (mirror the existing `is_inert_each_signal_breaks_it`
  shape at the gate level).
- **`markdown.rs`** —
    - With ≥1 inert + ≥1 faded Review account: both `### Faded — once engaged`
      and `### Inert — never engaged` subheads render, with correct counts, and
      Faded precedes Inert.
    - Inert-free Review renders the **flat** list (no subheads) — pins the
      droplist-style early return.
    - An inert account renders in the Inert table, a faded account renders as a
      Faded card (or in the Faded tail table) — pins the partition direction
      (catches a filter inversion).
- **`html.rs`** —
    - Inert Review row carries `data-inert="1"`; faded Review row carries
      `data-inert="0"`.
    - The "Hide never-engaged" control renders in the Review section only, with
      the inert count.
    - The `never engaged` pill renders on an inert row and not on a faded row.

## Files touched

- `src/scoring.rs` — `is_inert` visibility `fn` → `pub(crate) fn` (one word).
- `src/output/mod.rs` — add `is_review_inert` helper + its unit test.
- `src/output/markdown.rs` — split `write_review_section`; add subsection tests.
- `src/output/html.rs` — `data-inert` attribute, filter checkbox + JS predicate,
  pill + tests.

## Out of scope / deferred

- CSV `review_class` column (contract bump) — see above.
- Measuring the inert/faded breakdown on the real export as a TUNING note —
  optional follow-up; the split is presentational and does not move the bucket
  counts.
