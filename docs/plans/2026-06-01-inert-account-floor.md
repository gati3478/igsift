# Inert-account Unfollow floor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the scorer from confidently Unfollowing accounts it has no behavioural data on — a personal account with zero signal in any direction (ranked into Unfollow on tenure alone) is floored to Review instead — and widen brand detection so curated brand/creator follows classify correctly.

**Architecture:** Two complementary changes in the existing "gates not weights" style. (A) A config-gated, monotonic rung in `scoring::assign_bucket`'s Unfollow block: `floor_inert_to_review && is_inert(f) && !is_deleted(f)` → Review. `is_inert` is lifetime-zero across every engagement counter plus the two negative-owner-action flags; `__deleted__` accounts are exempt (a gone account is a safe, certain drop). (B) Three new `BRAND_LEXICON` tokens, each 0-false-positive-verified against the real export. Both are Review-only and cannot manufacture a drop.

**Tech Stack:** Rust edition 2024, stable toolchain. `cargo nextest`, `cargo clippy -D warnings`, `cargo deny`. Config in TOML (`config/scoring.toml` + three presets). Tests are in-module `#[cfg(test)]` plus `tests/cli.rs` integration.

**Spec:** [`docs/specs/2026-06-01-inert-account-floor-design.md`](../specs/2026-06-01-inert-account-floor-design.md)

**Key facts the implementer must not rediscover:**

- `assign_bucket` lives in `src/scoring.rs:377`. The Unfollow block is `if keep_prob < p.unfollow_max { … }` at `src/scoring.rs:459-474`; the existing carve-out returns Review for `is_close_friend || is_favorited || is_keeplisted || account_class != Personal`. The new rung goes immediately after it, before `return Bucket::Unfollow`.
- The droplist returns `Bucket::Unfollow` at the **top** of `assign_bucket` (`src/scoring.rs:392-394`) and never reaches the Unfollow block, so a droplisted account is structurally immune to the inert floor — no ordering work needed.
- **The score reads decayed/windowed fields; `is_inert` reads lifetime raw counts.** Setting `acct.likes_given = 1` breaks inertness **without** changing `keep_prob` (the score uses `likes_given_decayed`, which stays 0). This is the lever the predicate-isolation tests use.
- `baseline_cfg()` (`src/scoring.rs:557`) is the only `ScoringParams` struct literal in the codebase; adding the field requires updating it. Set it `false` there (mirrors `dead_mutual_review_max_tenure_days: 0`) so existing tests are untouched and new tests enable the floor explicitly.
- The synthetic fixture buckets `keep 1 / review 3 / unfollow 0` (`tests/cli.rs:576-578`). No fixture account is in Unfollow, so the default-on floor cannot shift fixture counts — `tests/cli.rs` needs **no** changes.

---

### Task 1: Config field `floor_inert_to_review` (default true)

**Files:**

- Modify: `src/config.rs` (add field to `ScoringParams` ~line 152, add default fn ~line 191, add tests ~line 615)
- Modify: `src/scoring.rs:557-569` (add field to `baseline_cfg()`)
- Modify: `config/presets/balanced.toml`, `config/presets/engagement.toml`, `config/presets/tenure.toml`, `config/scoring.toml` (append key to `[scoring]`)

- [ ] **Step 1: Write the failing config tests**

Add to the `#[cfg(test)] mod tests` block in `src/config.rs`, after `dead_mutual_review_max_tenure_zero_disables` (~line 615):

```rust
    #[test]
    fn floor_inert_to_review_defaults_to_true() {
        // A config body with no `floor_inert_to_review` key parses with the
        // floor ON by default — like the dead-mutual / close-tie gates it ships
        // live (high-precision, Review-only, monotonic). 0/false disables.
        let cfg = parse_str(VALID_BODY, "/synthetic.toml").expect("parses");
        validate(&cfg).expect("validates");
        assert!(cfg.scoring.floor_inert_to_review);
    }

    #[test]
    fn floor_inert_to_review_false_disables() {
        let body = format!("{VALID_BODY}floor_inert_to_review = false\n");
        let cfg = parse_str(&body, "/synthetic.toml").expect("parses");
        assert!(!cfg.scoring.floor_inert_to_review);
    }
```

- [ ] **Step 2: Run the tests to verify they fail to compile (field missing)**

Run: `cargo nextest run -p igsift config:: 2>&1 | tail -20`
Expected: compile error — `no field 'floor_inert_to_review' on type '&ScoringParams'`.

