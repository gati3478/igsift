# Non-reciprocal Close-Tie Penalty + Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Penalize and gate to Review any personal account the owner marked close-friend/favorited that never followed them back — the mirror-inverse of the existing parasocial exemption.

**Architecture:** One shared predicate `is_nonreciprocal_close_tie` drives two effects: a signed-negative penalty term in `score_raw` (score honesty) and a monotonic Keep→Review rung in `assign_bucket` (guaranteed floor). Penalty is weight-controlled and always-on (like `hide_story_penalty`); the gate is toggle-controlled and **on by default** (`demote_nonmutual_close_ties`, `serde` default `true`). Never reaches Unfollow on its own — the existing rung-6 close-friend/favorite gate catches the heavily-penalized case at Review.

**Tech Stack:** Rust edition 2024, `serde`/`toml` config, `cargo nextest`, TDD throughout.

**Spec:** [`docs/specs/2026-06-01-nonmutual-close-tie-gate-design.md`](../../specs/2026-06-01-nonmutual-close-tie-gate-design.md)

---

## File Structure

- `src/config.rs` — new `[weights]` field `nonmutual_close_tie_penalty` (required), new `[scoring]` toggle `demote_nonmutual_close_ties` (`serde` default `true`), `validate()` finiteness entry, `VALID_BODY` + config tests.
- `src/scoring.rs` — `is_nonreciprocal_close_tie` predicate, 17th term in `term_contributions` (`NUM_TERMS` 16→17), gate rung in `assign_bucket`, `baseline_cfg()` literal update, new unit tests.
- `src/output/mod.rs` — new `decision_hint` arm + precedence-test row + mutual-guard row.
- `config/scoring.toml`, `config/presets/{balanced,engagement,tenure}.toml` — the new weight (start 6.0) + toggle.
- `docs/DESIGN.md`, `CLAUDE.md`, `docs/TUNING.md` — docs + calibration round.

Branch already created: `feat/nonmutual-close-tie-gate` (spec committed at `f6a818d`).

---

## Task 1: Config plumbing (knobs, defaults, validation)

Adding a **required** weight field breaks every config-parse the instant the struct changes, so this task wires the field through all four TOMLs, the test fixture, the one struct literal, and `validate()` in one shot — landing with the whole suite green again.

**Files:**

- Modify: `src/config.rs` (`WeightsConfig`, `ScoringParams`, default fn, `validate`, `VALID_BODY`, tests)
- Modify: `src/scoring.rs` (`baseline_cfg()` literal only — keep it compiling)
- Modify: `config/scoring.toml`, `config/presets/balanced.toml`, `config/presets/engagement.toml`, `config/presets/tenure.toml`

- [ ] **Step 1: Write the failing config tests**

In `src/config.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn nonmutual_close_tie_defaults_to_demote_true() {
        // A config body with no `demote_nonmutual_close_ties` key parses with
        // the gate ON by default — unlike effort-skew/reciprocity, this gate
        // ships live (high-precision, Review-only). The weight is required, so
        // VALID_BODY must carry it (see VALID_BODY edit in this task).
        let cfg = parse_str(VALID_BODY, "/synthetic.toml").expect("parses");
        validate(&cfg).expect("validates");
        assert!(cfg.scoring.demote_nonmutual_close_ties);
    }

    #[test]
    fn nonmutual_close_tie_penalty_is_required() {
        // The penalty is a [weights] field with no serde default (every weight
        // is required). A body omitting it must fail to parse, naming the field.
        let body = VALID_BODY.replace("nonmutual_close_tie_penalty = 4.0\n", "");
        let err = parse_str(&body, "/synthetic.toml")
            .expect_err("missing required weight must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nonmutual_close_tie_penalty"),
            "error must name the missing weight: {msg}",
        );
    }

    #[test]
    fn non_finite_nonmutual_close_tie_penalty_is_rejected() {
        let body = VALID_BODY.replace(
            "nonmutual_close_tie_penalty = 4.0",
            "nonmutual_close_tie_penalty = nan",
        );
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("NaN penalty must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nonmutual_close_tie_penalty"),
            "error must name the offending weight: {msg}",
        );
    }
```

