# Dead-mutual review gate — design

**Date:** 2026-06-01
**Status:** approved (design)
**Related:** [`2026-06-01-nonmutual-close-tie-gate-design.md`](2026-06-01-nonmutual-close-tie-gate-design.md),
[`2026-05-31-effort-skew-gate-design.md`](2026-05-31-effort-skew-gate-design.md),
[`2026-05-30-reciprocity-aware-scoring.md`](2026-05-30-reciprocity-aware-scoring.md)

## Problem

A reciprocal follow (`is_mutual`) and a non-trivial `follow_tenure_days`
are **undecayed structural facts** — they cannot fade. An account can
therefore ride a near-`1.0` `keep_prob` on mutual + tenure alone while
having **no behavioural signal in either direction**: no DM sent or
received, no inbound reaction, and effectively zero recent outbound
engagement. The follow-back never became a relationship.

The existing relationship gates miss this class by construction:

- **non-reciprocal close-tie** requires `!is_mutual` — these are mutual.
- **reciprocity ceiling** targets one-way _consumption_ (you consume their
  content); these have ~zero consumption too.
- **effort-skew** requires owner DM volume (`dm_out >= min_dm_out`); these
  have no DM at all.
- **deep-mutual floor** _protects_ long mutuals; these are deliberately the
  _short_-tenure mutuals it does not reach.

The exemplar: a personal mutual at `keep_prob ≈ 1.0`, tenure just over a
year, `dm=0`, no inbound, one stray like in 90 days. The owner's judgement:
**Review** — "we've barely interacted and haven't been following that long,
unlike the long-tenure mutuals I keep with low engagement."

## Non-goals

- **Not** an Unfollow path. The class is defined by _absence_ of signal,
  which is weaker evidence than the marker- and volume-guarded gates;
  Review (human attention) is the only safe outcome.
- **Not** a re-weighting. Like the other relationship gates, correctness
  must not depend on the noisy `labels.txt` oracle — it is a monotonic
  gate, not a tuned weight. No paired penalty term (unlike the close-tie
  gate): the gate alone guarantees the Review floor, and these accounts
  already score high, so a penalty would be cosmetic.
- **Not** marker-exempt. A stale close-friend/favorite marker with zero
  interaction behind it does **not** float the account back to Keep —
  the owner explicitly wants these reviewed despite the marker. (Marked
  accounts with _real_ DM history are excluded by the inbound/`dm_out`
  clauses, not by the marker.)

## Predicate

`is_dead_mutual(f, p)` — all must hold:

| clause                  | field / helper                                                                     | meaning                                                |
| ----------------------- | ---------------------------------------------------------------------------------- | ------------------------------------------------------ |
| enabled                 | `p.dead_mutual_review_max_tenure_days > 0`                                         | `0` disables the gate                                  |
| personal                | `f.account_class == AccountClass::Personal`                                        | relationship gate, not for brands                      |
| mutual                  | `f.is_mutual`                                                                      | they followed back                                     |
| not keeplisted          | `!f.is_keeplisted`                                                                 | explicit keep opts out                                 |
| no inbound              | `!has_inbound_signal(f)`                                                           | they never replied / reacted / requested               |
| no outbound DM          | `dm_out(f) == 0`                                                                   | you never messaged them either                         |
| ~zero recent engagement | `f.likes_given_90d + f.comments_given_90d <= 1`                                    | at most one stray like; one like is not a relationship |
| short tenure            | `f.follow_tenure_days.is_some_and(\|d\| d < p.dead_mutual_review_max_tenure_days)` | younger than the owner's typical kept mutual           |

`has_inbound_signal` and `dm_out` are the existing scoring helpers, reused
verbatim — the inbound clause is the same "did the other party engage
toward you" test the reciprocity gate uses.

### Why each clause earns its place

- **No inbound + no outbound DM** is the spine: a thread that exists in
  either direction is a relationship signal and disqualifies the account.
  This is also what excludes the accounts whose DM history was only
  recently un-dropped (the no-display-name resolver fix): once their DMs
  attribute, `dm_out`/`has_inbound_signal` exclude them.
