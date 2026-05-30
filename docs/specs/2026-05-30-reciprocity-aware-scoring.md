# Reciprocity-aware scoring: keep-ceiling gate + deep-mutual keep-floor

**Status:** approved design, pre-implementation
**Date:** 2026-05-30
**Author:** Gati (design via brainstorming session)

## Problem

The scorer conflates **one-directional consumption** with **relationship**.
A followee you liked/story-viewed heavily but who never followed you back,
never DM'd you, and never reacted to you scores into `keep` purely on
outbound activity. Measured against a real export (649 accounts):

- bucket split is **510 keep / 130 review / 9 unfollow** — upside-down for a
  tool whose purpose is surfacing unfollow candidates.
- **130 of the 510 keeps** are non-mutual personal accounts with zero DMs and
  zero reactions — pure parasocial consumption inflated into "keep".

A second gap: `is_mutual` is computed but **never scored**, and the follower
timestamp (`FollowerEntry.followed_me_at`) is parsed then **discarded**, so
"how long have we _mutually_ followed each other" — a strong keep signal for
long-standing relationships — is invisible to scoring.

### What the data can and cannot do (the asymmetry)

An Instagram export is **good at confirming keeps** (reciprocity and deep
shared history are visible) and **bad at confirming unfollows** (disinterest
leaves no trace). Therefore scoring should own the _keep_ side via monotonic
gates; the **drop-list** owns the _unfollow_ side. Trying to push "I'm not
interested anymore" accounts to `unfollow` from signals alone is chasing
information the export does not contain.

## Design

Two changes to `scoring::assign_bucket`, both **monotonic safety gates** (not
weights), plus the wiring to compute true mutual-age.

### Rule 1 — reciprocity keep-ceiling gate

An account that scores into `keep` is demoted to `review` when its entire
relationship is one-directional consumption — **all** of:

- `account_class == Personal` (brands/news/shops are legitimately one-way
  follows — exempt)
- `!is_mutual`
- no inbound signal: `!inbound_dm_request && dm_reactions_received == 0`
  and not a two-way thread (`dm_balance` is not `Some(b < 1.0)`)
- not explicitly kept: `!is_favorited && !is_close_friend &&
!is_keep_allowlisted`

Effect: can only move `keep → review`. **Cannot produce an unfollow.** It is
the exact mirror of the existing brand/favorite _unfollow_ gate.

A fully one-sided thread you started (`dm_balance == Some(1.0)`, talking into
the void) counts as **no** reciprocity and does not exempt.

### Rule 2 — deep-mutual keep-floor

A mutual account whose **reciprocal age** ≥ `deep_mutual_keep_days` is floored
to `keep`. Reciprocal age = days since the _later_ of {you followed them, they
followed you back} — i.e. how long the relationship has been mutual.

Effect: can only move an account _up_ to `keep`. **Cannot produce an
unfollow.** Default threshold **730 days (2 years)**: long-term reciprocal
follows are real relationships worth keeping even with no recent engagement.

### Precedence (only the `keep_min` rung changes)

```
1. is_restricted              → Review     (unchanged)
2. is_drop_listed             → Unfollow   (unchanged)
3. deep-mutual floor fires    → Keep        ← NEW (Rule 2)
4. keep_prob >= keep_min      → Keep,
     unless reciprocity gate  → Review       ← NEW (Rule 1)
5. keep_prob < unfollow_max (+existing gates) → Unfollow   (unchanged)
6. else                       → Review     (unchanged)
```

The drop-list (rung 2) still outranks the deep-mutual floor — an explicit drop
intent beats a long mutual history. `is_restricted` still outranks everything.

## Configuration

New `[scoring]` keys, both `#[serde(default)]` so existing/preset TOMLs without
them still parse:

| key                            | type | default | meaning                        |
| ------------------------------ | ---- | ------- | ------------------------------ |
| `require_reciprocity_for_keep` | bool | `true`  | Rule 1 on/off                  |
| `deep_mutual_keep_days`        | u32  | `730`   | Rule 2 threshold; `0` disables |

`balanced.toml` mirrors `scoring.toml` (both gates on). The `engagement`
preset — which is explicitly about raw activity volume — sets
`require_reciprocity_for_keep = false`. The compiled-in fallback keeps both on.

## Data wiring

`followed_me_at` is already parsed into `FollowerEntry` but discarded at
aggregation. Plumb it through:

- `aggregate` builds a `handle → followed_me_at` map alongside the existing
  follower-handle set.
- New `AccountFeatures` field `mutual_age_days: Option<u32>` (matching the
  existing `follow_tenure_days` / `dm_recency_days` shape), computed as
  `days_since(max(followed_at, followed_me_at), now)` when both follow
  timestamps are present and the account is mutual; `None` otherwise.
- `None` mutual-age never satisfies the floor (conservative — a relationship
  we cannot date does not get auto-kept).

Caveat observed in the real export: ~81% of mutual accounts report
`followed_me_at` within a day of `followed_at` (simultaneous follow-back, or
exporter approximation), so for most accounts mutual-age equals follow-tenure.
The signal only diverges for older relationships — which is exactly the band
the 2-year floor cares about.

## Measured effect (real export, 649 accounts)

| metric                                  | before        | after (Rule 1 + Rule 2 @ 730d) |
| --------------------------------------- | ------------- | ------------------------------ |
| keep / review / unfollow                | 510 / 130 / 9 | ~427 / ~213 / 9                |
| accounts auto-kept by deep-mutual floor | —             | ~209                           |

Five-account validation sample (decisions known out-of-band):

| profile shape                            | intent   | result              | residual        |
| ---------------------------------------- | -------- | ------------------- | --------------- |
| 9.6yr mutual, recent likes               | keep     | **keep** ✅         | —               |
| 9.6yr mutual, no engagement              | keep     | **keep** ✅ (floor) | —               |
| 7yr mutual, "I know him"                 | keep     | **keep** ✅ (floor) | label was stale |
| non-mutual personal, low signal          | unfollow | review              | drop-list       |
| non-mutual personal, heavy one-way likes | unfollow | review              | drop-list       |
| 2.4yr mutual, story-driven, uninterested | unfollow | keep                | drop-list       |

The three unfollow-intent accounts cannot be reached by signals (disinterest
is invisible; a 2.4yr mutual reads as a relationship). They are drop-list
territory by design.

## Why gates, not weights

Both rules are monotonic and one-directional, so neither can manufacture the
expensive error (a wrongful unfollow): Rule 1 only refuses to auto-keep a
stranger, Rule 2 only refuses to drop a years-deep mutual. Their soundness does
**not** depend on the calibration labels being clean — which matters, because
`config/labels.txt` is a known-noisy oracle (it mixes true IRL intent with
"algorithm-correct-in-isolation" compromise entries). Label agreement is used
here as a regression tripwire, not as proof of correctness.

The penalty/boost variants explored during design scored higher on raw label
agreement but introduced hard mismatches (a flat non-mutual penalty wrongly
unfollowed 5 keep-labeled public figures the brand lexicon misses; a smooth
mutual-tenure boost wrongly kept 3 recent mutual drops). They were rejected:
weights inherit label noise and swing decisions both ways; gates do not.

## Out of scope

- Pushing the three unfollow-intent sample accounts to `unfollow` from signals
  (drop-list territory — explicitly accepted).
- A full relabel of `config/labels.txt` (separate task; requires the owner's
  per-account IRL knowledge).

## Test plan (TDD)

`src/scoring.rs` table-driven unit tests, hand-shaped `AccountFeatures`:

1. Reciprocity gate floors a non-mutual personal high-scorer to `review`.
2. Gate exemptions each keep `keep`: mutual / brand-class / favorited /
   close-friend / keep-allowlisted / inbound-reaction / two-way DM.
3. One-sided thread (`dm_balance == 1.0`) does **not** exempt.
4. Deep-mutual floor: mutual + age ≥ threshold → `keep` even from a low score.
5. Floor boundary is inclusive at exactly `deep_mutual_keep_days`.
6. Floor does not fire for `mutual_age == None`, for non-mutual, or below
   threshold.
7. Precedence: `is_drop_listed` and `is_restricted` still beat the floor.
8. `require_reciprocity_for_keep = false` and `deep_mutual_keep_days = 0`
   each disable their rule.

`src/features/aggregate.rs`: `mutual_since`/`mutual_age_days` computed as
`max(both timestamps)`; `None` when either timestamp or mutuality is absent.

Fixture-count tests in `tests/cli.rs` must still pass; update only if a new
CSV column is added (decision deferred — surfacing mutual-age in output is
optional and not required by either rule).