- [ ] **Step 2: Run the new tests to verify they fail to compile**

Run: `cargo test -p igsift --lib config:: 2>&1 | head -30`
Expected: compile error — `nonmutual_close_tie_penalty` is not a field of `WeightsConfig`, `demote_nonmutual_close_ties` not a field of `ScoringParams`.

- [ ] **Step 3: Add the weight field to `WeightsConfig`**

In `src/config.rs`, in `struct WeightsConfig`, after `pub inbound_request: f64,`:

```rust
    /// Subtracted from `score_raw` for a personal, non-mutual account the
    /// owner marked close-friend/favorited (an unreciprocated explicit tie).
    /// Always-on like the other penalties — `0.0` disables the score erosion;
    /// the Review floor is governed separately by
    /// `ScoringParams::demote_nonmutual_close_ties`. See
    /// `scoring::is_nonreciprocal_close_tie`.
    pub nonmutual_close_tie_penalty: f64,
```

- [ ] **Step 4: Add the toggle + default fn to `ScoringParams`**

In `src/config.rs`, in `struct ScoringParams`, after the `effort_skew_hard` field:

```rust
    /// When `true` (**default**), a personal, non-mutual account marked
    /// close-friend/favorited (and not keeplisted) is floored at `review`
    /// instead of auto-keeping on the stale marker — the mirror-inverse of the
    /// reciprocity ceiling. Monotonic: Keep → Review only. See
    /// [`crate::scoring::assign_bucket`]. Unlike effort-skew/reciprocity this
    /// defaults ON: it is high-precision (explicit marker + non-mutual +
    /// personal) and never produces Unfollow.
    #[serde(default = "default_demote_nonmutual_close_ties")]
    pub demote_nonmutual_close_ties: bool,
```

After `default_effort_skew_hard`:

```rust
fn default_demote_nonmutual_close_ties() -> bool {
    // On by default — see docs/specs/2026-06-01-nonmutual-close-tie-gate-design.md.
    // High-precision (explicit marker + non-mutual + personal), Review-only.
    true
}
```

- [ ] **Step 5: Add the weight to the `validate()` finiteness loop**

In `src/config.rs::validate`, in the `for (name, w) in [ ... ]` array, after the `("inbound_request", cfg.weights.inbound_request),` entry:

```rust
        (
            "nonmutual_close_tie_penalty",
            cfg.weights.nonmutual_close_tie_penalty,
        ),
```

- [ ] **Step 6: Add the weight to `VALID_BODY`**

In `src/config.rs`, in the `VALID_BODY` const, in the `[weights]` block after `inbound_request = 0.5`:

```
nonmutual_close_tie_penalty = 4.0
```

- [ ] **Step 7: Update the `baseline_cfg()` struct literal in `src/scoring.rs`**

In `src/scoring.rs` tests, in `baseline_cfg()`: in the `WeightsConfig { ... }` literal after `inbound_request: 0.5,` add `nonmutual_close_tie_penalty: 0.0,` (penalty OFF in the baseline so existing tests are unchanged); in the `ScoringParams { ... }` literal after `effort_skew_hard: 0.95,` add `demote_nonmutual_close_ties: false,` (gate OFF in the baseline so existing keep-tests are unchanged — new tests opt in explicitly, mirroring `skew_cfg()`).

- [ ] **Step 8: Add the weight + toggle to the four production TOMLs**

`config/scoring.toml` — in `[weights]` after the `reactions_given` line block, in the penalties group after `removed_suggestion_penalty = 0.3`:

```toml
nonmutual_close_tie_penalty = 6.0 # personal + marked close-friend/favorited + they
                                  # don't follow back: unreciprocated explicit tie.
                                  # Starting value; calibrated in TUNING round 11.
```

`config/scoring.toml` — in `[scoring]`, after the effort-skew block:

```toml
demote_nonmutual_close_ties = true # floor an unreciprocated close-friend/favorite at review
```

`config/presets/balanced.toml`, `engagement.toml`, `tenure.toml` — in each `[weights]` penalties group after `removed_suggestion_penalty = 0.3`:

```toml
nonmutual_close_tie_penalty = 6.0
```