- **Engagement cap (`<= 1`)** is load-bearing, not decoration. Dropping it
  widens the set from 23 → 36 on the owner's export, sweeping in active
  one-sided likers (one at 42 likes/90d) — people whose content the owner
  clearly values. The cap keeps the gate to genuinely _inert_ mutuals.
  Story replies are intentionally **not** in the cap: a one-sided
  story-reply pattern (you reach out, they never reciprocate) is itself
  Review-worthy and the owner confirmed those belong in Review too.
- **Tenure threshold** is the separator the owner named. Keep-mutuals on
  the owner's export run a median tenure of ~821 days; the exemplar sits at
  ~372 (below the 25th percentile, ~437). The default
  `dead_mutual_review_max_tenure_days = 437` is that p25 — "younger than
  three-quarters of the mutuals you keep." Tunable in TOML; `0` disables.

## Placement in `assign_bucket`

A new rung **inside** the `keep_prob >= p.keep_min` block, beside the other
Keep→Review ceilings (it only ever demotes a would-be Keep). All rungs in
that block return `Bucket::Review`, so ordering among them is cosmetic; the
dead-mutual rung sits after the non-reciprocal close-tie ceiling. It is
strictly below the HARD effort-skew tier, the droplist, and the restricted
floor, and below the deep-mutual keep-floor — a long reciprocal history
(deep-mutual) wins, exactly as intended (these are short-tenure mutuals the
floor never reaches anyway).

Monotonic: Keep → Review only. It can never manufacture an Unfollow.

## Config

Mirror `deep_mutual_keep_days` (a `u32` day-threshold with a `0`-disables
sentinel and a `serde` default):

- `config.rs`: `[scoring]` field `dead_mutual_review_max_tenure_days: u32`,
  `#[serde(default = "default_dead_mutual_review_max_tenure_days")]`
  returning `437`. No `[weights]` change (pure gate).
- `config/scoring.toml` + all three presets
  (`balanced` / `engagement` / `tenure`): `dead_mutual_review_max_tenure_days = 437`.
  Shipped **on** in every preset, like the close-tie gate — Review-only,
  zero measured `labels.txt` regression. The `serde` default keeps a
  binary-only install (the compiled-in `balanced` fallback) gated.

## Decision hint

Add one row to `output::decision_hint` (shared by Markdown + HTML): a dead
mutual surfaces as e.g. `"inactive mutual — no contact either way"`.
Inserted at the right precedence in the chain; the table-driven precedence
test is extended to cover it. Mirrors `is_dead_mutual` semantically (the
predicate is private to `scoring`, so the hint re-expresses it, as the
close-tie hint does).

## Validation (owner export, post-resolver-fix data)

- 23 accounts caught; **0** collide with the 58 `labels.txt` entries (all
  unlabeled) — and the outcome is Review, so the safe direction by
  construction.
- The exemplar is caught; the deep-mutual / marked / DM-bearing neighbours
  are not.

## Test surface

- `scoring.rs`: `is_dead_mutual` unit coverage (each clause flips the
  result), a gate on/off control test, a "never yields Unfollow" test, and
  guards that a DM-bearing or long-tenure or keeplisted mutual is **not**
  demoted.
- `config.rs`: the new field parses, defaults to 437, and `0` disables.
- `output/mod.rs`: the new `decision_hint` row + precedence test extension.
- `tests/cli.rs`: re-pin fixture counts if the synthetic fixture exercises
  the gate (diagnose, don't relax, per CLAUDE.md).

## Companion change (separate, scoring.toml-only)

Lower `effort_skew_soft` 0.80 → 0.75 in `config/scoring.toml` only (presets
ship effort-skew off). Catches a faded, outbound-skewed mutual thread
(`reply_skew = 0.769`) that misses the current 0.80 bar by 0.031; +2
accounts on the owner export, 0 label regressions. Not part of the gate;
bundled because it closes the second account from the same triage pass.
