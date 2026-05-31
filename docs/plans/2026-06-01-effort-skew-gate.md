# Effort-Skew Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop counting Instagram's "Liked a message" reaction-shadows as DM messages, and add a monotonic two-tier effort-skew gate that demotes Keep→Review for accounts the owner over-invests in (high outbound DM volume, near-zero real replies), guarded on DM evidence so it never touches off-IG relationships.

**Architecture:** Three coupled changes. (1) Dedup shadows in `walk_inbox_thread` so `dm_messages_total` / `dm_balance` stop being inflated. (2) A new `dm_inbound_replies` count separating real replies from taps. (3) A tiered gate in `scoring::assign_bucket` reading three new `[scoring]` config keys, plus a CSV column and a decision-hint arm for decision support. All gating is monotonic (Keep→Review only) and disabled by default (`effort_skew_min_dm_out == 0` sentinel) so presets and binary-only installs are unaffected.

**Tech Stack:** Rust 2024, serde/toml config, `csv` crate, `cargo nextest`. Design source: [`docs/specs/2026-05-31-effort-skew-gate-design.md`](../specs/2026-05-31-effort-skew-gate-design.md).

---

## File Structure

- `src/config.rs` — add 3 `ScoringParams` fields + serde defaults + validation.
- `src/features/aggregate.rs` — add `dm_inbound_replies` field; dedup + reply-count in `walk_inbox_thread`; `LIKE_SHADOW_CONTENT` const; dedup unit test.
- `src/scoring.rs` — the tiered gate in `assign_bucket` + helper fns + gate tests; update `baseline_cfg`/`baseline_account`.
- `src/output/csv.rs` — two new columns + header test.
- `src/output/mod.rs` — decision-hint skew arm + presentation consts + test rows.
- `src/output/markdown.rs`, `src/output/html.rs` — no logic change; update test `AccountFeatures` literals only (they inherit the new hint via the shared `decision_hint`).
- `src/labels.rs` — update test `AccountFeatures` literal only.
- `tests/cli.rs` — fixture shadow count bump + appended CSV header.
- `tests/fixtures/sample_export/.../carol_thread/message_1.json` — add one shadow message.
- `config/scoring.toml` — owner values (gate on).
- `docs/DESIGN.md`, `docs/TUNING.md` — header contract + precedence + note.

---

## Task 1: Config — three effort-skew params

**Files:**