(Presets omit the toggle — it defaults `true`, matching how the presets omit the effort-skew keys and inherit their serde defaults.)

- [ ] **Step 9: Build and run the config tests**

Run: `cargo build --all-targets 2>&1 | tail -5 && cargo test -p igsift --lib config:: 2>&1 | tail -20`
Expected: build OK; all `config::` tests PASS, including the three new ones and the existing `all_presets_parse_and_validate` / `builtin_default_parses`.

- [ ] **Step 10: Commit**

```bash
git add src/config.rs src/scoring.rs config/scoring.toml config/presets/
git commit -m "feat(config): nonmutual_close_tie_penalty weight + demote toggle"
```

---

## Task 2: Predicate + penalty term

**Files:**

- Modify: `src/scoring.rs` (`is_nonreciprocal_close_tie`, `term_contributions`, `NUM_TERMS`, tests)

- [ ] **Step 1: Write the failing penalty tests**

In `src/scoring.rs` tests (after the existing penalty-magnitude tests), add:

```rust
    #[test]
    fn nonmutual_close_friend_penalty_applies() {
        // Personal + non-mutual + close_friend (baseline_account is non-mutual,
        // Personal, not keeplisted) → close_friend_boost(+5) minus penalty(6) = -1.
        let mut cfg = baseline_cfg();
        cfg.weights.nonmutual_close_tie_penalty = 6.0;
        let mut acct = baseline_account("redflag");
        acct.is_close_friend = true;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - (-1.0)).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn nonmutual_favorite_penalty_applies() {
        // favorite_boost(+3) minus penalty(6) = -3.
        let mut cfg = baseline_cfg();
        cfg.weights.nonmutual_close_tie_penalty = 6.0;
        let mut acct = baseline_account("redflag");
        acct.is_favorited = true;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - (-3.0)).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn mutual_close_friend_has_no_penalty() {
        // is_mutual = true → predicate false → full +5 boost, no erosion.
        let mut cfg = baseline_cfg();
        cfg.weights.nonmutual_close_tie_penalty = 6.0;
        let mut acct = baseline_account("realfriend");
        acct.is_close_friend = true;
        acct.is_mutual = true;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - 5.0).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn brand_close_friend_has_no_penalty() {
        // account_class == Brand → predicate excludes it (brands legitimately
        // don't follow back). Full boost, no penalty.
        let mut cfg = baseline_cfg();
        cfg.weights.nonmutual_close_tie_penalty = 6.0;
        let mut acct = baseline_account("brandtie");
        acct.is_close_friend = true;
        acct.account_class = AccountClass::Brand;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - 5.0).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn keeplisted_nonmutual_close_friend_has_no_penalty() {
        // is_keeplisted folded into the predicate → explicit keep opts out.
        let mut cfg = baseline_cfg();
        cfg.weights.nonmutual_close_tie_penalty = 6.0;
        let mut acct = baseline_account("kepttie");
        acct.is_close_friend = true;
        acct.is_keeplisted = true;
        let score_raw = score(std::slice::from_ref(&acct), &cfg)[0].score_raw;
        assert!((score_raw - 5.0).abs() < 1e-12, "score_raw={score_raw}");
    }

    #[test]
    fn nonmutual_close_tie_penalty_can_be_dominant_feature() {
        // |penalty 6| > |close_friend_boost 5| → surfaces as dominant_feature.
        let mut cfg = baseline_cfg();
        cfg.weights.nonmutual_close_tie_penalty = 6.0;
        let mut acct = baseline_account("redflag");
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].dominant_feature, "nonmutual_close_tie_penalty");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p igsift --lib scoring::tests::nonmutual 2>&1 | tail -20`
Expected: FAIL — penalty has no effect yet (score_raw = +5/+3, dominant = "close_friend_boost").

- [ ] **Step 3: Add the predicate**

In `src/scoring.rs`, after the `is_parasocial` function (and before `has_inbound_signal`), add:

