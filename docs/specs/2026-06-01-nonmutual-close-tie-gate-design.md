# Non-reciprocal close-tie penalty + gate — design

**Status:** approved design, pre-implementation.
**Date:** 2026-06-01.
**Related:** [`2026-05-30-reciprocity-aware-scoring.md`](2026-05-30-reciprocity-aware-scoring.md)
(the reciprocity ceiling this inverts), [`2026-05-31-effort-skew-gate-design.md`](2026-05-31-effort-skew-gate-design.md)
(the monotonic keep-ceiling pattern this follows), [`docs/DESIGN.md`](../DESIGN.md)
("Scoring composition", "Bucket precedence"), [`docs/TUNING.md`](../TUNING.md)
(penalty-magnitude calibration lands here).

## Motivation

Surfaced by a real-export account (the **worked example**; handle withheld per
the Privacy convention — a personal followee paired with explicit drop intent is
the same disclosure as the gitignored `config/labels.txt`). Its row shape:

```
bucket = keep, keep_score = 1.000
account_class = personal, mutual = FALSE   (they do not follow the owner back)
is_close_friend = true                     (the owner's explicit marker)
dm_msgs = 10, reply_skew = 0.700 (7 owner / 3 them), likes_given_90d = 3
follow_tenure_days ≈ 1.5y, reactions_received_180d = 0
```

The owner marked this account **close friend** and engages with it, yet it has
**never followed the owner back**. That asymmetry — _you applied your strongest
"this person matters" flag and they didn't even reciprocate the follow_ — is a
red flag, and the current model has **no catch for it**:

1. `is_close_friend` adds **+5.0** to `score_raw`, pinning `keep_prob` to
   ~1.000. Nothing in the engagement terms outweighs a +5 boost.
2. The one gate that could demote a non-mutual personal account — the
   reciprocity keep-ceiling (`is_parasocial`) — **explicitly exempts close
   friends and favorites** (`!is_favorited && !is_close_friend`), and is off for
   this owner anyway.

So the exact shape "explicit close marker **+** they don't follow back" is
treated as pure positive evidence. This change makes that shape **suspect**: it
is the mirror-inverse of the existing parasocial exemption. Where
`is_parasocial` says _close-friend / favorited makes you safe_, this says
_close-friend / favorited **combined with non-mutual** makes you a red flag_.

The owner's goal, stated directly: such accounts "should go to **at least
review**" and "should in fact be **heavily penalized**." Hence a hybrid — a
score-eroding penalty for honesty in the report, plus a gate that guarantees the
Review floor.

## The shape — one shared predicate (SSOT)

A single function in `src/scoring.rs`, consumed by the penalty term and the gate
rung (and matched, by shape, by the decision hint). The exact inverse of
`is_parasocial`:

```rust
fn is_nonreciprocal_close_tie(f: &AccountFeatures) -> bool {
    f.account_class == AccountClass::Personal   // brands legitimately don't follow back
        && !f.is_mutual                         // they didn't reciprocate the follow
        && (f.is_close_friend || f.is_favorited) // an explicit "matters to me" marker
        && !f.is_keeplisted                     // explicit keep intent opts out, like is_parasocial
}
```

- **Personal-only** is load-bearing: a brand / public figure you favorited that
  doesn't follow back is normal, not a red flag. `account_class == Brand` is
  excluded.
- **Explicit markers only** (`is_close_friend || is_favorited`) — _not_ raw
  engagement. Keying on engagement alone is exactly what turning on
  `require_reciprocity_for_keep` did, and the 2026-05-30 labeling pass measured
  that as harmful (it demoted 20/20 deliberately-curated one-way creator/brand
  follows, agreement 69.0% → 34.5%). The explicit marker is what distinguishes
  the worked example from a legit one-way creator follow: you don't mark a
  creator you follow one-way as a close friend.
- **`!is_keeplisted`** is baked into the predicate (mirrors `is_parasocial`), so
  _both_ the penalty and the gate respect a keeplist override automatically —
  keeplisting one of these accounts opts it fully back out.

Droplist is **not** in the predicate: a droplisted account is forced to Unfollow
at rung 2, above this whole feature, so the predicate firing for it just adds an
inert penalty term to a score that no longer drives the bucket. Harmless, and
keeping it out of the predicate keeps the predicate about _shape_, not overrides.

## Two knobs, each following an existing pattern

The codebase already splits its tunables two ways; this change follows that split
rather than inventing a third shape:

- **Penalties are weight-controlled and always-on** (like `hide_story_penalty` —
  weight `0.0` disables, no toggle).
- **Gates are toggle-controlled** (like `require_reciprocity_for_keep` /
  `effort_skew_*`).

