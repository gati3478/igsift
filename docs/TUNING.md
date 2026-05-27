# Tuning notes — `config/scoring.toml`

Running journal of weight / threshold / decay edits, with the empirical
distribution shift each one produced and the judgment behind it. Read
top-down (newest at top) when picking the next edit; each round was a
single TOML change so the contribution of each move is attributable.

The methodology choice for this pass was the **hybrid** in DESIGN.md's
"Open questions": iterate on the live ranking against the real export,
with `config/labels.txt` (when laid down) serving as a held-out accuracy
floor. The labels file is not committed — it's a per-user artifact.

## 2026-05-27 — labeled round (round 3, weights)

First weights edit gated by `config/labels.txt`: 28 hand-labels (21 keep,
7 drop) spread across the top, bottom, and review band of `keep_prob`.
The labels file is per-user and gitignored — round shape, matrix, and
rationale are what's preserved here.

### Verdict

`481 / 154 / 8` (Keep / Review / Unfollow) on 643 followings, up from
`481 / 160 / 2`. Single edit: `unfollow_max 0.3 → 0.35`. Agreement
against labels improved 6/28 → 7/28 (21.4 % → 25.0 %); the single hard
mismatch (`moonrisecrystals`, label=keep ∩ bucket=unfollow) persists
because the fix is allowlist-side, not weights-side.

### Before

```
labels: 28 total (21 keep, 7 drop)
confusion matrix:
                 bucket=keep  bucket=review  bucket=unfollow
  label=keep               5             15                1
  label=drop               0              6                1
agreement: 6/28 (21.4%)
hard mismatches (1): moonrisecrystals
```

Reading the matrix — three patterns, ranked by signal strength:

1. **15 keep-labels in bucket=review** — brand pages, business pages,
   IRL-known accounts. None have positive engagement features the
   algorithm can amplify (zero likes/comments/DMs × any weight = zero).
   Brand-gate already floors 4 of them to Review; the remaining 11 are
   allowlist or future lexicon-expansion territory.
2. **6 drop-labels in bucket=review** — algorithm marks them borderline
   correctly but doesn't push below the 0.3 cutoff. One (`san___ndro`,
   0.302) sits just above `unfollow_max`; the rest (`gosip.12` 0.426,
   `dvidmakesthings` 0.480, `mahouuun` 0.562, `_zeromus__` 0.649,
   `modekeyboards` 0.678) are pure-tenure with no engagement —
   indistinguishable from interleaved keep-labels in the same band, so
   not addressable by a pure-tenure weight cut without breaking the
   keeps.
3. **1 hard mismatch** — `moonrisecrystals` (label=keep, bucket=unfollow,
   0.276). Shop page; brand lexicon doesn't catch `moonrise` or
   `crystals`. Allowlist territory, not weight-tunable.

### Decision — raise `unfollow_max` (0.3 → 0.35)

The labeled-drop band has a discrete cluster right above the existing
cutoff: `san___ndro` at 0.302. No labeled-keep sits in `[0.30, 0.35)`
— the lowest labeled-keep `keep_prob`s are `butt_news` 0.264
(brand-gated to Review regardless) and `moonrisecrystals` 0.276
(already in Unfollow at the old cutoff). So widening Unfollow to 0.35
captures one labeled-drop into agreement with **zero risk** of pulling
a labeled-keep down. Same single-variable methodology as rounds 1 and 2
of the first calibration pass — the effect is fully attributable.

**Considered and rejected: `tenure 0.15 → 0.10`.** Would widen Unfollow
into the mid-review band but pull `firstpressgames` (label=keep, 412 d
tenure, 0.355) and `tamarashubitidze` (label=keep, 446 d, 0.470) into
Unfollow alongside `dvidmakesthings` (label=drop, 144 d, 0.480) — no
engagement signal to discriminate. Net would be +2 agreements but +2
new hard mismatches. Trading soft mismatches for hard ones is strictly
worse than the smaller move.

### After

```
bucket keep: 481
bucket review: 154
bucket unfollow: 8

confusion matrix:
                 bucket=keep  bucket=review  bucket=unfollow
  label=keep               5             15                1
  label=drop               0              5                2
agreement: 7/28 (25.0%)
hard mismatches (1): moonrisecrystals (unchanged)
```