```rust
/// `true` when an account is an **unreciprocated explicit tie**: a personal
/// account the owner marked close-friend or favorited that never followed them
/// back. The mirror-inverse of [`is_parasocial`] — where that exempts
/// close-friend/favorited, this targets exactly that pair when combined with
/// non-mutuality. `is_keeplisted` is folded in so an explicit keep opts out;
/// brands are excluded (they legitimately don't follow back). Drives both the
/// penalty term and the keep-ceiling gate.
fn is_nonreciprocal_close_tie(f: &AccountFeatures) -> bool {
    f.account_class == AccountClass::Personal
        && !f.is_mutual
        && (f.is_close_friend || f.is_favorited)
        && !f.is_keeplisted
}
```

- [ ] **Step 4: Bump `NUM_TERMS` and add the term**

In `src/scoring.rs`, change `pub const NUM_TERMS: usize = 16;` to `pub const NUM_TERMS: usize = 17;`.

In `term_contributions`, after the `let dm_balance_term = ...;` / `let reaction_balance_term = ...;` lines, add:

```rust
    let nonmutual_close_tie = if is_nonreciprocal_close_tie(f) {
        w.nonmutual_close_tie_penalty
    } else {
        0.0
    };
```

In the returned array, after the final `("removed_suggestion_penalty", -removed_suggestion),` entry, add:

```rust
        ("nonmutual_close_tie_penalty", -nonmutual_close_tie),
```

- [ ] **Step 5: Run the penalty tests to verify they pass**

Run: `cargo test -p igsift --lib scoring::tests::nonmutual 2>&1 | tail -20`
Expected: PASS (all six). Also run the full scoring module to confirm no regression: `cargo test -p igsift --lib scoring:: 2>&1 | tail -10` → all PASS.

- [ ] **Step 6: Commit**

```bash
git add src/scoring.rs
git commit -m "feat(scoring): non-reciprocal close-tie penalty term"
```

---

## Task 3: The Keep→Review gate

**Files:**

- Modify: `src/scoring.rs` (`assign_bucket`, tests)

- [ ] **Step 1: Write the failing gate tests**

In `src/scoring.rs` tests, add:

```rust
    #[test]
    fn gate_demotes_nonmutual_close_friend_keeper() {
        // Penalty stays 0 (baseline) so the account still scores into Keep —
        // isolates the gate. close_friend(+5) → keep_prob ≈ 0.993 ≥ keep_min.
        let mut cfg = baseline_cfg();
        cfg.scoring.demote_nonmutual_close_ties = true;
        let mut acct = baseline_account("ghosttie");
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob >= cfg.scoring.keep_min,
            "must score into Keep first: {}",
            scored[0].keep_prob,
        );
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn gate_demotes_nonmutual_favorite_keeper() {
        let mut cfg = baseline_cfg();
        cfg.scoring.demote_nonmutual_close_ties = true;
        let mut acct = baseline_account("favtie");
        acct.is_favorited = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn gate_off_keeps_nonmutual_close_friend() {
        let cfg = baseline_cfg(); // demote_nonmutual_close_ties = false
        let mut acct = baseline_account("ghosttie");
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Keep, "gate off → stays Keep");
    }

    #[test]
    fn gate_respects_keeplist() {
        let mut cfg = baseline_cfg();
        cfg.scoring.demote_nonmutual_close_ties = true;
        let mut acct = baseline_account("kepttie");
        acct.is_close_friend = true;
        acct.is_keeplisted = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(
            scored[0].bucket,
            Bucket::Keep,
            "keeplist folded into predicate → gate skips it",
        );
    }

    #[test]
    fn gate_ignores_mutual_close_friend() {
        let mut cfg = baseline_cfg();
        cfg.scoring.demote_nonmutual_close_ties = true;
        let mut acct = baseline_account("realfriend");
        acct.is_close_friend = true;
        acct.is_mutual = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Keep);
    }

    #[test]
    fn gate_never_yields_unfollow() {
        // A huge penalty drags keep_prob below unfollow_max, but the existing
        // rung-6 close-friend/favorite gate floors it at Review — never Unfollow.
        let mut cfg = baseline_cfg();
        cfg.scoring.demote_nonmutual_close_ties = true;
        cfg.weights.nonmutual_close_tie_penalty = 20.0; // 5 - 20 = -15
        let mut acct = baseline_account("buriedtie");
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob < cfg.scoring.unfollow_max,
            "must score into the Unfollow range: {}",
            scored[0].keep_prob,
        );
        assert_eq!(scored[0].bucket, Bucket::Review, "floored at Review, not Unfollow");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p igsift --lib scoring::tests::gate_ 2>&1 | tail -20`