- [ ] **Step 3: Add the field + default fn**

In `src/config.rs`, add to `ScoringParams` immediately after the `dead_mutual_review_max_tenure_days` field (after line 151):

```rust
    /// When `true` (**default**), a personal account that would bucket
    /// Unfollow but shows **no behavioural signal in any direction** — zero
    /// engagement (likes/comments/story/stories-viewed/saves), no DM, no DM
    /// reactions in or out, no inbound request, and no negative owner action
    /// (`is_hide_story_from` / `is_removed_suggestion`) — is floored at
    /// `review` instead. Tenure alone is not a drop signal: absence of
    /// interaction is absence of evidence, not evidence to drop. Monotonic:
    /// Unfollow → Review only. `__deleted__` accounts are exempt (a gone
    /// account is the one safe, certain drop). `false` disables. See
    /// `docs/specs/2026-06-01-inert-account-floor-design.md`.
    #[serde(default = "default_floor_inert_to_review")]
    pub floor_inert_to_review: bool,
```

Add the default fn after `default_dead_mutual_review_max_tenure_days` (after line 191):

```rust
fn default_floor_inert_to_review() -> bool {
    // On by default — see docs/specs/2026-06-01-inert-account-floor-design.md.
    // High-precision (zero positive signal AND no negative owner action),
    // Review-only, monotonic. `__deleted__` accounts stay Unfollow.
    true
}
```

- [ ] **Step 4: Update the one `ScoringParams` literal so the crate compiles**

In `src/scoring.rs`, in `baseline_cfg()`, add the field after `dead_mutual_review_max_tenure_days: 0,` (line 568):

```rust
                dead_mutual_review_max_tenure_days: 0,
                floor_inert_to_review: false,
```

(False here, mirroring `dead_mutual_review_max_tenure_days: 0` — existing tests must be untouched; new tests in Task 2 enable the floor explicitly.)

- [ ] **Step 5: Append the key to all four config files**

Append to the end of the `[scoring]` section in **each** of `config/presets/balanced.toml`, `config/presets/engagement.toml`, `config/presets/tenure.toml`, and `config/scoring.toml`:

```toml
# Inert-account floor: a personal account with zero signal in any direction
# (no engagement, DM, reactions, inbound, or negative owner action) that would
# bucket Unfollow is floored to Review instead — tenure alone is not a drop
# signal. __deleted__ accounts stay Unfollow. false disables. See
# docs/specs/2026-06-01-inert-account-floor-design.md.
floor_inert_to_review = true
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo nextest run -p igsift config::`
Expected: PASS, including `floor_inert_to_review_defaults_to_true`, `floor_inert_to_review_false_disables`, and the existing `all_presets_parse_and_validate`.

- [ ] **Step 7: Commit**

```bash
git add src/config.rs src/scoring.rs config/scoring.toml config/presets/
git commit -m "feat(config): add floor_inert_to_review (default on) + preset wiring"
```

---

### Task 2: `is_inert` / `is_deleted` predicates + the gate rung

**Files:**

- Modify: `src/scoring.rs` (add predicates near `is_dead_mutual` ~line 330; add rung in the Unfollow block ~line 471; add tests at the end of the test module ~line 1714)
- Modify: `docs/specs/2026-06-01-inert-account-floor-design.md` (sync the `is_inert` code block to include the two negative-owner-action clauses)

- [ ] **Step 1: Write the failing scoring tests**

Add at the end of the `#[cfg(test)] mod tests` block in `src/scoring.rs`, just before its closing brace (line 1715):

