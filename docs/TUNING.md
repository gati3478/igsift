# Tuning notes — `config/scoring.toml`

Running journal of weight / threshold / decay edits, with the empirical
distribution shift each one produced and the judgment behind it. Read
top-down (newest at top) when picking the next edit; each round was a
single TOML change so the contribution of each move is attributable.

The methodology choice for this pass was the **hybrid** in DESIGN.md's
"Open questions": iterate on the live ranking against the real export,
with `config/labels.txt` (when laid down) serving as a held-out accuracy
floor. The labels file is not committed — it's a per-user artifact.

## 2026-05-29 — halve `story_out` (round 5, weights)

First round run _after_ `story_likes.json` (~28k events) was folded into
`story_interactions_out`, against the 42-entry `config/labels.txt`
(28 matched the followings set; 14 were since-blocked/unfollowed).

### Verdict

`story_out` 1.0 → 0.5. Bucket split `510 / 130 / 9` (keep / review /
unfollow) on 649 followings; agreement `8/28` (28.6%); **hard mismatches
2 → 1**. Mirrored into `config/presets/balanced.toml` — the justification
is general, not user-specific.

### Before / after

```
story_out = 1.0:  525 / 115 / 9   agreement 10/28 (35.7%)   2 hard mismatches
story_out = 0.5:  510 / 130 / 9   agreement  8/28 (28.6%)   1 hard mismatch
```

Both hard mismatches at baseline were `label=drop`, `story_out`-dominated
accounts at `keep_prob` ≈ 0.84 and 0.92 — story-heavy follows the user
wants gone, inflated into `keep`.

### Why halve it (two independent reasons)

1. **Volume de-duplication.** Folding `story_likes` roughly doubled the
   events feeding `story_interactions_out`; leaving the weight at 1.0
   double-counts the signal. This argument is export-independent, which
   is why it goes into `balanced` too.
2. **It doesn't discriminate.** Among story-dominated labels the split is
   ~2 keep vs ~2 drop — a coin flip for intent. A non-discriminating
   signal shouldn't be a strong driver.

### Why agreement _fell_ yet this is correct

The two keep-agreements lost (a brand magazine page and a personal
story-heavy follow, both `keep`-labeled) were held in `keep` _only_ by
the inflated `story_out` term — coincidence, not signal. The 35.7%
leaned on that; 28.6% is the more honest floor.

### Ceiling finding — why we stopped tuning weights

Tuning is **near zero-sum** here (keep-recall vs drop-precision ~1:1)
because intent is not separable on the current features:

- **DM is the only clean signal** — every DM-dominated label is `keep`,
  no drop is DM-dominated. Already weighted highest (3.0).
- **`story_out` and `likes` are noisy** (mixed keep/drop among the labels
  each dominates).
- **~12 keep-labels are low-engagement** brand / local-business / sparse
  personal follows carried only by `tenure`. No global weight lifts them
  into `keep` without also lifting drop-intent _old_ follows (a
  `label=drop` personal account already sits at `keep_prob` ≈ 0.30,
  tenure-dominated — raising tenure promotes it too).

Pushing `story_out` to ≈0.3 to chase 0 hard mismatches was rejected: it
overfits one account and drops more story-keepers into Review.

### Open follow-up — the real fix is a feature, not a weight

The remaining hard mismatch and the low-engagement-keep misses both want
a **drop-list**: a user-maintained `config/drop_list.txt` mirroring
`keep_allowlist.txt`, gating `keep → review` (never auto-keep a
hand-flagged drop). With the existing keep-allowlist for the
low-engagement keeps, that covers what global weights structurally
cannot. Candidate v2 (see ROADMAP). Decay constants still unrevisited.

## 2026-05-27 — brand-lexicon expansion (round 4, structural)

Not a TOML edit — a code change to `BRAND_LEXICON` in
`src/features/account_class.rs` — but it shifts the brand-classification
counts on the real export and the rationale belongs in the same journal
as the weights rounds. Same shape as the round-3-precursor "brand gate
(structural change, not a weights edit)" entry below.

### Verdict

Brand count `19 → 44` on 643 followings. Bucket split `481 / 155 / 7`
**unchanged from the round-3 post-allowlist anchor** (`481 / 154 / 8` at
round-3 commit → `481 / 155 / 7` after `moonrisecrystals` was added to
`keep_allowlist.txt`; the round-4 lexicon then shifts nothing because
all 25 newly-classified brands already sit at
`keep_prob > unfollow_max = 0.35`, so the gate has no work to do at
current weights). Confusion matrix vs `config/labels.txt` also unchanged
(`agreement: 7/28, hard mismatches: none`) — the labeled brand-shaped
accounts were already in `bucket=review` based on `keep_prob` alone, and
brand-class promotion doesn't shift them out of Review (the gate floors
Unfollow → Review, not Personal → Keep). **Value is forward-looking
robustness**, not present-tense bucket improvement.

### Lexicon (before / after)

```
before (round-3 brand-gate slice, 8 tokens):
  official, studio, magazine, records, gallery, news, media, agency

after (round-4 expansion, 16 tokens):
  + books, press, games, store, comics  (5+ chars)
  + zine, shop, cafe                    (4 chars; floor relaxed)
```