Expected: FAIL — `gate_demotes_*` and `gate_never_yields_unfollow` assert Review but get Keep (gate not implemented). `gate_off_*`, `gate_respects_keeplist`, `gate_ignores_mutual` pass already (no gate ⇒ Keep).

- [ ] **Step 3: Add the gate rung**

In `src/scoring.rs::assign_bucket`, inside the `if keep_prob >= p.keep_min {` block, **after** the SOFT effort-skew block and **before** the reciprocity-ceiling block, insert:

```rust
        // Non-reciprocal close-tie ceiling: an explicit close-friend/favorite
        // marker on a personal account that never followed back is a red flag,
        // not a keep signal. The penalty term may already have eroded the
        // score; this guarantees the Review floor when it didn't. Monotonic
        // (Keep → Review only) — the mirror-inverse of the reciprocity ceiling
        // below. keeplist is folded into the predicate, so an explicit keep
        // opts out. See docs/specs/2026-06-01-nonmutual-close-tie-gate-design.md.
        if p.demote_nonmutual_close_ties && is_nonreciprocal_close_tie(f) {
            return Bucket::Review;
        }
```

- [ ] **Step 4: Run the gate tests to verify they pass**

Run: `cargo test -p igsift --lib scoring::tests::gate_ 2>&1 | tail -20`
Expected: PASS (all six). Then full module: `cargo test -p igsift --lib scoring:: 2>&1 | tail -10` → all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/scoring.rs
git commit -m "feat(scoring): non-reciprocal close-tie Keep->Review gate"
```

---

## Task 4: Decision hint (Markdown + HTML)

**Files:**

- Modify: `src/output/mod.rs` (`decision_hint`, precedence test)

- [ ] **Step 1: Write the failing hint tests**

In `src/output/mod.rs` tests, add two rows to the `cases` array in `decision_hint_precedence_chain` (place them immediately after the `"keeplist beats close_friend"` case):

```rust
            Case {
                // The new red-flag arm beats the generic "marked close friend":
                // a personal, non-mutual, close-friend account is described as
                // the unreciprocated tie it is. Sits below keeplist, above the
                // plain close_friend/favorited arms.
                label: "non-reciprocal close tie beats marked close friend",
                expected: "close tie not reciprocated — they don't follow you back",
                mutate: |f| {
                    f.is_close_friend = true;
                    f.is_mutual = false; // baseline is_mutual = true
                },
                bucket: Bucket::Review,
            },
            Case {
                // Guard: a MUTUAL close friend still reports "marked close
                // friend" — the new arm requires non-mutuality.
                label: "mutual close friend still marked close friend",
                expected: "marked close friend",
                mutate: |f| {
                    f.is_close_friend = true; // is_mutual stays true (baseline)
                },
                bucket: Bucket::Keep,
            },
```

- [ ] **Step 2: Run to verify the new arm fails**

Run: `cargo test -p igsift --lib output::tests::decision_hint 2>&1 | tail -20`
Expected: FAIL on `"non-reciprocal close tie beats marked close friend"` — currently returns `"marked close friend"`. The mutual-guard row passes already.

- [ ] **Step 3: Add the hint arm**

In `src/output/mod.rs::decision_hint`, immediately after the `if f.is_keeplisted { return "explicit keeplist"; }` block and before `if f.is_close_friend { ... }`, insert:

```rust
    // A personal account marked close-friend/favorited that never followed
    // back — name the red flag rather than the bland "marked close friend".
    // (is_keeplisted already returned above, so the keeplist opt-out holds.)
    if !f.is_mutual
        && (f.is_close_friend || f.is_favorited)
        && matches!(f.account_class, AccountClass::Personal)
    {
        return "close tie not reciprocated — they don't follow you back";
    }
