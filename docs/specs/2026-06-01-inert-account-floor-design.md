# Inert-account Unfollow floor — design

**Date:** 2026-06-01
**Status:** approved (design)
**Related:** [`2026-06-01-dead-mutual-review-gate-design.md`](2026-06-01-dead-mutual-review-gate-design.md),
[`2026-05-30-reciprocity-aware-scoring.md`](2026-05-30-reciprocity-aware-scoring.md)

## Problem

Every account in the owner's genuine Unfollow bucket has the **same shape**:
`top_signal = tenure`, and every engagement column is zero — no DM in or
out, no reactions either way, zero likes/comments/saves/story-interactions.
They reach Unfollow not on any negative signal but because tenure is the
only non-zero term they carry, and `keep_prob` therefore sorts them by
**follow-age alone**. The scorer is ranking _silence_, then slicing it at
`unfollow_max`.

This conflates two very different things:

- **Absence of data** — an account you have never interacted with. The
  export shows nothing because there is nothing, not because the
  relationship soured.
- **Evidence to drop** — a soured or abandoned relationship.

Tenure is not a drop signal. For a zero-engagement account the honest
verdict is "look at it yourself" (Review), not "remove" (Unfollow). The
owner confirmed this directly: curated one-way follows that fell into
Unfollow — independent bookstores, art/game studios, a band, notable
creators, local venues — are non-correspondents he would not _strictly_
unfollow, only review.

The existing brand gate (`account_class == Personal` required for Unfollow)
already encodes a slice of this principle, but it is **recall-limited**: the
lexicon catches `store`/`books`/`press`/`zine` and misses `design`,
`studies`, `project`, bands, and any notable person whose handle is a
personal name. A lexicon arms-race cannot close the gap — a person's name
carries no brand token. The fix must key on _evidence_, not _account type_.

## Non-goals

- **Not** an Unfollow path. Like the other relationship gates, the class is
  defined by _absence_ of signal; Review (human attention) is the only safe
  outcome. The gate is monotonic — Unfollow → Review only — so it can never
  manufacture a drop.
- **Not** a re-weighting. Correctness must not depend on the noisy
  `labels.txt` oracle. This is a gate, not a tuned weight. No paired penalty
  term.
- **Not** a word-boundary matcher rework. The deferred 3-char tokens (`art`,
  `bar`) and the matcher change they need stay deferred — the inert floor
  catches those zero-engagement handles regardless of classification.

## Design

Two complementary changes serving one principle: **silence is absence of
evidence, not evidence to drop.**

### Change A — inert-account floor (primary)

A new config-gated, monotonic rung inside the `keep_prob < unfollow_max`
block of `scoring::assign_bucket`, beside the existing
`close_friend / favorited / keeplist / Brand` carve-outs.

**`is_inert` predicate** — no behavioural signal in any direction, on the
**lifetime raw** counters (deliberately stricter than dead-mutual's `<= 1`
tolerance; the claim is "never interacted in any way the export records"):

```rust
fn is_inert(f: &AccountFeatures) -> bool {
    !f.is_hide_story_from && !f.is_removed_suggestion
        && f.likes_given == 0 && f.comments_given == 0
        && f.story_interactions_out == 0 && f.stories_viewed == 0
        && f.saved_their_content == 0
        && f.dm_messages_total == 0
        && f.dm_reactions_given == 0 && f.dm_reactions_received == 0
        && !f.inbound_dm_request
}
```

`is_hide_story_from` / `is_removed_suggestion` are deliberate negative owner→them actions — real evidence to drop — so an account carrying either is not inert and stays a genuine Unfollow candidate.

`dm_messages_total == 0` subsumes `dm_out == 0` and `dm_inbound_replies == 0`
and forces `dm_balance == None`; combined with the reaction/request clauses
this implies `!has_inbound_signal(f)` — the inert predicate is strictly
stronger than the dead-mutual core, so the two need not share code (no
refactor of the tested `is_dead_mutual`).

**`__deleted__` carve-out** — IG redacts deleted/deactivated accounts as
handles prefixed `__deleted__` (7 in the owner's export; no igsift code
emits the string). A gone account is the one case where Unfollow is
positively correct — there is nothing to lose by removing it — so it is
exempted from the floor and stays Unfollow:

```rust
fn is_deleted(f: &AccountFeatures) -> bool {
    f.username.starts_with("__deleted__")
}
```

If IG ever changes the redaction string the account degrades to Review (the
safe direction).

**Ladder placement** (only the Unfollow block changes):

```
keep_prob < unfollow_max:
    close_friend | favorited | keeplisted | Brand          → Review   (existing)
    floor_inert_to_review & is_inert & !is_deleted          → Review   (NEW)
    else                                                    → Unfollow
```

Monotonic: turns Unfollow → Review only. Sits structurally below the
droplist — a droplisted handle returns Unfollow at the top of
`assign_bucket` and never reaches this block — so explicit drops are
untouched.

**Config:** `floor_inert_to_review: bool`, defaulted `true` in all three
presets, `config/scoring.toml`, and the `serde` default (same class as the
dead-mutual gate: high-precision, Review-only, monotonic). `false` disables.

### Change B — brand-lexicon recall (secondary)

Add `design`, `studies`, `project` to `BRAND_LEXICON` (each ≥ 4 chars).
Every token must pass the module's existing discipline — a
**0-false-positive grep against the real export's followee list** — before
it ships; a token that false-positives on any real personal handle is
dropped from this change.

> **As built:** on verification only `project` passed the 0-FP grep. `design`
> matched a personal design-creator the owner engages with (a real FP) and
> `studies` matched an ambiguous `name.studies` personal portfolio, so both
> were dropped — see TUNING round 12. The two paragraphs below are the
> pre-implementation rationale and still hold for the shipped `project` token.

Honest scope: with Change A in place the lexicon's _bucketing_ value is
small — every zero-engagement `*_design` / `*studies` / `*project` handle is
already floored by the inert gate. The payoff is **classification accuracy**
(`amanita_design_` reads `Brand` not `Personal` in the CSV `account_class`
column) plus protecting a brand that carries _some_ engagement (a few likes,
so not inert) from ever reaching Unfollow.

## Expected effect

On the owner's export (currently 522 keep / 90 review / 37 unfollow):

| bucket   | before | after | note                                 |
| -------- | -----: | ----: | ------------------------------------ |
| keep     |    522 |   522 | unchanged                            |
| review   |     90 |  ~116 | +~26 silent accounts                 |
| unfollow |     37 |   ~11 | droplist forces + `__deleted__` only |

Unfollow becomes high-precision: when it says drop, it is either an explicit
droplist entry or a provably-gone account.

## Testing

- Unit tests on the gate rung, mirroring the dead-mutual block: inert →
  Review; a single like spares the floor (not inert → stays Unfollow);
  `__deleted__` stays Unfollow; droplist still forces Unfollow; `is_mutual`
  irrelevant to the predicate; `floor_inert_to_review = false` restores
  Unfollow.
- `is_inert` predicate tests over the field matrix (each engagement field
  independently breaks inertness).
- Lexicon tests for the three new tokens; a 0-FP assertion note documenting
  the export verification.
- **Label-regression check** against `config/labels.txt`: confirm zero
  keep-labeled accounts move toward Unfollow (structurally impossible under
  the monotonic gate — measured anyway). Record the bucket split and
  agreement delta in `docs/TUNING.md` (round 12).
- Fixture-count assertions in `tests/cli.rs` adjusted if the synthetic
  fixture shifts buckets.

## Privacy

The spec and the TUNING round-12 entry use structural descriptors (`a
zero-engagement personal account at keep_prob=0.32`), never the owner's raw
followee handles — per the project's privacy convention. Brand-business
public handles (e.g. a `*_design` studio) remain quotable.

## Deferred follow-up (out of scope)

The inert floor grows Review to ~116. If that becomes a pile the owner will
not scan, the fix is an **output-layer** sub-grouping of Review — "inert —
never engaged" (reuse `is_inert`) vs "faded — had engagement, now cold" — so
the inert accounts skim in bulk without drowning the genuinely-ambiguous
faded ones. Pure presentation, no bucket/score change; its own spec → plan
cycle in a later session.