The matrix moved exactly as predicted: `san___ndro` shifted
`bucket=review → bucket=unfollow`, joining `nikolszi` in the
`label=drop ∩ bucket=unfollow` agreement cell. Five unlabeled accounts
in `[0.30, 0.35)` also moved to Unfollow: `ivan28h`, `tiedtopsf`,
`lukasmuller19980`, `__deleted__bhiebeaaibgjcgedb`, `too_old_emulsion`
— all zero-engagement, short-to-medium tenure, no display name in the
CSV. Conservative widening into actionable territory.

### Why we stopped

- The dominant matrix pattern (15 keeps in review) isn't addressable by
  any single-knob TOML edit — those accounts have zero positive
  engagement features. The fix is either keep_allowlist expansion
  (user-side) or brand-lexicon widening (code-side, accepting more
  lexicon false positives for more brand recall). Neither is a weights
  edit.
- The secondary pattern (5 remaining drops in mid-review) is structurally
  limited: labeled drops at 0.43–0.68 share their feature shape
  (pure-tenure, zero engagement) with labeled keeps in the same band. A
  pure-tenure cut hits both — the same ceiling round 2 of the first
  calibration ran into.
- 21 % → 25 % agreement is the honest ceiling for a confusion matrix
  this thin given the structural limits above.

### Open follow-ups

- Add `moonrisecrystals` (and similar shop-page false negatives) to
  `keep_allowlist.txt`. The brand lexicon will never catch store names
  without unacceptable collateral.
- The 5 mid-band labeled drops (`gosip.12`, `dvidmakesthings`,
  `mahouuun`, `_zeromus__`, `modekeyboards`) won't move via weights as
  currently designed. Realistic options: extend the labeled set toward
  50+ entries to surface a clearer pattern; or accept that the keep_prob
  ranking is a manual-triage signal rather than a bucket-assignment
  ground truth for these accounts.
- Brand-lexicon expansion candidates this round surfaced: `books`
  (`kona_books_`), `bar` (`klaras_bar`), `zine` (would catch
  `danarti_zine` among others). Defer until the labeled set is thicker
  or a specific evaluation justifies absorbing the false-positive cost.

## 2026-05-27 — brand gate (structural change, not a weights edit)

The brand / public-figure account-class heuristic landed alongside the
keep-allowlist override (see ROADMAP). Not a TOML edit — a code-level
addition of a new Unfollow gate — but it shifts the bucket distribution
on the real export, so worth recording here for continuity:

```
before (round 2 weights, no brand gate):  481 / 159 / 3
after  (same weights, brand gate live):    481 / 160 / 2
```

One account (`butt_news`) moved Unfollow → Review on the `"news"`
substring match — a known acceptable false positive of the text-only
lexicon. 19 of 643 followings are now classified as `Brand`; only that
one was in the Unfollow band, so the gate's effect on the bucket split
is small at the current weights. The structural value is forward-looking:
the next round of weight tuning can widen Unfollow (lower
`unfollow_max` or further drop `tenure`) without re-introducing brand
false positives, because the gate catches them upstream.

### Open follow-ups after the brand gate

- Lay down `config/labels.txt` per the strategy in the
  `labels.txt.example` template (5 top / 5 bottom / 20 review-band).
  The confusion matrix becomes the held-out floor for the next
  weight edit.
- Consider widening the Unfollow band now that brands are filtered —
  candidates: raise `unfollow_max` from 0.3 to 0.35, OR drop `tenure`
  from 0.15 to 0.1. Either move pulls the long tail of `[0.3, 0.5)`
  tenure-only accounts (35 + 64 = 99 accounts) into a more actionable
  Unfollow recommendation. Hold off until labels land — the matrix
  tells us which of the two is closer to the user's intent.
- Lexicon false-positive triage: `butt_news` is the only known case
  on this export. If the labeled set confirms it should be Unfollow,
  the user can add it to `keep_allowlist.txt`'s **opposite** — there
  isn't one yet; allowlist semantics are "never unfollow". For now,
  the false positive surfaces as a Review-band account and the user
  handles it manually.

## 2026-05-27 — first calibration pass

Goal: move the bucket distribution from "everything is Keep" to a
meaningful three-way split, without introducing brand / public-figure
gating (that lands in a later ROADMAP slice).

### Verdict

**481 / 159 / 3** (Keep / Review / Unfollow) on 643 followings.

