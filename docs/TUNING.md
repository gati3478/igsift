# Tuning notes — `config/scoring.toml`

Running journal of weight / threshold / decay edits, with the empirical
distribution shift each one produced and the judgment behind it. Read
top-down (newest at top) when picking the next edit; each round was a
single TOML change so the contribution of each move is attributable.

The methodology choice for this pass was the **hybrid** in DESIGN.md's
"Open questions": iterate on the live ranking against the real export,
with `config/labels.txt` (when laid down) serving as a held-out accuracy
floor. The labels file is not committed — it's a per-user artifact.

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