```

- [ ] **Step 4: Run the hint tests to verify they pass**

Run: `cargo test -p igsift --lib output:: 2>&1 | tail -15`
Expected: PASS — full precedence chain and all output tests green.

- [ ] **Step 5: Commit**

```bash
git add src/output/mod.rs
git commit -m "feat(output): non-reciprocal close-tie decision hint"
```

---

## Task 5: Full suite + fixture-count reconciliation

The integration tests in `tests/cli.rs` run the binary with `cwd = temp_dir`, so it loads the **compiled-in balanced preset** — which now ships the gate ON + penalty 6.0. If the synthetic fixture contains a personal, non-mutual, close-friend/favorited account, a bucket count will shift. Per CLAUDE.md, a shifted count means **diagnose and re-pin to the correct value — never relax the assertion.**

**Files:**

- Possibly modify: `tests/cli.rs` (re-pin exact counts only if they legitimately shift)

- [ ] **Step 1: Run the full test suite**

Run: `cargo nextest run 2>&1 | tail -30`
Expected: either all PASS (fixture has no such account — no count change), or a `tests/cli.rs` count assertion fails.

- [ ] **Step 2: If a `tests/cli.rs` count failed — diagnose, do not relax**

Identify which account moved: regenerate the fixture's audit and inspect the bucket of any personal non-mutual close-friend/favorited handle.

Run: `cargo run -- tests/fixtures/sample_export --out /tmp/fixture-audit 2>&1 | tail -5 && grep -E "personal" /tmp/fixture-audit.csv | awk -F, '$14=="false"'`

Confirm the moved account is genuinely a non-mutual personal close-friend/favorited (i.e. the gate _should_ demote it Keep→Review). If so, update the exact integer in the failing `tests/cli.rs` assertion to the new value and add a one-line comment noting the gate caused the shift. If the moved account is NOT such a shape, the gate has a bug — stop and re-open Task 2/3.

- [ ] **Step 3: Re-run the full suite**

Run: `cargo nextest run 2>&1 | tail -10`
Expected: all PASS.

- [ ] **Step 4: Commit (only if `tests/cli.rs` changed)**

```bash
git add tests/cli.rs
git commit -m "test(cli): re-pin fixture counts for close-tie gate demotion"
```

If no count changed, skip the commit and note "no fixture-count shift" in the next task's notes.

---

## Task 6: Calibration round on the real export

Sets the penalty **magnitude** empirically (the one open number from the spec) and records the round in `docs/TUNING.md`. Uses personal data under `downloaded-ig-data/` — do not commit any handle/label numbers; document with structural descriptors per the Privacy convention.

**Files:**

- Possibly modify: `config/scoring.toml` + 3 presets (final magnitude if 6.0 is wrong)
- Modify: `docs/TUNING.md` (new round)

- [ ] **Step 1: Run the scorer on the real export with the trace on the worked example**

Run: `cargo run -- downloaded-ig-data --trace <worked-example-handle> 2>&1 | tail -40`
(handle withheld per the Privacy convention — the personal followee paired with
drop intent is the same disclosure as the gitignored `config/labels.txt`.)
Expected: the trace shows `nonmutual_close_tie_penalty (-6.00)` as a term and the account's `keep_score` is visibly reduced from 1.000; bucket is Unfollow (it is droplisted) — confirm `top_signal` for non-droplisted siblings of this shape reads `nonmutual_close_tie_penalty`.

- [ ] **Step 2: Count the gate's footprint and check for collateral**

Run: `cargo run -- downloaded-ig-data --out /tmp/real-audit 2>&1 | tail -3 && grep -c "nonmutual_close_tie_penalty" /tmp/real-audit.csv`
Then run the config sanity + label agreement: `cargo run -- check downloaded-ig-data 2>&1 | tail -25`
Inspect: how many accounts moved Keep→Review, and whether `check`'s confusion matrix shows any **labeled-keep account demoted to Review** (a hard mismatch). A hard mismatch means that specific account is a keeplist candidate — it is not a reason to weaken the predicate.

- [ ] **Step 3: Decide the magnitude**

If the worked example and its cohort read as "clearly out of keep, visibly flagged" and there are **zero hard mismatches**, keep 6.0. If the erosion is too weak (still reads mid-keep) or too strong (eroding into a confusing deep-negative), adjust in 1.0 steps and re-run Steps 1–2. Update `config/scoring.toml` and all three presets to the final value together (they share one number).

- [ ] **Step 4: Document the round in `docs/TUNING.md`**

Append a new round section (match the existing house style — structural descriptors, no raw handles): the motivating shape (`a personal, non-mutual, close-friend-marked account at keep_prob≈1.000`), the chosen magnitude, the gate footprint (N accounts Keep→Review), and the hard-mismatch count. Reference the spec.

- [ ] **Step 5: Commit**

```bash
git add config/scoring.toml config/presets/ docs/TUNING.md
git commit -m "tune: calibrate nonmutual_close_tie_penalty (TUNING round 11)"
```

---

## Task 7: Documentation

**Files:**

- Modify: `docs/DESIGN.md` ("Scoring composition", "Bucket precedence")
- Modify: `CLAUDE.md` (relationship-gates Conventions paragraph)

- [ ] **Step 1: Update `docs/DESIGN.md` scoring composition**

Find the "Scoring composition" section's term/weight list and add a row for `nonmutual_close_tie_penalty` (a penalty, subtracted when personal + non-mutual + close-friend/favorited + not keeplisted). Find the "Bucket precedence" ladder and insert the new rung in the `keep_prob ≥ keep_min` block, alongside the reciprocity ceiling, noting it is gated by `demote_nonmutual_close_ties` (default on) and is Review-only.

- [ ] **Step 2: Update `CLAUDE.md` Conventions**

In the "Relationship gates are monotonic" paragraph, add a sentence documenting the non-reciprocal close-tie gate: the mirror-inverse of the reciprocity ceiling, on by default (unlike the others), penalty + Review-floor, never Unfollow, predicate folds in keeplist. Reference `docs/specs/2026-06-01-nonmutual-close-tie-gate-design.md`.

- [ ] **Step 3: Verify docs build-free (markdown only — visual scan)**

Run: `grep -n "nonmutual_close_tie" docs/DESIGN.md CLAUDE.md`
Expected: the new references appear in both files.

- [ ] **Step 4: Commit**

```bash
git add docs/DESIGN.md CLAUDE.md
git commit -m "docs: document the non-reciprocal close-tie gate"
```

---

## Task 8: Final verification gate

**Files:** none (verification only)

- [ ] **Step 1: Format**

Run: `cargo fmt --all && git diff --stat`
Expected: no unstaged formatting drift (or commit it if rustfmt touched anything).

- [ ] **Step 2: Clippy (CI treats warnings as errors)**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: zero warnings.

- [ ] **Step 3: Full test run**

Run: `cargo nextest run 2>&1 | tail -15`
Expected: all PASS.

- [ ] **Step 4: Supply-chain gate**

Run: `cargo deny check advisories bans sources 2>&1 | tail -10`
Expected: no new advisories/bans (this change adds no dependencies).

- [ ] **Step 5: Commit any fmt drift and report**

```bash
git add -A && git commit -m "chore: fmt" --allow-empty
git log --oneline f6a818d..HEAD
```

Then proceed to `superpowers:finishing-a-development-branch` to decide merge/PR.

---

## Self-Review Notes

- **Spec coverage:** predicate (Task 2 Step 3), penalty term + `NUM_TERMS` (Task 2), gate rung + precedence (Task 3), decision hint (Task 4), two knobs + `serde` default-true + `validate` (Task 1), CSV-via-top_signal (no column — Task 2 Step 1 `nonmutual_close_tie_penalty_can_be_dominant_feature`), calibration (Task 6), docs (Task 7), all touch-points (Tasks 1–7). Covered.
- **Type consistency:** field `nonmutual_close_tie_penalty` (weight, f64) and `demote_nonmutual_close_ties` (bool) used identically across config.rs, scoring.rs, and the TOMLs; predicate name `is_nonreciprocal_close_tie` consistent in Tasks 2–3; hint string byte-identical in Task 4's two sites (test expectation + impl).
- **No placeholders:** every code step shows full code; the only deferred value (penalty magnitude) is a real starting number (6.0) with an explicit calibration task (Task 6) to finalize it.