```rust
    /// baseline_cfg has threshold = 0, so a strictly-inert account (no
    /// engagement, no penalties allowed) scores `score_raw = 0 → keep_prob =
    /// 0.5` and cannot go lower. To exercise the Unfollow-block rung we widen
    /// the band so 0.5 sits below unfollow_max, then turn the floor on.
    fn inert_unfollow_cfg() -> ScoringConfig {
        let mut cfg = baseline_cfg();
        cfg.scoring.unfollow_max = 0.6; // 0.5 (inert keep_prob) now in the unfollow band
        cfg.scoring.floor_inert_to_review = true;
        cfg
    }

    #[test]
    fn is_inert_each_signal_breaks_it() {
        assert!(is_inert(&baseline_account("z")), "all-zero baseline is inert");
        let mutations: &[(&str, fn(&mut AccountFeatures))] = &[
            ("likes_given", |a| a.likes_given = 1),
            ("comments_given", |a| a.comments_given = 1),
            ("story_interactions_out", |a| a.story_interactions_out = 1),
            ("stories_viewed", |a| a.stories_viewed = 1),
            ("saved_their_content", |a| a.saved_their_content = 1),
            ("dm_messages_total", |a| a.dm_messages_total = 1),
            ("dm_reactions_given", |a| a.dm_reactions_given = 1),
            ("dm_reactions_received", |a| a.dm_reactions_received = 1),
            ("inbound_dm_request", |a| a.inbound_dm_request = true),
            ("is_hide_story_from", |a| a.is_hide_story_from = true),
            ("is_removed_suggestion", |a| a.is_removed_suggestion = true),
        ];
        for (name, mutate) in mutations {
            let mut a = baseline_account("z");
            mutate(&mut a);
            assert!(!is_inert(&a), "{name} must break inertness");
        }
    }

    #[test]
    fn inert_floor_demotes_to_review() {
        let cfg = inert_unfollow_cfg();
        let acct = baseline_account("silent"); // all zero → inert, keep_prob 0.5
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(
            scored[0].keep_prob < cfg.scoring.unfollow_max,
            "must be in the unfollow band: {}",
            scored[0].keep_prob,
        );
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn inert_floor_off_unfollows() {
        // Same account, floor disabled → reaches its natural Unfollow. Proves
        // the demotion above is the floor's doing.
        let mut cfg = inert_unfollow_cfg();
        cfg.scoring.floor_inert_to_review = false;
        let acct = baseline_account("silent");
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn inert_floor_spared_by_a_single_like() {
        // is_inert reads lifetime raw counts; the score reads decayed fields.
        // One lifetime like breaks inertness WITHOUT moving keep_prob out of
        // the band — isolating the predicate. Stays Unfollow.
        let cfg = inert_unfollow_cfg();
        let mut acct = baseline_account("oneliker");
        acct.likes_given = 1;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(scored[0].keep_prob < cfg.scoring.unfollow_max);
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn inert_floor_not_applied_with_negative_owner_action() {
        // is_hide_story_from is a deliberate negative signal — real evidence to
        // drop. Not inert; stays Unfollow even with the floor on.
        let cfg = inert_unfollow_cfg();
        let mut acct = baseline_account("hidden");
        acct.is_hide_story_from = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn inert_floor_exempts_deleted_accounts() {
        // A __deleted__ account is gone — Unfollow is the safe, certain drop,
        // so it is exempt from the floor even though it is inert.
        let cfg = inert_unfollow_cfg();
        let acct = baseline_account("__deleted__abc123");
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(scored[0].keep_prob < cfg.scoring.unfollow_max);
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn inert_floor_applies_regardless_of_mutual() {
        // Unlike dead-mutual, the inert floor doesn't key on mutuality — a
        // non-deep mutual that is inert and in the unfollow band is floored too.
        // (mutual_age_days unset → the deep-mutual keep-floor doesn't fire.)
        let cfg = inert_unfollow_cfg();
        let mut acct = baseline_account("mutualsilent");
        acct.is_mutual = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn droplist_forces_unfollow_over_inert_floor() {
        // The droplist returns Unfollow at the top of assign_bucket, before the
        // inert floor can demote. An inert droplisted account stays Unfollow.
        let cfg = inert_unfollow_cfg();
        let mut acct = baseline_account("silentdrop");
        acct.is_droplisted = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Unfollow);
    }
```

- [ ] **Step 2: Run the tests to verify they fail to compile (predicates missing)**

Run: `cargo nextest run -p igsift scoring::tests::inert 2>&1 | tail -20`
Expected: compile error — `cannot find function 'is_inert' in this scope`.

- [ ] **Step 3: Add the predicates**

In `src/scoring.rs`, add immediately after `is_dead_mutual` (after line 330), before `has_inbound_signal`:

```rust
/// `true` when an account carries **no behavioural signal in any direction**:
/// zero engagement (likes/comments/story-interactions/stories-viewed/saves),
/// no DM at all, no DM reactions in or out, no inbound request, and no negative
/// owner action (`is_hide_story_from` / `is_removed_suggestion`). Its
/// `keep_prob` is pure tenure. Lifetime-zero — deliberately stricter than the
/// dead-mutual core's `<= 1` tolerance: the claim is "never interacted in any
/// way the export records." An account in Unfollow because of a negative owner
/// action is NOT inert — it has real evidence to drop and stays eligible.
/// Drives the inert-account Unfollow floor (Review, never Unfollow). See
/// docs/specs/2026-06-01-inert-account-floor-design.md.
fn is_inert(f: &AccountFeatures) -> bool {
    !f.is_hide_story_from
        && !f.is_removed_suggestion
        && f.likes_given == 0
        && f.comments_given == 0
        && f.story_interactions_out == 0
        && f.stories_viewed == 0
        && f.saved_their_content == 0
        && f.dm_messages_total == 0
        && f.dm_reactions_given == 0
        && f.dm_reactions_received == 0
        && !f.inbound_dm_request
}

/// `true` when the handle is Instagram's redaction for a deleted / deactivated
/// account (`__deleted__…`). Such an account is gone — Unfollow is the one safe,
/// certain drop — so it is exempt from the inert floor. Not igsift-emitted; if
/// IG ever changes the prefix the account simply degrades to Review (the safe
/// direction), never a wrongful keep.
fn is_deleted(f: &AccountFeatures) -> bool {
    f.username.starts_with("__deleted__")
}
```

- [ ] **Step 4: Add the gate rung**

In `src/scoring.rs`, inside the `if keep_prob < p.unfollow_max` block, immediately after the existing carve-out's closing brace (after line 473, the `}` that closes the `if f.is_close_friend || … { return Bucket::Review; }`) and before `return Bucket::Unfollow;`:

```rust
        // Inert-account floor: a personal account reaching Unfollow purely for
        // lack of positive signal — zero engagement in any direction, no DM, no
        // reactions, no inbound, and no negative owner action — has no evidence
        // FOR dropping, only an absence of data. Floor to Review. `__deleted__`
        // accounts are exempt: a gone account is the one safe, certain drop.
        // Monotonic (Unfollow → Review only). Sits below the droplist, which
        // returns Unfollow at the top of this fn and never reaches here. See
        // docs/specs/2026-06-01-inert-account-floor-design.md.
        if p.floor_inert_to_review && is_inert(f) && !is_deleted(f) {
            return Bucket::Review;
        }
```

- [ ] **Step 5: Run the new tests to verify they pass**

Run: `cargo nextest run -p igsift scoring::`
Expected: PASS — all eight new tests plus the unchanged existing scoring tests (notably `unfollow_when_no_boost_and_low_prob`, which still Unfollows because `baseline_cfg` has the floor off).

- [ ] **Step 6: Sync the spec's predicate**

In `docs/specs/2026-06-01-inert-account-floor-design.md`, replace the `is_inert` code block (under "Change A") so it matches the shipped predicate — prepend the two negative-owner-action clauses:

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

Add one sentence below it: "`is_hide_story_from` / `is_removed_suggestion` are deliberate negative owner→them actions — real evidence to drop — so an account carrying either is not inert and stays a genuine Unfollow candidate."

- [ ] **Step 7: Commit**

```bash
git add src/scoring.rs docs/specs/2026-06-01-inert-account-floor-design.md
git commit -m "feat(scoring): inert-account Unfollow floor (Review, __deleted__ exempt)"
```

---

### Task 3: Brand-lexicon recall — `design` / `studies` / `project`

**Files:**

- Modify: `src/features/account_class.rs:57-65` (extend `BRAND_LEXICON`) and the test module (~line 254, add a token test)

- [ ] **Step 1: Verify the candidate tokens are 0-false-positive against the real export**

The repo convention (`src/features/account_class.rs` module doc): a token ships only after a 0-FP grep against the real export's followee list. Extract the owner's export and grep the following list for each candidate:

```bash
cargo run --quiet -- check downloaded-ig-data >/dev/null 2>&1   # extracts to .igsift-extracted*/
FOLLOWING=$(ls .igsift-extracted*/connections/followers_and_following/following.json 2>/dev/null | head -1)
grep -ioE '"value": *"[^"]*(design|studies|project)[^"]*"' "$FOLLOWING" | sort -u
```

Expected: every hit is a brand/creator/page (e.g. public brand pages like `amanita_design_`, `projectfungus`, `thebarewytchproject`), zero personal handles. **If any personal handle matches a token, drop that token from this task** and note it in the Task 5 TUNING entry. (Use structural descriptors, never a real personal handle, when recording a dropped token.) (`design`/`studies`/`project` are each ≥4 chars; the deferred 3-char `art`/`bar` tokens and the word-boundary matcher rework stay out of scope per the spec.)

- [ ] **Step 2: Write the failing lexicon test**

Add to the `#[cfg(test)] mod tests` block in `src/features/account_class.rs`, after `four_char_tokens_zine_shop_cafe_match_real_handles` (~line 254):

```rust
    #[test]
    fn round12_tokens_match_real_handles() {
        // Round-12 expansion (inert-floor companion). NOTE (as-built): on the
        // 0-FP grep only `project` survived — the `design` / `studies` candidate
        // tokens matched personal handles and were dropped (see TUNING round 12),
        // so the shipped test asserts only the `project` hits (public brand pages).
        let c = empty();
        assert_eq!(c.classify("projectfungus", None), AccountClass::Brand);
        assert_eq!(c.classify("thebarewytchproject", None), AccountClass::Brand);
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo nextest run -p igsift account_class::tests::round12_tokens_match_real_handles`
Expected: FAIL — assertions return `AccountClass::Personal` (tokens not yet in the lexicon).

- [ ] **Step 4: Add the tokens**

In `src/features/account_class.rs`, extend `BRAND_LEXICON` (after the round-4 `"zine", "shop", "cafe",` line, before the closing `]`):

```rust
    // Round-12 expansion (inert-floor companion, docs/TUNING.md round 12):
    // curated one-way brand/creator follows the earlier lexicon missed. Each
    // ≥ 4 chars and 0-export-FP-verified on the owner's following list.
    "design", "studies", "project",
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo nextest run -p igsift account_class::`
Expected: PASS — `round12_tokens_match_real_handles` and all existing lexicon tests (including `short_tokens_excluded_to_avoid_false_positives` and `known_substring_false_positives_are_pinned`).

- [ ] **Step 6: Commit**

```bash
git add src/features/account_class.rs
git commit -m "feat(account-class): add design/studies/project brand tokens (0-FP verified)"
```

---

### Task 4: Documentation — DESIGN.md + CLAUDE.md

**Files:**

- Modify: `docs/DESIGN.md` (Buckets / account-class section)
- Modify: `CLAUDE.md` (relationship-gates paragraph)

- [ ] **Step 1: Document the floor in DESIGN.md**

In `docs/DESIGN.md`, find the section describing the Unfollow bucket / account-class gate (search for "never `unfollow`" or "account_class == Personal"). Add a paragraph describing the inert floor:

> **Inert-account floor.** A personal account reaching the Unfollow band purely
> for lack of positive signal — zero engagement in any direction, no DM, no
> reactions, no inbound, and no negative owner action (`hide_story` /
> `removed_suggestion`) — is floored to **Review**, not Unfollow. Tenure is not
> a drop signal: an account you have never interacted with is an absence of
> evidence, not evidence to drop. `__deleted__` accounts are exempt (gone =
> safe, certain drop). Config: `floor_inert_to_review` (default on); monotonic,
> Review-only. See `docs/specs/2026-06-01-inert-account-floor-design.md`.

- [ ] **Step 2: Document the floor in CLAUDE.md**

In `CLAUDE.md`, in the "Relationship gates are monotonic" paragraph (search for "dead-mutual gate"), append a sentence after the dead-mutual description:

> The **inert-account floor** (`scoring.floor_inert_to_review`, default
> **true**, `false` disables) is the Unfollow-side mirror of these keep-side
> gates: a personal account in the Unfollow band with **zero behavioural signal
> in any direction** (no engagement, DM, reactions, inbound, or negative owner
> action) is floored Unfollow → Review — tenure alone is not a drop signal.
> `__deleted__` handles are exempt (a gone account is a safe, certain drop). The
> predicate `is_inert` is the SSOT; ships **on** in every preset (Review-only,
> monotonic). See
> [`docs/specs/2026-06-01-inert-account-floor-design.md`](docs/specs/2026-06-01-inert-account-floor-design.md).

- [ ] **Step 3: Verify the docs build references resolve**

Run: `ls docs/specs/2026-06-01-inert-account-floor-design.md`
Expected: the file exists (link targets are valid).

- [ ] **Step 4: Commit**

```bash
git add docs/DESIGN.md CLAUDE.md
git commit -m "docs: document the inert-account Unfollow floor (DESIGN + CLAUDE)"
```

---

### Task 5: Full verification + label-regression measurement (TUNING round 12)

**Files:**

- Modify: `docs/TUNING.md` (add round 12 entry)

- [ ] **Step 1: Run the full gate suite**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo nextest run
cargo deny check advisories bans sources
```

Expected: fmt clean, clippy 0 warnings, all tests pass (the new config + scoring + lexicon tests plus the unchanged `tests/cli.rs` `keep 1 / review 3 / unfollow 0` fixture assertion), cargo-deny clean.

- [ ] **Step 2: Measure the bucket split + label agreement on the real export**

```bash
cargo run --quiet -- run downloaded-ig-data --out /tmp/igsift-r12
cut -d',' -f4 /tmp/igsift-r12.csv | tail -n +2 | sort | uniq -c
```

Expected: roughly `keep 522 / review ~116 / unfollow ~11` (down from the pre-change `522 / 90 / 37`). The run loads the owner's `config/scoring.toml` (now floor-on) and `config/labels.txt`, printing the confusion matrix + agreement in the summary. Record: the exact bucket counts, the agreement %, and confirm the `label=keep & bucket=unfollow` cell did **not** increase versus the pre-change run (the gate is monotonic Unfollow → Review, so it can only stay equal or fall).

- [ ] **Step 3: Confirm the Unfollow survivors are only droplist + `__deleted__`**

```bash
awk -F',' 'NR>1 && $4=="unfollow"{print $1}' /tmp/igsift-r12.csv
```

Expected: every surviving Unfollow handle is either on `config/droplist.txt` or starts with `__deleted__`. Any other survivor means an account is in Unfollow with a non-inert signal (a real penalty) — note it in the TUNING entry; it is not a bug, it is an account with genuine negative evidence.

- [ ] **Step 4: Write the TUNING round 12 entry**

Append a round 12 section to `docs/TUNING.md`. **Privacy: structural descriptors only — no raw personal followee handles. Brand-business public handles are quotable.** Record:

- The change: inert-account Unfollow floor (default on) + the three lexicon tokens.
- The 0-FP verification result from Task 3 Step 1 (which tokens shipped, any dropped).
- The bucket split before → after (`522 / 90 / 37` → measured) and the agreement delta.
- The confirmation that zero keep-labeled accounts moved toward Unfollow.
- The deferred follow-up pointer: Review inert/faded sub-grouping (output layer), see the spec's "Deferred follow-up" section.

Use the existing round-11 entry as the formatting template (descriptors like "a zero-engagement personal account at keep_prob=0.32").

- [ ] **Step 5: Commit**

```bash
git add docs/TUNING.md
git commit -m "docs: TUNING round 12 — inert floor + lexicon recall measurement"
```

---

## Self-Review

**Spec coverage:**

- Change A (inert floor) → Tasks 1 (config) + 2 (predicates + rung). ✓
- `__deleted__` carve-out → Task 2 (`is_deleted`, test `inert_floor_exempts_deleted_accounts`). ✓
- Config default-on, all presets + serde default → Task 1. ✓
- Change B (lexicon `design`/`studies`/`project`, 0-FP verified, word-boundary out of scope) → Task 3. ✓
- Monotonicity / below-droplist → Task 2 rung placement + test `droplist_forces_unfollow_over_inert_floor`. ✓
- Testing (predicate matrix, single-like spares, deleted stays, droplist forces, toggle off, mutual-irrelevant) → Task 2. ✓
- Label-regression + bucket-split measurement + TUNING round 12 + privacy → Task 5. ✓
- Deferred Review sub-grouping recorded → spec + Task 5 Step 4 pointer (out of scope for this plan). ✓
- Spec/code sync (hide_story refinement) → Task 2 Step 6. ✓

**Type/name consistency:** field `floor_inert_to_review: bool`; default fn `default_floor_inert_to_review`; predicates `is_inert(f: &AccountFeatures) -> bool`, `is_deleted(f: &AccountFeatures) -> bool`; test helper `inert_unfollow_cfg()`; tokens `"design"`, `"studies"`, `"project"`. Consistent across all tasks. The one `ScoringParams` literal (`baseline_cfg`) is updated in Task 1 Step 4 so the crate compiles before Task 2.

**Placeholder scan:** no TBD/TODO; every code step shows complete code; every run step states the exact command + expected result.
