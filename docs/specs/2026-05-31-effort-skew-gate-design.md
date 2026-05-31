# Effort-skew gate — design

**Status:** approved design, pre-implementation.
**Date:** 2026-05-31.
**Related:** [`2026-05-30-reciprocity-aware-scoring.md`](2026-05-30-reciprocity-aware-scoring.md)
(the reciprocity gates this generalizes), [`docs/DESIGN.md`](../DESIGN.md)
("Bucket precedence", "Partial observability"), [`docs/TUNING.md`](../TUNING.md)
(threshold calibration lands here).

## Motivation

Two findings against the 2026-05-11 export, traced through one motivating
account — a close-friend-marked, mutual personal followee (referred to below as
**the worked example**; handle withheld per the Privacy convention):

1. **`dm_balance` is silently corrupted dataset-wide.** Instagram serializes a
   message _like_ (the double-tap heart) **twice**: once in the target
   message's `reactions[]` array, and again as a standalone message with
   `content == "Liked a message"` from the reactor. The parser counts the
   shadow as a real inbound message, so every such like inflates
   `dm_messages_total` and — worse — improves `dm_balance`, the one signal
   built to detect one-sidedness. Across the export: **31,155 shadows in 394
   of 594 threads (4.4% of all DM messages); 100% co-occur with `reactions[]`**
   (never a standalone older-format record), confirming they are duplicates,
   not primary data.

    The worked example: a thread of **6 owner messages, 1 real reply, 3
    hearts**. Counting the 3 hearts as inbound messages reports
    `dm_balance = 6/4 = 0.60` (penalty −0.20); the true conversational balance is
    `6/1 = 0.857` (penalty −0.71). The likes camouflage exactly the pattern the
    balance penalty exists to catch.

2. **The model rewards the owner's own outbound effort as a keep signal.**
   The worked example scores `keep_prob = 1.000` driven by `dm` (+11.5, partly
   the phantom messages), `likes` (+5.5), `story_out` (+3.6) — all _outbound_ —
   plus a stale `close_friend_boost` (+5.0). Its only genuine inbound is one
   decayed reaction (+2.4). This violates DESIGN.md's stated principle
   _keep = relationship, not consumption_. Weight-tuning cannot fix it: at
   `score_raw ≈ 28.7` it is an order of magnitude above the keep boundary
   (`score_raw < ~2.35`). Only a **gate** can move a high scorer.

The owner's goal: surface accounts they pour effort into that barely reciprocate,
and let _egregious_ one-sidedness override a stale keep marker (e.g. a
close-friend added long ago who now ghosts), without touching deliberately
curated one-way follows or relationships that live off Instagram.

## Hard constraint — partial observability

Instagram does not export who likes/comments on the owner's posts or views/reacts
to their stories. The **only** inbound channels are DM messages, DM
`reactions[].actor`, and `message_requests/` presence. Consequently a close
friend the owner talks to on WhatsApp / sees in person shows **near-zero IG
inbound** — identical data to a ghost. The owner has confirmed **most of their
real relationships happen off Instagram.**

**Design consequence:** the skew signal is trustworthy _only inside a DM thread
the owner invested in_, where IG ships both directions. It must never demote on
"low overall inbound," because low overall inbound is IG's default for almost
everyone. Both tiers of the gate are therefore **evidence-guarded** on owner DM
volume. This is the flaw that kept the old reciprocity-ceiling gate
(`require_reciprocity_for_keep`) off — it demoted intentional one-way creator
follows; the evidence guard fixes it by acting only where reciprocity is
actually observable.

## Step 1 — De-duplicate the like shadow

`content == "Liked a message"` entries are **excluded from `dm_messages_total`
and from the `dm_balance` outbound/inbound counts**, on both directions. They
remain captured via `reactions[]` → `dm_reactions_given` / `dm_reactions_received`,
so no signal is lost — a like stops being counted as _both_ a message and a
reaction.

- Detection: a named module constant `LIKE_SHADOW_CONTENT = "Liked a message"`
  in `src/features/aggregate.rs` (or alongside the DM parse in `src/export.rs`),
  matched exactly. A single chokepoint so a future IG variant (`"Reacted …"`)
  extends in one place. Exact match, not substring — a real reply containing
  the phrase must not be dropped.
- Touch site: `walk_inbox_thread` in `src/features/aggregate.rs` — skip the
  message-volume increment, the outbound/inbound classification, and the
  recency update for a shadow; still process its (empty) `reactions[]` as today.