| Knob                          | Type   | Section     | `serde` default           | Presets                    | `config/scoring.toml` |
| ----------------------------- | ------ | ----------- | ------------------------- | -------------------------- | --------------------- |
| `nonmutual_close_tie_penalty` | `f64`  | `[weights]` | **none — required field** | tuned value                | tuned value           |
| `demote_nonmutual_close_ties` | `bool` | `[scoring]` | **`true`**                | `true` (omitted ⇒ default) | `true`                |

**On by default for everyone** (owner's call): unlike effort-skew/reciprocity
(which default off because they were measured potentially harmful on curated
one-way follows), this gate is high-precision — it requires the explicit marker
**and** non-mutual **and** personal — and it only ever demotes to Review (human
triage), never Unfollow. So it ships live in all three presets and as the
`serde` default, and a binary-only install gets it.

Consequence of the weight being a **required** `[weights]` field (the established
convention — every weight appears in every config): all three presets,
`config/scoring.toml`, and the `VALID_BODY` test fixture must carry it, and the
`validate()` finiteness loop must list it. An external config missing the key
fails to parse loudly — identical to the existing 16 weights.

## Mechanism 1 — Penalty (score honesty)

A **17th term** in `scoring::term_contributions` (`NUM_TERMS` 16 → 17),
surfacing **signed-negative** so it can rank as the dominant contribution:

```rust
(
    "nonmutual_close_tie_penalty",
    if is_nonreciprocal_close_tie(f) { -w.nonmutual_close_tie_penalty } else { 0.0 },
)
```

Because the magnitude (~6) exceeds `close_friend_boost` (+5) and the engagement
terms, it becomes the **dominant term** for the worked example →
`dominant_feature` / `top_signal` flips from `likes` to
`nonmutual_close_tie_penalty`. That is how the
red flag surfaces in the **CSV** without a new column — the existing `top_signal`
column and `mutual = false` together tell the story, and `--trace` shows the full
breakdown. The DESIGN.md CSV header contract is unchanged.

## Mechanism 2 — Gate (guaranteed Review floor)

A new monotonic rung in `scoring::assign_bucket`, inside the
`keep_prob >= keep_min` block, alongside the reciprocity ceiling:

```rust
if p.demote_nonmutual_close_ties && is_nonreciprocal_close_tie(f) {
    return Bucket::Review;
}
```

**Never reaches Unfollow on its own.** If the penalty drags `keep_prob` below
`unfollow_max`, the account falls through to rung 6, where the **existing**
`is_close_friend || is_favorited` gate already demotes Unfollow → Review. So the
floor is Review across the entire score range; the droplist stays the only path
to a forced Unfollow. (The worked example is on the droplist, so it lands in
Unfollow regardless — this feature is about catching the _class_ structurally,
for the accounts the owner hasn't hand-flagged.) Consistent with the gates-not-weights,
keep-gates-never-manufacture-Unfollow rule.

## Precedence — one rung inserted

```
1. is_restricted                                         → Review    (unchanged, top floor)
2. is_droplisted                                         → Unfollow  (unchanged)
3. effort-skew HARD                                      → Review    (unchanged)
4. deep-mutual floor                                     → Keep      (requires mutual — cannot
                                                                       co-occur with this gate)
5. keep_prob ≥ keep_min:
     effort-skew SOFT                                    → Review     (unchanged)
     demote_nonmutual_close_ties && is_nonreciprocal_close_tie → Review   ← NEW
     reciprocity ceiling (if enabled)                   → Review      (unchanged)
     else                                               → Keep
6. keep_prob < unfollow_max (+ cf/fav/keeplist/brand gate) → Unfollow / Review   (unchanged)
7. otherwise                                            → Review
```

Order among the three rung-5 keep-ceilings is immaterial — all return Review.
Rung 4 (deep-mutual) requires `is_mutual`, which the predicate negates, so the
two are mutually exclusive by construction.

## Decision hint (Markdown + HTML)

A new arm in `src/output/mod.rs::decision_hint`, inserted **after the keeplist
check, before the generic `is_close_friend` / `is_favorited` arms**, so a
non-reciprocal close tie is described as the red flag rather than a bland "marked
close friend":

```rust
// (is_keeplisted already returned above; so it is false here)
if !f.is_mutual
    && (f.is_close_friend || f.is_favorited)
    && matches!(f.account_class, AccountClass::Personal)
{
    return "close tie not reciprocated — they don't follow you back";
}
```

Fires on **shape, regardless of the toggle** — same convention as the
effort-skew and long-standing-mutual hints (a true characterization of the
account whether or not the gate is enabled or retuned). The 18-row precedence
test gains a new row (non-reciprocal-close-tie beats "marked close friend") plus
a guard (a **mutual** close friend still reports "marked close friend").

## Config

```toml
# [weights]
nonmutual_close_tie_penalty = <tuned>   # subtracted when personal + !mutual + (close_friend|favorited)

# [scoring]
demote_nonmutual_close_ties = true      # floor such accounts at Review; serde default true
```

The exact penalty magnitude is **the one open number** and is a TUNING-round
decision, not guessed here (see below). The gate guarantees the bucket; the
penalty only controls how low `keep_score` reads in the report.

## Calibration (TUNING round, during implementation)

The gate's correctness does not depend on the noisy `labels.txt` oracle, but the
**penalty magnitude** and the **collateral footprint** do need a measured pass
on the real export:

1. Pick a starting magnitude (~6.0, just above `close_friend_boost`).
2. Run the scorer on the real export; confirm the worked example reads visibly
   low and `top_signal = nonmutual_close_tie_penalty`.
3. **Count how many accounts the gate moves Keep → Review** and eyeball them —
   the predicate is narrow, but the footprint must be inspected, not assumed.
4. Check **zero hard mismatches** against `labels.txt` (a labeled-keep account
   demoted to Review is the failure mode to rule out; if one appears, it is a
   keeplist candidate, not a reason to widen the predicate).
5. Lock the magnitude in `config/scoring.toml` + all presets; document the round
   (footprint, mismatches, chosen magnitude) in `docs/TUNING.md` — the SSOT for
   tuning numbers.

## Touch-points

- `src/config.rs` — `nonmutual_close_tie_penalty` on `WeightsConfig`;
  `demote_nonmutual_close_ties` on `ScoringParams` with
  `default_demote_nonmutual_close_ties() -> true`; add the weight to the
  `validate()` finiteness loop; extend `VALID_BODY`.
- `src/scoring.rs` — `is_nonreciprocal_close_tie` predicate; the 17th term in
  `term_contributions`; `NUM_TERMS` 16 → 17; the gate rung in `assign_bucket`;
  the `baseline_cfg()` / config builders in tests gain the new fields; new tests
  (below).
- `src/output/mod.rs` — the new `decision_hint` arm + precedence-test row + guard.
- `config/scoring.toml` + `config/presets/{balanced,engagement,tenure}.toml` —
  the new weight (tuned) and (presets) the toggle defaulting on.
- Any other `WeightsConfig { .. }` / `ScoringParams { .. }` literal — grep so the
  new required field compiles everywhere (scoring tests, `examples/showcase.rs`,
  output tests).
- Docs: `docs/DESIGN.md` ("Scoring composition" formula, "Bucket precedence"),
  `CLAUDE.md` (Conventions — the relationship-gates paragraph), `docs/TUNING.md`
  (the calibration round).

## Testing (TDD)

- **Predicate (`scoring.rs` unit):** fires on personal + !mutual + close_friend;
  fires on personal + !mutual + favorited; does **not** fire when mutual; does
  **not** fire when Brand; does **not** fire when keeplisted; does **not** fire
  on a personal !mutual account with engagement but no marker.
- **Penalty (`scoring.rs` unit):** a close-friend non-mutual personal account
  scores exactly `close_friend_boost − nonmutual_close_tie_penalty` (+ other
  terms); the term is signed-negative; `dominant_feature` surfaces it when its
  magnitude leads.
- **Gate (`scoring.rs` unit):** demotes a Keep-scoring non-reciprocal close tie
  to Review when the toggle is on; **stays Keep when the toggle is off**;
  keeplist survives the gate; a **mutual** close friend is untouched; the gate
  never yields Unfollow (a heavily-penalized one lands in Review via the rung-6
  cf/fav gate, not Unfollow); boundary — toggle on + predicate true + score into
  keep ⇒ Review.
- **Config (`config.rs` unit):** a body omitting `demote_nonmutual_close_ties`
  parses with the toggle **true** (default); the new weight is required (a body
  omitting it fails to parse); a non-finite `nonmutual_close_tie_penalty` is
  rejected by `validate()`.
- **Hint (`output/mod.rs` unit):** the precedence-chain table gains the new row
  and the mutual-close-friend guard.
- **Fixture counts (`tests/cli.rs`):** if the synthetic fixture contains a
  personal non-mutual close-friend/favorited account, a bucket count shifts —
  diagnose and re-pin to the correct post-gate value; do not relax the assertion
  (CLAUDE.md). Consider adding such an account to the fixture so the gate is
  exercised E2E.

## Out of scope

- Engagement-based (marker-less) reciprocity demotion — that is
  `require_reciprocity_for_keep`, measured harmful for this owner and left as-is.
- Letting this gate reach Unfollow (the droplist is that path; keep-gates stay
  monotonic).
- A new CSV column — the penalty surfaces through the existing `top_signal`.
- The penalty-magnitude number (a TUNING round, after implementation).