Two edits: `threshold 0.0 → 1.5`, `tenure 0.3 → 0.15`. Decay constants
(`tau_dm_days = 180`, `tau_content_days = 365`) unchanged — neither the
top nor the bottom of the ranking exhibited a recency-vs-staleness mismatch
that would have warranted touching them at this pass.

### Before

```
bucket keep: 641
bucket review: 2
bucket unfollow: 0
keep_prob histogram:
  [0.7, 0.8):   4
  [0.8, 0.9):  41
  [0.9, 1.0]: 598
```

Top-10 was healthy (`dm` / `likes` dominant). Bottom-10 sat at
`keep_prob 0.72–0.86` with `dominant=tenure` — every bottom account
cleared the Keep cutoff on tenure alone. Symptom: with `threshold = 0`,
even a one-year follow contributes ~1.78 raw before any engagement,
which sigmoids to ~0.86. Nothing could fall out of Keep.

### Round 1 — raise `threshold` (0.0 → 1.5)

Decision 2a from the slice prompt: re-centre the sigmoid so the tenure
floor lands inside the Review band rather than above the Keep cutoff.
Picked 1.5 (not 2.5) to keep the move attributable — a more aggressive
threshold would have moved tenure-only accounts into Unfollow in the
same step that pushed engaged accounts down, mixing two effects.

```
bucket keep: 579
bucket review: 64
bucket unfollow: 0
```

Bottom-10 dropped to `keep_prob 0.36–0.57`, all `dominant=tenure`.
Review band populated. Unfollow still empty — the lowest-scoring
account (`butt_news`, ~22-day pure-tenure follow) sat at 0.364, just
above `unfollow_max = 0.3`.

### Round 2 — halve `tenure` (0.3 → 0.15)

Decision 2c: the bottom of the ranking was **uniformly**
`dominant=tenure` after round 1, which is the explicit signal in the
slice prompt that `w_tenure` is doing more work than designed. `--trace`
confirmed bottom-10 had _zero_ other contributions — `butt_news` /
`nikolszi` / `gregorybonsignore` were pure-tenure accounts.

```
bucket keep: 481
bucket review: 159
bucket unfollow: 3
keep_prob histogram:
  [0.2, 0.3):    3
  [0.3, 0.4):   35
  [0.4, 0.5):   64
  [0.5, 0.6):   31
  [0.6, 0.7):   29
  [0.7, 0.8):   20
  [0.8, 0.9):   16
  [0.9, 1.0]:  445
```

The three Unfollows are all pure-tenure accounts with the _shortest_
follow durations — the "impulse-followed, never engaged" pattern. The
35 accounts in `[0.3, 0.4)` are the borderline cases; **deliberately
left in Review** rather than widened into Unfollow because brand and
public-figure accounts probably populate that band, and the
account-class heuristic that filters them lands in the next ROADMAP
slice. Unfollow stays narrow but trustworthy.

### Why we stopped

- Round 2's distribution has shape: the [0.9, 1.0] mass is the
  genuinely engaged accounts; the [0.3, 0.7) tail is the band the
  user should review by hand; the [0.2, 0.3) tail is the safe-to-drop
  candidates. Both ends look right.
- Further widening Unfollow without the brand/public-figure heuristic
  risks falsely flagging brand accounts — better to ship the heuristic
  first and re-tune afterwards.
- Decay constants (decision 2d) require per-account judgment on
  recent-vs-stale activity that this pass didn't surface a clear case
  for. Deferred until a later iteration has labeled data or a specific
  account that highlights the mismatch.

### Open follow-ups

- After the brand / public-figure account-class heuristic lands,
  re-run and decide whether to widen Unfollow (raise `unfollow_max` or
  further lower `tenure`).
- Lay down `config/labels.txt` (copy from `config/labels.txt.example`)
  and use the confusion-matrix report as the accuracy floor for the
  next round of edits. Recommended ~30-label distribution: **5 from
  the top of `keep_prob` (calibration), 5 from the bottom (calibration),
  20 from the Review band 0.3–0.7 (discriminative)**. Pick handles from
  `recommendations_<DATE>.md` — the Markdown summary already shows
  `display_name` and `dominant_feature` so you can label without
  re-opening Instagram. The template explains format and rationale.
- Decay constants stay first-pass guesses (180d DM, 365d content);
  revisit only when a specific account's ranking contradicts user
  judgment in a way that points at the decay term.