- Schema-drift posture (CLAUDE.md): the constant is the documented IG behavior;
  if a fresh export stops emitting shadows the counts simply fall to the
  `reactions[]`-only path with no error — degrade quietly, consistent with the
  honest-counting stance.

## Step 2 — Reply-vs-react feature

Add `dm_inbound_replies: u32` to `AccountFeatures` — the other party's **real**
messages (post-shadow-dedup). This separates "they reply" from "they tap a
heart." Reactions stay weak by design: still shown, still scored through
`reactions_received`, but **excluded from the gate's reciprocity measure** —
a thread whose entire inbound is taps yields `dm_inbound_replies ≈ 0`.

Accumulate in the existing `DmAccum` sidecar (so multi-thread → one handle
composes correctly), finalized onto `AccountFeatures` next to `dm_balance`.

## Step 3 — The effort-skew metric

Reuse the deduped balance shape, volume-guarded:

```
reply_skew = my_dm_out / (my_dm_out + dm_inbound_replies)    ∈ [0,1]; 1.0 = I talk, they don't
evidence   = my_dm_out ≥ effort_skew_min_dm_out              (owner genuinely invested)
```

- `my_dm_out` and `dm_inbound_replies` are both post-dedup.
- `reply_skew` is `None` when `my_dm_out + dm_inbound_replies == 0` (no thread)
  — the gate cannot fire without evidence.
- Reactions are deliberately absent from the ratio.
- **`effort_skew_min_dm_out == 0` is a sentinel that disables the gate
  entirely** (mirrors `deep_mutual_keep_days == 0`). It is _not_ "evidence
  bar of zero" — `my_dm_out ≥ 0` is always true, which would fire the gate on
  every thread. So: `gate_on = effort_skew_min_dm_out > 0`, and only when
  `gate_on` does `evidence = my_dm_out ≥ effort_skew_min_dm_out` apply.

## Step 4 — The tiered gate

A single metric, two thresholds, mirroring the owner's proposal: _the degree of
skew decides whether skew overrides the keep markers._ Both tiers are
**monotonic** — they only ever move Keep → Review, never manufacture Unfollow —
consistent with the gates-not-weights rule.

Two derived predicates, used below:

```
gate_on    = effort_skew_min_dm_out > 0                       (sentinel; see Step 3)
evidence   = gate_on && my_dm_out ≥ effort_skew_min_dm_out
soft_exempt = is_close_friend || is_favorited || account_class == Brand
```

Precedence in `scoring::assign_bucket` (new rungs marked ←):

```
1. is_restricted                                                      → Review    (unchanged, top floor)
2. is_droplisted                                                      → Unfollow  (unchanged)
3. HARD: !is_keeplisted && evidence && reply_skew ≥ effort_skew_hard   → Review    ← overrides close-friend/favorite/mutual/deep-mutual
4. deep-mutual floor                                                  → Keep       (unchanged)
5. keep_prob ≥ keep_min:
     SOFT: !is_keeplisted && !soft_exempt && evidence && reply_skew ≥ effort_skew_soft → Review  ←
     [existing reciprocity ceiling, if ever re-enabled, is checked in this same branch]
     else                                                             → Keep
6. keep_prob < unfollow_max (+gates)                                  → Unfollow / Review   (unchanged)
7. otherwise                                                          → Review
```

`is_keeplisted` is folded into both tier predicates (a `!`-guard, not a
standalone rung) so the ladder stays a clean sequence of bucket-assigning checks.

**Marker semantics — two distinct sets, named explicitly to retire the
overloaded term "IG marker":**

- **`soft_exempt = is_close_friend || is_favorited || account_class == Brand`**
  exempts the **SOFT** tier — soft only demotes plain, _unmarked_ personal
  accounts that scored into Keep on the owner's outbound. **`is_mutual` is
  deliberately NOT in `soft_exempt`**: a follow-back who never replies in a
  high-volume DM thread is the target, not an exception. (Deep mutuals never
  reach the soft tier anyway — rung 4 floors them to Keep first.)
- **The HARD tier overrides a strictly larger set** — everything in
  `soft_exempt` **plus** `is_mutual` and the deep-mutual floor (which it
  outranks by sitting at rung 3, above rung 4). Rationale: any IG-side marker
  can be **stale**, and extreme observed one-sidedness is evidence it no longer
  reflects the relationship.