Floor relaxed from ≥ 5 chars to ≥ 4 chars. The active rule is now
**empirical**: a token is added only after a 0-FP grep against the real
export. 3-char tokens (`bar`, `art`) are still deferred — they need
word-boundary matcher semantics (`klaras_bar` matches but `barbara`
doesn't), which is a structural matcher change not justified by the
marginal recall gain at current scale.

### Per-token audit (real export, 643 followees)

**Scope note (audit recommendation):** the FP counts below are
**0 false-positives against this specific 643-followee export**, not a
universal 0-FP claim. The substrings (`books`, `press`, `games`, …) are
substring-matched without word boundaries, so on a different user's
larger followee set they will hit personal handles that contain these
letter-runs (`audiobookslover`, `pressureguy`, `gamestop_lover`,
`bookshopper`, `cafelover_mara` are all plausible misses). The
brand-gate semantics (Unfollow → Review, never Personal → silently
suppressed Unfollow) keeps each such miss cheap — one manual triage
event — but a future maintainer porting the lexicon to a different
export should **re-run the per-token grep against their own followee
list** before trusting these numbers transitively.

| Token  | Chars | Hits | FPs | Net new brand catches¹                               |
| ------ | ----- | ---- | --- | ---------------------------------------------------- |
| books  | 5     | 6    | 0   | 6                                                    |
| press  | 5     | 7    | 0   | 7                                                    |
| games  | 5     | 5    | 0   | 4 (1 overlaps `press`)                               |
| store  | 5     | 1    | 0   | 1                                                    |
| comics | 6     | 1    | 0   | 1                                                    |
| zine   | 4     | 7    | 0   | 2 (5 already match `magazine`)                       |
| shop   | 4     | 2    | 0   | 2                                                    |
| cafe   | 4     | 1    | 0   | 1                                                    |
| —      |       |      |     | **24 net new** (raw expansion catches 25, minus dup) |

¹ Excludes accounts already caught by an existing lexicon token.

### Considered and held out

- `design` (6 chars, 4 hits, 0 FPs against the export). Held by Gati's
  call: handles like `indiebydesign` could be personal-designer
  portfolio accounts, not brands. The brand-gate flooring would be
  reasonable for explicit design studios but wrong for personal
  designers; `keep_allowlist.txt` is the right venue case-by-case.
- `bar` (3 chars, 3 real hits, 5 FPs without word boundaries:
  `bardic.cub`, `mimosa_barr`, `nbaratelli`, `waynebarlowe_thedarkness`,
  `thebarewytchproject`). Safe only under word-boundary semantics.
  Deferred until a labeled round shows the recall need.

### Side effect — test isolation

`tests/cli.rs::ig_mgr()` now sets the spawned binary's cwd to
`std::env::temp_dir()` so the binary's cwd-relative config lookups
(`config/scoring.toml`, `config/labels.txt`,
`config/keep_allowlist.txt`) miss the per-user files at the repo root.
Without this, `cargo test` after laying down a real `config/labels.txt`
contaminates `fixture_counts_match_expected` with a non-zero
allowlist size and 28 "labels not in scored set" warnings, even though
nothing in the fixture itself changed.

### Why we stopped

- The matrix-improvement value of the lexicon expansion is zero at
  current weights (newly-Brand accounts are already correctly bucketed
  as Review). Further lexicon growth without a corresponding labeled
  round shifting where the gate matters is unlikely to move the needle.
- Word-boundary semantics is the next structural lever if shorter
  tokens become necessary — but adding them now would be over-design
  for the recall gain available at current scale.

## 2026-05-27 — labeled round (round 3, weights)

First weights edit gated by `config/labels.txt`: 28 hand-labels (21 keep,
7 drop) spread across the top, bottom, and review band of `keep_prob`.
The labels file is per-user and gitignored — round shape, matrix, and
rationale are what's preserved here.

### Verdict

`481 / 154 / 8` (Keep / Review / Unfollow) on 643 followings, up from
`481 / 160 / 2`. Single edit: `unfollow_max 0.3 → 0.35`. Agreement
against labels improved 6/28 → 7/28 (21.4 % → 25.0 %). The single hard
mismatch at commit time (a brand-shaped shop-page, label=keep
∩ bucket=unfollow, identified in `keep_allowlist.txt` as the allowlist
target) was resolved post-commit via the user-side allowlist — see
**Addendum** below for the post-commit state.

Personal handles named in the original drafting of this entry have been
replaced with structural descriptors per the round-3 audit (privacy
posture in `CLAUDE.md`: per-user `keep`/`drop` intent paired with
followee identity is the same disclosure as the gitignored `labels.txt`).
Brand-business handles (`butt_news`, `moonrisecrystals`,
`kona_books_`, …) are retained because the brand names are public.

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
   correctly but doesn't push below the 0.3 cutoff. One labeled-drop
   sits just above `unfollow_max` at `keep_prob=0.302`; the other five
   land in `keep_prob ∈ [0.43, 0.68]`, all pure-tenure with no
   engagement — feature-indistinguishable from interleaved keep-labels
   in the same band, so not addressable by a pure-tenure weight cut
   without breaking the keeps.
3. **1 hard mismatch** — `moonrisecrystals` (label=keep, bucket=unfollow,
   0.276). Shop page; brand lexicon doesn't catch `moonrise` or
   `crystals`. Allowlist territory, not weight-tunable.

### Decision — raise `unfollow_max` (0.3 → 0.35)

The labeled-drop band has a discrete cluster right above the existing
cutoff: one labeled-drop at `keep_prob = 0.302`. No labeled-keep sits in
`[0.30, 0.35)` — the lowest labeled-keep `keep_prob`s are `butt_news`
0.264 (brand-gated to Review regardless) and `moonrisecrystals` 0.276
(already in Unfollow at the old cutoff). So widening Unfollow to 0.35
captures one labeled-drop into agreement with **zero risk** of pulling
a labeled-keep down. Same single-variable methodology as rounds 1 and 2
of the first calibration pass — the effect is fully attributable.

**Considered and rejected: `tenure 0.15 → 0.10`.** Would widen Unfollow
into the mid-review band. Empirical re-run during the round audit
showed the move would pull a labeled-keep account at
`keep_prob ≈ 0.40` (1471 d tenure, pure-tenure, business-page handle)
down to ~0.316 — under `unfollow_max = 0.35` and so into Unfollow,
creating a **new hard mismatch where none currently exists**. The
labeled-drops in the mid-review band stay above 0.35 even at
`tenure = 0.10` (lowest drop ends at ~0.27 only if its tenure is short
enough — none of the labeled drops have tenure short enough for that),
so the move trades structural-protection of long-tenure labeled-keeps
for ~0–1 new agreement. Soft-for-hard is strictly worse than the
smaller `unfollow_max` move.

_(The original draft of this rejection named two specific labeled-keep
accounts as the predicted hard mismatches; the audit re-derivation
found one of those two would actually have stayed in Review, and that
the real hard mismatch was a different account. The conclusion is
unchanged but the math was sloppy — preserved here as a methodology
note: when rejecting a knob, derive the displaced accounts from the
current scoring, don't reason in-head from feature shape alone.)_

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

The matrix moved exactly as predicted: the labeled-drop at 0.302 shifted
`bucket=review → bucket=unfollow`, joining a second labeled-drop at
0.297 (already in Unfollow under the old cutoff) in the
`label=drop ∩ bucket=unfollow` agreement cell. Five unlabeled accounts
in `[0.30, 0.35)` also moved to Unfollow — all zero-engagement,
short-to-medium tenure, no display name in the CSV (most likely
disabled or abandoned accounts the user impulse-followed).
Conservative widening into actionable territory.

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

- **RESOLVED 2026-05-27.** Add `moonrisecrystals` (and similar shop-page
  false negatives) to `keep_allowlist.txt`. The brand lexicon will never
  catch store names without unacceptable collateral. Actioned immediately
  post-commit; see Addendum below.
- The 5 mid-band labeled drops at `keep_prob ∈ [0.43, 0.68]` won't move
  via weights as currently designed. Realistic options: extend the
  labeled set toward 50+ entries to surface a clearer pattern; or accept
  that the keep_prob ranking is a manual-triage signal rather than a
  bucket-assignment ground truth for these accounts.
- Brand-lexicon expansion candidates this round surfaced: `books`
  (`kona_books_`), `bar` (`klaras_bar`), `zine` (would catch
  `danarti_zine` among others). **PARTIALLY RESOLVED** in round 4 —
  `books`, `zine`, plus six others added. `bar` deferred pending
  word-boundary semantics.

### Addendum — post-commit allowlist resolution

Immediately after the `unfollow_max` commit landed, the open follow-up
above was actioned: `moonrisecrystals` added to
`config/keep_allowlist.txt`. The allowlist gate floors
`Unfollow → Review`, producing the post-commit state below — this is
what the binary reports today.

```
bucket keep: 481
bucket review: 155
bucket unfollow: 7

confusion matrix:
                 bucket=keep  bucket=review  bucket=unfollow
  label=keep               5             16                0
  label=drop               0              5                2
agreement: 7/28 (25.0%)
hard mismatches: none
```

Agreement unchanged (a `label=keep ∩ bucket=unfollow` hard mismatch
becoming a `label=keep ∩ bucket=review` soft mismatch is not an
agreement gain). Hard-mismatch count: 1 → 0 — the cleanest matrix state
achievable without code changes. Round 4 then anchors against this
post-allowlist `481 / 155 / 7`, not the `481 / 154 / 8` at-commit state.

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
confirmed bottom-10 had _zero_ other contributions — `butt_news` plus
two personal handles (anonymized per the round-3 audit privacy posture)
were the canonical pure-tenure accounts at the bottom.

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
  `following-audit_<DATE>.md` — the Markdown summary already shows
  `display_name` and `dominant_feature` so you can label without
  re-opening Instagram. The template explains format and rationale.
- Decay constants stay first-pass guesses (180d DM, 365d content);
  revisit only when a specific account's ranking contradicts user
  judgment in a way that points at the decay term.