- Modify: `src/config.rs:90-119` (struct + default fns)
- Modify: `src/config.rs:175-246` (`validate`)
- Modify: `src/scoring.rs:436-444` (`baseline_cfg` test builder)
- Test: `src/config.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test** — append to `src/config.rs` tests module (after `out_of_range_keep_min_is_rejected`, before the closing `}`):

```rust
    #[test]
    fn effort_skew_defaults_to_disabled() {
        // A config body with no effort-skew keys must parse, with the gate
        // OFF by default (min_dm_out == 0 sentinel) so presets / binary-only
        // installs keep current behavior.
        let cfg = parse_str(VALID_BODY, "/synthetic.toml").expect("parses");
        validate(&cfg).expect("validates");
        assert_eq!(cfg.scoring.effort_skew_min_dm_out, 0);
        assert_eq!(cfg.scoring.effort_skew_soft, 0.85);
        assert_eq!(cfg.scoring.effort_skew_hard, 0.95);
    }

    #[test]
    fn effort_skew_hard_below_soft_is_rejected() {
        let body = format!(
            "{VALID_BODY}effort_skew_min_dm_out = 8\n\
             effort_skew_soft = 0.9\neffort_skew_hard = 0.8\n"
        );
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("hard < soft must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("effort_skew_hard") && msg.contains("effort_skew_soft"),
            "error must reference both thresholds: {msg}",
        );
    }

    #[test]
    fn effort_skew_threshold_out_of_range_is_rejected() {
        let body = format!("{VALID_BODY}effort_skew_soft = 1.5\n");
        let cfg: ScoringConfig = toml::from_str(&body).expect("toml parses");
        let err = validate(&cfg).expect_err("soft > 1 must be rejected");
        assert!(format!("{err:#}").contains("effort_skew_soft"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::tests::effort_skew 2>&1 | tail -20`
Expected: FAIL — `no field effort_skew_min_dm_out on type ScoringParams` (compile error).

- [ ] **Step 3: Add the fields** — in `src/config.rs`, inside `struct ScoringParams` after the `deep_mutual_keep_days` field (line ~106):

```rust
    /// Evidence guard for the effort-skew gate: the gate only acts on a
    /// thread where the owner sent at least this many real (non-shadow)
    /// messages. **`0` disables the entire gate** (sentinel; mirrors
    /// `deep_mutual_keep_days == 0`) — it is NOT "evidence bar of zero",
    /// which would fire on every thread. See
    /// `docs/specs/2026-05-31-effort-skew-gate-design.md`.
    #[serde(default = "default_effort_skew_min_dm_out")]
    pub effort_skew_min_dm_out: u32,
    /// SOFT tier: an unmarked personal account scoring into Keep whose
    /// `dm_balance` (post-dedup reply skew) is ≥ this is demoted to Review.
    #[serde(default = "default_effort_skew_soft")]
    pub effort_skew_soft: f64,
    /// HARD tier: any account whose reply skew is ≥ this is demoted to
    /// Review even with a close-friend / favorite / mutual marker (keeplist
    /// and the restricted floor still win).
    #[serde(default = "default_effort_skew_hard")]
    pub effort_skew_hard: f64,
```

- [ ] **Step 4: Add the default fns** — after `default_deep_mutual_keep_days` (line ~119):

```rust
fn default_effort_skew_min_dm_out() -> u32 {
    // Disabled by default: the gate can override IG keep markers, so it must
    // be opt-in. The owner's config/scoring.toml turns it on.
    0
}

fn default_effort_skew_soft() -> f64 {
    0.85
}

fn default_effort_skew_hard() -> f64 {
    0.95
}
```

- [ ] **Step 5: Add validation** — in `validate`, before the final `Ok(())` (line ~245):

```rust
    for (name, v) in [
        ("effort_skew_soft", cfg.scoring.effort_skew_soft),
        ("effort_skew_hard", cfg.scoring.effort_skew_hard),
    ] {
        ensure!(
            (0.0..=1.0).contains(&v),
            "scoring.{name} must lie in [0, 1] (got {v}); it is a reply-skew ratio.",
        );
    }
    ensure!(
        cfg.scoring.effort_skew_hard >= cfg.scoring.effort_skew_soft,
        "scoring.effort_skew_hard ({}) must be >= scoring.effort_skew_soft ({}); \
         the hard (marker-overriding) tier cannot be easier to trip than the soft tier.",
        cfg.scoring.effort_skew_hard,
        cfg.scoring.effort_skew_soft,
    );
```

- [ ] **Step 6: Update the scoring test config builder** — in `src/scoring.rs`, inside `baseline_cfg()`'s `ScoringParams { ... }` (after `deep_mutual_keep_days: 730,`):

```rust
                effort_skew_min_dm_out: 0,
                effort_skew_soft: 0.85,
                effort_skew_hard: 0.95,
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test --lib config::tests::effort_skew 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 8: Commit**

```bash
git add src/config.rs src/scoring.rs
git commit -m "feat(config): effort-skew gate params (disabled by default)"
```

---

## Task 2: Add `dm_inbound_replies` field to `AccountFeatures`

This is a compile-only mechanical change — the field is initialized to `0` everywhere and populated in Task 3. The struct has no `Default`, so **every** literal must be updated or the build breaks.

**Files:**

- Modify: `src/features/aggregate.rs:99-180` (struct), `:278` (real builder), `:668` (`fake_features`)
- Modify: `src/scoring.rs:370` (`baseline_account`)
- Modify: `src/labels.rs:411`
- Modify: `src/output/mod.rs:198`, `src/output/csv.rs:144`, `src/output/markdown.rs:429`, `src/output/html.rs:800`

- [ ] **Step 1: Add the struct field** — in `src/features/aggregate.rs`, immediately after the `dm_balance: Option<f32>,` field (line ~162):

```rust
    /// The other party's **real** messages in resolved 1:1 threads —
    /// `dm_messages_total` minus the owner's outbound and minus the
    /// "Liked a message" reaction-shadows (see `LIKE_SHADOW_CONTENT`).
    /// Separates "they reply" from "they tap a heart": a thread whose entire
    /// inbound is taps has `dm_inbound_replies == 0`. Feeds the effort-skew
    /// gate's evidence/skew computation in [`crate::scoring`].
    pub dm_inbound_replies: u32,
```

- [ ] **Step 2: Run build to verify it fails**

Run: `cargo build --all-targets 2>&1 | grep -c "missing field \`dm_inbound_replies\`"`
Expected: a non-zero count (one error per construction site).

- [ ] **Step 3: Add `dm_inbound_replies: 0,` to every construction site**

In each literal below, add the line next to the other `dm_*` initializers:

- `src/features/aggregate.rs:278` (the real `aggregate` builder — near `dm_balance: None,`)
- `src/features/aggregate.rs:668` (`fake_features`)
- `src/scoring.rs:370` (`baseline_account`)
- `src/labels.rs:411`
- `src/output/mod.rs:198` (`baseline`)
- `src/output/csv.rs:144` (`make_scored`)
- `src/output/markdown.rs:429` (`baseline_features`)
- `src/output/html.rs:800`

Each gets exactly:

```rust
                dm_inbound_replies: 0,
```

(Match surrounding indentation — it differs per site.)

- [ ] **Step 4: Run build to verify it passes**

Run: `cargo build --all-targets 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add src/
git commit -m "feat(features): add dm_inbound_replies field (unpopulated)"
```

---

## Task 3: Dedup like-shadows + populate `dm_inbound_replies`

**Files:**

- Modify: `src/features/aggregate.rs` — add `LIKE_SHADOW_CONTENT` const + `is_like_shadow` helper near the top; rewrite the `walk_inbox_thread` loop body.
- Modify import: `src/features/aggregate.rs:60-63` — add `DmMessage` to the `crate::export` use list.
- Test: `src/features/aggregate.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test** — add to the `aggregate.rs` tests module:

```rust
    #[test]
    fn like_shadows_excluded_from_volume_and_balance() {
        use crate::export::{DmMessage, DmReaction, DmThread};
        let me = "Me";
        // Owner sends 3 real messages; the other party sends 1 real reply
        // and "likes" 2 of the owner's messages. IG serializes each like
        // BOTH as a reaction on the owner's message AND as a standalone
        // "Liked a message" from the other party. Post-dedup we want:
        //   dm_messages_total   = 4 (3 owner + 1 real reply; shadows dropped)
        //   dm_inbound_replies  = 1 (the real reply only)
        //   dm_reactions_received = 2 (from reactions[], still counted)
        let msg = |sender: &str, content: &str, reactions: Vec<DmReaction>| DmMessage {
            sender: Some(sender.to_owned()),
            timestamp: Some(Timestamp::from_second(1_700_000_000).unwrap()),
            content: Some(content.to_owned()),
            reactions,
        };
        let heart = || DmReaction {
            reaction: Some("❤".to_owned()),
            actor: Some("Them".to_owned()),
        };
        let thread = DmThread {
            folder: "them_1".to_owned(),
            title: Some("Them".to_owned()),
            participants: vec!["Them".to_owned(), me.to_owned()],
            messages: vec![
                msg(me, "hi", vec![heart()]),
                msg("Them", "Liked a message", vec![]),
                msg(me, "you there?", vec![heart()]),
                msg("Them", "Liked a message", vec![]),
                msg(me, "ok", vec![]),
                msg("Them", "yes!", vec![]),
            ],
        };

        let mut features = fake_features("them");
        let mut acc = DmAccum::default();
        let now = Timestamp::from_second(1_700_500_000).unwrap();
        walk_inbox_thread(&thread, &mut features, &mut acc, me, now, 180);

        assert_eq!(features.dm_messages_total, 4, "shadows excluded from volume");
        assert_eq!(features.dm_inbound_replies, 1, "only the real reply counts");
        assert_eq!(acc.outbound, 3, "owner real messages");
        assert_eq!(acc.inbound, 1, "their real messages (shadows excluded)");
        assert_eq!(features.dm_reactions_received, 2, "reactions[] still counted");
    }
```

(If `fake_features` / `DmAccum` / `walk_inbox_thread` are not already visible in the test module's scope, add `use super::*;` items as needed — `DmAccum` and `walk_inbox_thread` are module-private but tests are a submodule, so `super::` reaches them.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib aggregate::tests::like_shadows_excluded 2>&1 | tail -20`
Expected: FAIL — assertion `dm_messages_total == 4` gets `6` (shadows still counted).

- [ ] **Step 3: Add the constant + helper** — in `src/features/aggregate.rs`, after the imports (around line 66), add:

```rust
/// Instagram serializes a message *like* (double-tap heart) twice: in the
/// target message's `reactions[]` AND as a standalone message with this exact
/// content from the reactor. The reaction is the canonical record; this
/// standalone "shadow" is a duplicate and must not count as a conversational
/// message (it inflates `dm_messages_total` and corrupts `dm_balance`). Exact
/// match, not substring — a real reply containing the phrase is not a shadow.
/// Single chokepoint for the rule; a future IG variant extends here.
const LIKE_SHADOW_CONTENT: &str = "Liked a message";

fn is_like_shadow(msg: &DmMessage) -> bool {
    msg.content.as_deref() == Some(LIKE_SHADOW_CONTENT)
}
```

Add `DmMessage` to the existing `use crate::export::{...}` import block (line ~60):

```rust
use crate::export::{
    CommentEntry, DmMessage, DmThread, FollowerEntry, FollowingEntry, MeIdentity, ShapeAEntry,
    ShapeCEntry, owner_username,
};
```

- [ ] **Step 4: Rewrite the loop body** — replace the body of `walk_inbox_thread` (the `for msg in &thread.messages { ... }` loop) with:

```rust
    for msg in &thread.messages {
        // Reactions don't carry their own timestamps — the parent message's
        // timestamp drives both decay and the 180d window. reactions[] is the
        // canonical like record and is processed for EVERY message, including
        // the shadow we drop below, so a like is never lost.
        let decayed = decay_weight(msg.timestamp, now, tau_dm);
        let in_180d = within_window(msg.timestamp, now, 180);
        for r in &msg.reactions {
            match r.actor.as_deref() {
                Some(s) if s == me_name => {
                    features.dm_reactions_given += 1;
                    features.dm_reactions_given_decayed += decayed;
                    if in_180d {
                        features.dm_reactions_given_180d += 1;
                    }
                }
                Some(_) => {
                    features.dm_reactions_received += 1;
                    features.dm_reactions_received_decayed += decayed;
                    if in_180d {
                        features.dm_reactions_received_180d += 1;
                    }
                }
                None => {}
            }
        }

        // "Liked a message" is the duplicate shadow of a reaction already
        // counted above — exclude it from message volume, balance, recency,
        // and the real-reply count. See LIKE_SHADOW_CONTENT.
        if is_like_shadow(msg) {
            continue;
        }

        features.dm_messages_total += 1;
        features.dm_messages_total_decayed += decayed;
        match msg.sender.as_deref() {
            Some(s) if s == me_name => acc.outbound += 1,
            Some(_) => {
                acc.inbound += 1;
                features.dm_inbound_replies += 1;
            }
            None => {}
        }
        if let Some(ts) = msg.timestamp {
            acc.latest = Some(match acc.latest {
                Some(prev) if prev >= ts => prev,
                _ => ts,
            });
        }
    }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib aggregate::tests::like_shadows_excluded 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Run the full aggregate test module to catch regressions**

Run: `cargo test --lib aggregate 2>&1 | tail -20`
Expected: PASS (existing DM tests unaffected — fixtures without shadows behave identically).

- [ ] **Step 7: Commit**

```bash
git add src/features/aggregate.rs
git commit -m "fix(features): dedup 'Liked a message' shadows; count real replies

IG double-counts a message like as both a reactions[] entry and a standalone
'Liked a message' message. Exclude the shadow from dm_messages_total / balance
/ recency and track dm_inbound_replies (real replies only); reactions[] stays
the canonical like source. Corrects dm_balance dataset-wide."
```

---

## Task 4: Exercise the dedup through the fixture (E2E count)

The smoke count `total DM messages` (raw, pre-dedup, `lib.rs:342`) legitimately grows when a shadow message is added to the fixture — the shadow IS a line in the file; dedup happens later in aggregation. This pins that the real parse path carries a shadow without crashing.

**Files:**

- Modify: `tests/fixtures/sample_export/your_instagram_activity/messages/inbox/carol_thread/message_1.json`
- Modify: `tests/cli.rs:492`

- [ ] **Step 1: Add a shadow to `carol_thread`** — in `message_1.json`, append one message to the `messages` array (after the existing `"synthetic reply"` object, before the closing `]`):

```json
    ,
    {
      "sender_name": "Carol Synth",
      "timestamp_ms": 1700200120000,
      "content": "Liked a message",
      "reactions": [],
      "is_geoblocked_for_viewer": false,
      "is_unsent_image_by_messenger_kid_parent": false
    }
```

- [ ] **Step 2: Run the fixture-count test to verify it fails**

Run: `cargo test --test cli fixture_counts 2>&1 | tail -20`
Expected: FAIL — `total DM messages: 9` no longer found (raw count is now 10).

- [ ] **Step 3: Update the raw-count assertion** — in `tests/cli.rs:492`:

```rust
        .stdout(contains("total DM messages: 10"))
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --test cli fixture_counts 2>&1 | tail -20`
Expected: PASS. (`DM reactions received total: 1` and `DM-attributed accounts: 1` are unchanged — the shadow has empty `reactions[]`, and carol's deduped `dm_messages_total` stays 2.)

- [ ] **Step 5: Commit**

```bash
git add tests/fixtures/sample_export tests/cli.rs
git commit -m "test(cli): carry a like-shadow through the fixture parse path"
```

---

## Task 5: The tiered effort-skew gate

**Files:**

- Modify: `src/scoring.rs` — add helper fns above `assign_bucket`; insert the two tiers into `assign_bucket`.
- Test: `src/scoring.rs` tests module.

- [ ] **Step 1: Write the failing tests** — add to `src/scoring.rs` tests module. Helper builds a high-scoring, evidence-bearing skewed account:

```rust
    /// A Keep-scoring account with a high-volume, owner-dominated DM thread:
    /// 10 owner messages, 1 real reply → reply_skew (dm_balance) ≈ 0.91,
    /// my_dm_out = 9. Scores into Keep on outbound likes.
    fn skewed_keeper(handle: &str, balance: f32) -> AccountFeatures {
        let mut a = baseline_account(handle);
        a.likes_given_decayed = 5.0; // keep_prob ≈ 0.97
        a.dm_messages_total = 10;
        a.dm_inbound_replies = 1; // my_dm_out = 10 - 1 = 9
        a.dm_balance = Some(balance);
        a
    }

    fn skew_cfg() -> ScoringConfig {
        let mut cfg = baseline_cfg();
        cfg.scoring.effort_skew_min_dm_out = 8;
        cfg.scoring.effort_skew_soft = 0.85;
        cfg.scoring.effort_skew_hard = 0.95;
        cfg
    }

    #[test]
    fn soft_tier_demotes_unmarked_skewed_keeper() {
        let cfg = skew_cfg();
        let acct = skewed_keeper("talker", 0.90); // soft ≤ 0.90 < hard
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert!(scored[0].keep_prob >= cfg.scoring.keep_min, "scores into Keep first");
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn soft_tier_respects_close_friend() {
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("bff", 0.90);
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Keep, "soft_exempt: close friend stays Keep");
    }

    #[test]
    fn soft_tier_demotes_non_deep_mutual() {
        // Mutual is NOT in soft_exempt — a follow-back who never replies in a
        // high-volume thread is the target. (Not deep: mutual_age stays None.)
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("mutual_ghost", 0.90);
        acct.is_mutual = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review);
    }

    #[test]
    fn hard_tier_demotes_close_friend() {
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("ghost_bff", 0.97); // ≥ hard
        acct.is_close_friend = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review, "hard tier overrides close friend");
    }

    #[test]
    fn hard_tier_beats_deep_mutual_floor() {
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("old_ghost", 0.97);
        acct.is_mutual = true;
        acct.mutual_age_days = Some(3000); // would floor to Keep at rung 4
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review, "hard tier sits above deep-mutual floor");
    }

    #[test]
    fn keeplist_survives_both_tiers() {
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("kept", 0.99);
        acct.is_keeplisted = true;
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Keep, "keeplist is explicit intent, never overridden by skew");
    }

    #[test]
    fn evidence_guard_blocks_thin_thread() {
        // reply_skew is extreme but my_dm_out (5) is below min_dm_out (8):
        // no evidence → no demotion.
        let cfg = skew_cfg();
        let mut acct = baseline_account("thin");
        acct.likes_given_decayed = 5.0;
        acct.dm_messages_total = 6;
        acct.dm_inbound_replies = 1; // my_dm_out = 5 < 8
        acct.dm_balance = Some(0.99);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Keep, "below evidence bar → gate cannot fire");
    }

    #[test]
    fn gate_disabled_when_min_is_zero() {
        // The sentinel: min_dm_out == 0 must NOT fire the gate on every thread.
        let cfg = baseline_cfg(); // effort_skew_min_dm_out = 0
        let acct = skewed_keeper("loud", 0.99);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Keep, "min_dm_out=0 disables the gate");
    }

    #[test]
    fn gate_never_yields_unfollow() {
        // Monotonic: even a maximally skewed, low-scoring personal account
        // demotes only to Review via the gate, never Unfollow. (The unfollow
        // path is the separate keep_prob < unfollow_max branch.)
        let cfg = skew_cfg();
        let mut acct = skewed_keeper("x", 0.99);
        acct.likes_given_decayed = 5.0; // keep it in the keep_prob >= keep_min branch
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_ne!(scored[0].bucket, Bucket::Unfollow);
    }

    #[test]
    fn soft_tier_boundary_is_inclusive() {
        let cfg = skew_cfg();
        let acct = skewed_keeper("edge", cfg.scoring.effort_skew_soft as f32);
        let scored = score(std::slice::from_ref(&acct), &cfg);
        assert_eq!(scored[0].bucket, Bucket::Review, "reply_skew == soft must demote (>=)");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib scoring::tests 2>&1 | tail -30`
Expected: FAIL — the new tests get `Keep` where `Review` is expected (gate not implemented).

- [ ] **Step 3: Add helper fns** — in `src/scoring.rs`, immediately above `fn assign_bucket`:

```rust
/// Owner's real outbound message count in resolved 1:1 threads (post-dedup).
/// `dm_messages_total` and `dm_inbound_replies` are both shadow-free, so the
/// difference is the owner's side. `saturating_sub` defends against any future
/// path where the two drift (it cannot underflow today).
fn dm_out(f: &AccountFeatures) -> u32 {
    f.dm_messages_total.saturating_sub(f.dm_inbound_replies)
}

/// The effort-skew gate only acts where IG gives bidirectional evidence: a
/// thread the owner genuinely invested in. `min_dm_out == 0` is the disable
/// sentinel — NOT an evidence bar of zero (which is always satisfied).
fn effort_skew_has_evidence(f: &AccountFeatures, p: &ScoringParams) -> bool {
    p.effort_skew_min_dm_out > 0 && dm_out(f) >= p.effort_skew_min_dm_out
}

/// Reply skew == the post-dedup `dm_balance` (owner messages / total real
/// messages). `None` when the thread has no classifiable messages.
fn reply_skew(f: &AccountFeatures) -> Option<f64> {
    f.dm_balance.map(f64::from)
}

/// SOFT-tier exemptions: stale-able IG keep markers that protect an account
/// from the soft (unmarked-only) tier. `is_mutual` is deliberately excluded —
/// a non-deep mutual who never replies is the target, not an exception.
fn effort_skew_soft_exempt(f: &AccountFeatures) -> bool {
    f.is_close_friend || f.is_favorited || f.account_class != AccountClass::Personal
}
```

- [ ] **Step 4: Insert the tiers into `assign_bucket`** — the function becomes (replacing the existing body):

```rust
fn assign_bucket(f: &AccountFeatures, keep_prob: f64, p: &ScoringParams) -> Bucket {
    if f.is_restricted {
        return Bucket::Review;
    }
    if f.is_droplisted {
        return Bucket::Unfollow;
    }
    // HARD effort-skew tier: extreme owner-dominated one-sidedness overrides
    // stale IG keep markers (close-friend / favorite / mutual) AND the
    // deep-mutual floor below — but never keeplist (explicit intent) or the
    // restricted floor above. Monotonic: Keep/anything → Review only.
    if !f.is_keeplisted
        && effort_skew_has_evidence(f, p)
        && reply_skew(f).is_some_and(|s| s >= p.effort_skew_hard)
    {
        return Bucket::Review;
    }
    if p.deep_mutual_keep_days > 0
        && f.is_mutual
        && f.mutual_age_days
            .is_some_and(|age| age >= p.deep_mutual_keep_days)
    {
        return Bucket::Keep;
    }
    if keep_prob >= p.keep_min {
        // SOFT effort-skew tier: demote an UNMARKED personal Keep that scored
        // on the owner's outbound but draws near-zero real replies back.
        if !f.is_keeplisted
            && !effort_skew_soft_exempt(f)
            && effort_skew_has_evidence(f, p)
            && reply_skew(f).is_some_and(|s| s >= p.effort_skew_soft)
        {
            return Bucket::Review;
        }
        if p.require_reciprocity_for_keep && is_parasocial(f) {
            return Bucket::Review;
        }
        return Bucket::Keep;
    }
    if keep_prob < p.unfollow_max {
        if f.is_close_friend
            || f.is_favorited
            || f.is_keeplisted
            || f.account_class != AccountClass::Personal
        {
            return Bucket::Review;
        }
        return Bucket::Unfollow;
    }
    Bucket::Review
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib scoring::tests 2>&1 | tail -30`
Expected: PASS — new gate tests green AND all pre-existing scoring tests still pass (they use `baseline_cfg` with `min_dm_out = 0`, gate off).

- [ ] **Step 6: Commit**

```bash
git add src/scoring.rs
git commit -m "feat(scoring): two-tier effort-skew gate (Keep->Review, evidence-guarded)"
```

---

## Task 6: CSV columns `reply_skew` + `dm_inbound_replies`

**Files:**

- Modify: `src/output/csv.rs` — `CsvRow` struct + serializer + row build + header test.
- Modify: `tests/cli.rs:623-629` — appended header.
- Modify: `docs/DESIGN.md` — the "Output" header line.

- [ ] **Step 1: Update the header tests first (they fail)** — in `src/output/csv.rs`, `header_matches_design_doc` expected string, append the two columns:

```rust
            "username,display_name,profile_url,bucket,keep_score,dm_msgs,last_dm_days,\
             reactions_given_180d,reactions_received_180d,\
             likes_given_90d,comments_given_90d,follow_tenure_days,\
             account_class,mutual,top_signal,reply_skew,dm_inbound_replies",
```

And in `tests/cli.rs` (the `assert_eq!(header, ...)` at line ~623), the same appended suffix `,reply_skew,dm_inbound_replies`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib output::csv::tests::header 2>&1 | tail -15`
Expected: FAIL — emitted header lacks the two new columns.

- [ ] **Step 3: Add the fields + serializer** — in `src/output/csv.rs`:

Add the `notes` field's siblings at the END of `struct CsvRow` (after `notes`):

```rust
    /// Reply skew == post-dedup `dm_balance`: owner messages / total real
    /// messages in resolved 1:1 threads. `1.0` = owner does all the talking.
    /// Empty when there is no resolvable thread. Decision support — surfaced
    /// on every row with a thread, not only gate-demoted ones.
    #[serde(serialize_with = "fmt_opt_three_decimals")]
    reply_skew: Option<f32>,
    /// The other party's real (non-shadow) message count.
    dm_inbound_replies: u32,
```

Add the serializer next to `fmt_three_decimals`:

```rust
fn fmt_opt_three_decimals<S: Serializer>(v: &Option<f32>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => s.serialize_str(&format!("{x:.3}")),
        None => s.serialize_str(""),
    }
}
```

Populate them in the `CsvRow { ... }` build (after `notes: ...`):

```rust
            reply_skew: s.features.dm_balance,
            dm_inbound_replies: s.features.dm_inbound_replies,
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib output::csv 2>&1 | tail -15`
Expected: PASS. The `option_none_serializes_as_empty_field` test still passes — fields[6]/fields[11] indices are unchanged (columns appended at the end).

- [ ] **Step 5: Update DESIGN.md header contract** — in `docs/DESIGN.md` "## Output", the fenced header line, append `,reply_skew,dm_inbound_replies`, and add a sentence after the `mutual` paragraph:

```markdown
`reply_skew` is the post-dedup `dm_balance` (owner messages ÷ total real
messages in resolved 1:1 DM threads); `1.0` means the owner does all the
talking. `dm_inbound_replies` is the other party's real message count, taps
("Liked a message") excluded. Both are decision support for the effort-skew
gate and appear on every row with a resolvable thread.
```

- [ ] **Step 6: Run the cli header test**

Run: `cargo test --test cli writes_csv_and_markdown 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/output/csv.rs tests/cli.rs docs/DESIGN.md
git commit -m "feat(output): CSV reply_skew + dm_inbound_replies columns"
```

---

## Task 7: Decision-hint arm for the effort-skew shape

The shared `decision_hint` is consumed by both Markdown and HTML, so one arm propagates to both. It uses fixed presentation thresholds (decoupled from live config, same pattern as `LONG_STANDING_MUTUAL_HINT_DAYS`).

**Files:**

- Modify: `src/output/mod.rs` — add consts + arm + test rows.

- [ ] **Step 1: Write the failing test** — add a row to the `decision_hint_precedence_chain` `cases` array (place it right after the `"hide_story beats active DM"` case so precedence is pinned), and one standalone test:

```rust
            Case {
                // Effort-skew shape beats the generic "active DM partner":
                // an owner-dominated thread (skew 0.9, my_dm_out 9) is better
                // characterized as one-sided talking. Sits below hide_story.
                label: "effort-skew beats active DM",
                expected: "you do the talking — they rarely reply",
                mutate: |f| {
                    f.dm_balance = Some(0.9);
                    f.dm_messages_total = 10;
                    f.dm_inbound_replies = 1; // my_dm_out = 9 >= 8
                    f.dm_messages_total_decayed = 5.0; // would otherwise say "active DM partner"
                },
                bucket: Bucket::Review,
            },
```

```rust
    #[test]
    fn effort_skew_hint_needs_volume_and_skew() {
        // Below the presentation volume floor, or below the skew floor, the
        // arm does not fire — falls through to the active-DM / DM-history arms.
        let mut f = baseline();
        f.dm_balance = Some(0.99);
        f.dm_messages_total = 6;
        f.dm_inbound_replies = 1; // my_dm_out = 5 < 8
        f.dm_messages_total_decayed = 5.0;
        assert_eq!(decision_hint(&f, Bucket::Keep), "active DM partner");

        let mut f = baseline();
        f.dm_balance = Some(0.5); // balanced, below skew floor
        f.dm_messages_total = 20;
        f.dm_inbound_replies = 10;
        f.dm_messages_total_decayed = 5.0;
        assert_eq!(decision_hint(&f, Bucket::Keep), "active DM partner");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib output::tests 2>&1 | tail -20`
Expected: FAIL — `effort-skew beats active DM` gets `"active DM partner"`.

- [ ] **Step 3: Add consts** — in `src/output/mod.rs`, after `LONG_STANDING_MUTUAL_HINT_DAYS` (line ~55):

```rust
/// Presentation thresholds for the "owner does the talking" shape in
/// [`decision_hint`]. Fixed shape descriptors, decoupled from the live
/// `effort_skew_*` gate config — same rationale as
/// [`LONG_STANDING_MUTUAL_HINT_DAYS`]: the hint is a true characterization of
/// an owner-dominated thread whether or not the gate is enabled or retuned.
const EFFORT_SKEW_HINT_BALANCE: f32 = 0.85;
const EFFORT_SKEW_HINT_MIN_OUT: u32 = 8;
```

- [ ] **Step 4: Add the arm** — in `decision_hint`, between the `is_hide_story_from` arm and the `dm_messages_total_decayed > 0.0` arm:

```rust
    // Owner-dominated thread: the owner sustained it while the other party
    // rarely replied (taps excluded — dm_inbound_replies counts real messages
    // only). More informative than "active DM partner" for these.
    if f.dm_balance.is_some_and(|b| b >= EFFORT_SKEW_HINT_BALANCE)
        && f.dm_messages_total.saturating_sub(f.dm_inbound_replies) >= EFFORT_SKEW_HINT_MIN_OUT
    {
        return "you do the talking — they rarely reply";
    }
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test --lib output::tests 2>&1 | tail -20`
Expected: PASS (new rows + existing chain all green — `baseline()` has `dm_balance = None`, so existing cases never trip the new arm).

- [ ] **Step 6: Commit**

```bash
git add src/output/mod.rs
git commit -m "feat(output): decision-hint arm for owner-dominated DM threads"
```

---

## Task 8: Owner config + docs

**Files:**

- Modify: `config/scoring.toml` (`[scoring]` section)
- Modify: `docs/DESIGN.md` ("Bucket precedence")
- Modify: `docs/TUNING.md`

- [ ] **Step 1: Turn the gate on in the owner config** — in `config/scoring.toml`, in the `[scoring]` block (after `deep_mutual_keep_days`):

```toml
# Effort-skew gate (docs/specs/2026-05-31-effort-skew-gate-design.md): demote
# Keep -> Review for accounts the owner over-invests in. Guarded on owner DM
# volume so it only acts where IG shows both directions. min_dm_out = 0 disables.
effort_skew_min_dm_out = 8     # owner real messages required as evidence
effort_skew_soft = 0.85        # unmarked personal Keep -> Review at this skew
effort_skew_hard = 0.95        # overrides close-friend/favorite/mutual at this skew
```

- [ ] **Step 2: Verify config still loads + validates**

Run: `cargo run -- check downloaded-ig-data/ 2>&1 | tail -5` (or `cargo test --lib config 2>&1 | tail -5`)
Expected: success / config validates.

- [ ] **Step 3: Update DESIGN.md "Bucket precedence"** — extend the precedence block to show the two new rungs (HARD above deep-mutual, SOFT inside the keep branch), matching `assign_bucket`. Add a short paragraph that the gate is evidence-guarded on `effort_skew_min_dm_out` and is the evidence-based successor to `require_reciprocity_for_keep`. Reference the spec.

- [ ] **Step 4: Add a TUNING.md note** — a short dated entry: gate shipped disabled-by-default in presets, enabled in the owner config at `min_dm_out=8 / soft=0.85 / hard=0.95`; thresholds are starting points pending a labeled calibration round; note that the worked-example account (`my_dm_out=6`) sits below the evidence bar by design. **Privacy:** use a structural descriptor only — never a personal followee handle paired with keep/drop intent (same disclosure as the gitignored `labels.txt`, per CLAUDE.md "Privacy first").

- [ ] **Step 5: Commit**

```bash
git add config/scoring.toml docs/DESIGN.md docs/TUNING.md
git commit -m "feat(config): enable effort-skew gate for owner; document precedence"
```

---

## Task 9: Full verification

- [ ] **Step 1: Format**

Run: `cargo fmt --all`

- [ ] **Step 2: Lint (CI treats warnings as errors)**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: no warnings.

- [ ] **Step 3: Full test suite**

Run: `cargo nextest run 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 4: Real-data sanity check** — confirm the gate moves the expected population and produces no hard label mismatches:

Run: `cargo run --release -- downloaded-ig-data/ --trace <worked-example-handle> 2>&1 | tail -25` (substitute the local handle at run time — do not commit it).
Expected: bucket split shifts some Keep→Review; the worked-example account stays `keep` (my_dm_out=6 < 8) but its `reply_skew` ≈ 0.86 appears in the CSV; `✓ no hard mismatches` still holds. If a hard mismatch appears, STOP — the thresholds need a labeled calibration round, not a code change.

- [ ] **Step 5: Commit any fmt-only changes**

```bash
git add -A
git commit -m "style: cargo fmt" || echo "nothing to format"
```

---

## Self-Review Notes (author)

- **Spec coverage:** Step 1 dedup → Task 3+4. Step 2 reply feature → Task 2+3. Step 3 metric → Task 5 helpers. Step 4 tiered gate → Task 5. Step 5 config → Task 1+8. Step 6 output → Task 6+7. Step 7 testing → folded per task. ✓
- **`reply_skew == dm_balance` (post-dedup):** intentional and DRY — `reply_skew` is not a stored field; the gate and CSV read `dm_balance` (now shadow-free) and derive `dm_out` from `dm_messages_total − dm_inbound_replies`. Only one new stored field (`dm_inbound_replies`).
- **Type consistency:** `effort_skew_min_dm_out: u32`, `effort_skew_soft/hard: f64`, `dm_inbound_replies: u32`, `dm_balance: Option<f32>` throughout. `reply_skew()` returns `Option<f64>`.
- **Decision-hint vs gate thresholds:** intentionally separate (presentation consts vs live config), mirroring the existing `LONG_STANDING_MUTUAL_HINT_DAYS` precedent. A hard-demoted close-friend still shows "marked close friend" (close-friend arm precedes the skew arm) — accepted; the CSV columns carry the numbers.