- **igsift-side explicit intent** (`is_keeplisted`, `is_droplisted`) is a
  _deliberate, recent_ decision and is **never** overridden by skew. Droplist
  sits above everything (rung 2); `is_keeplisted` is the `!`-guard on both tier
  predicates, so a keeplisted account skips the gate entirely. Rationale: a
  keeplisted account is one the owner consciously chose to keep — Review would
  contradict that intent. (Flaggable: to let keeplist mean "never _unfollow_"
  yet still allow Review, drop the `!is_keeplisted` guard from the HARD
  predicate only.)

Relationship to the old gate: `require_reciprocity_for_keep` stays as-is (off).
The SOFT tier is its evidence-based, graded successor; we document the overlap in
DESIGN.md but do not remove the old toggle in this change.

## Step 5 — Config (`[scoring]`)

```toml
effort_skew_min_dm_out = 8     # evidence guard (owner messages, post-dedup); 0 disables the whole gate
effort_skew_soft       = 0.85  # demote unmarked Keep → Review
effort_skew_hard       = 0.95  # demote even close-friend / favorite / mutual → Review
```

- **Presets** (`balanced` / `engagement` / `tenure`): `effort_skew_min_dm_out = 0`
  → gate disabled, so a binary-only install is unaffected and the compiled-in
  fallback keeps current behavior.
- **`config/scoring.toml`** (owner): values above, gate on.
- `serde` default for `effort_skew_min_dm_out` is `0` (disabled) so older configs
  parse unchanged.
- Starting thresholds (since calibrated — see [`docs/TUNING.md`](../TUNING.md),
  which owns the live numbers). **At the initial `min_dm_out = 8`** the worked
  example (`my_dm_out = 6`) sat below the evidence bar and stayed Keep; the
  calibration round lowered the bar to capture it and the other over-messaged
  close friends. The droplist remains the manual escape hatch for anything the
  gate doesn't reach.

## Step 6 — Output

- **CSV:** new columns `reply_skew` (raw `0.0–1.0` float, empty when `None`) and
  `dm_inbound_replies` (raw count). Header is the DESIGN.md contract — append at
  the end so existing column positions are unchanged; update the documented
  header in DESIGN.md + the `tests/cli.rs` header assertion together.
- **Markdown / HTML:** a new `decision_hint` row via the shared SSOT in
  `src/output/mod.rs::decision_hint` — e.g. _"you do the talking — 18 sent, 1
  real reply"_ — inserted at the right precedence and added to the table-driven
  precedence test. Both writers call the shared function (no copy-paste).
- The metric is decision-support regardless of whether the gate fires, so it
  surfaces on every row with a thread, not only demoted ones.

## Step 7 — Testing (TDD)

- **Dedup (`aggregate.rs` unit):** a thread with N real messages + K
  "Liked a message" shadows → `dm_messages_total == N`, `dm_balance` computed on
  N, `dm_reactions_received` still counts the hearts from `reactions[]`.
  `dm_inbound_replies` excludes shadows.
- **Fixture counts (`tests/cli.rs`):** the locked-in integer counts will shift
  where the synthetic fixture contains shadows. **Diagnose and re-pin to the
  correct post-dedup value — do not relax the assertion** (CLAUDE.md). If the
  fixture has no shadow today, add one so the dedup path is exercised by the E2E
  count test.
- **Gate precedence (`scoring.rs` unit):** hard-beats-deep-mutual,
  hard-beats-close-friend, **hard-beats-mutual**, soft-respects-close-friend,
  soft-demotes-unmarked, **soft-demotes-non-deep-mutual** (mutual is NOT in
  `soft_exempt` — the case that exposed the spec inconsistency),
  evidence-guard-blocks-no-thread (`reply_skew` high but `my_dm_out` below
  `min_dm_out` → stays Keep), keeplist-survives-both-tiers,
  `min_dm_out == 0`-disables-gate (sentinel — must NOT fire on every thread),
  monotonic (gate never yields Unfollow).
- **Boundary:** `reply_skew == t_soft` / `== t_hard` inclusivity pinned (mirror
  the existing `keep_min` / `unfollow_max` boundary tests).

## Out of scope

- Re-weighting `reactions_received` (the issue was double-counting, not the
  weight; the dedup fixes it surgically).
- A non-DM skew signal (partial observability makes it untrustworthy).
- Removing `require_reciprocity_for_keep`.
- Threshold calibration numbers (a TUNING round, after implementation).
