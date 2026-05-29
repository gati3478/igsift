# SPEC — drop-list feature

Status: **proposed** (not yet implemented). Spec only; confirm before code.

## Objective

Give the user an explicit override that forces a hand-flagged account to
`Unfollow`, regardless of score or inferred keep-signals. It is the exact
inverse of the existing `config/keep_allowlist.txt`:

| List                    | File                        | Effect                                       |
| ----------------------- | --------------------------- | -------------------------------------------- |
| keep-allowlist (exists) | `config/keep_allowlist.txt` | floors `Unfollow → Review` (never auto-drop) |
| **drop-list (new)**     | `config/drop_list.txt`      | forces `→ Unfollow` (never auto-keep)        |

**Why it exists** (see `docs/TUNING.md` round 5): keep/drop intent is not
separable on the current features, so weight-tuning trades keep-recall for
drop-precision ~1:1. The two failure modes left after tuning — the hard
mismatch (a story-heavy drop-intent account scored into `keep`) and the
low-engagement keeps — are structural. The keep-allowlist already handles
the second; the drop-list handles the first. This closes the loop without
chasing labels.

**Target user:** the single local user (Gati) running the one-shot audit;
maintains the file by hand, same as labels and keep-allowlist.

**Non-goal:** automated unfollow. The drop-list changes a _recommendation_;
the user still acts manually inside Instagram. No network, no API.

## Behavior / acceptance criteria

The drop-list slots into `scoring::assign_bucket` as a new precedence rung.

**Bucket precedence (new, top wins):**

```
1. is_restricted      → Review     (unchanged hard floor; beats drop-list)
2. is_drop_listed     → Unfollow   (NEW: beats keep_min + all keep-gates)
3. keep_prob >= keep_min → Keep
4. keep_prob <  unfollow_max:
       close_friend | favorited | keep_allowlisted | non-Personal → Review
       else → Unfollow
5. otherwise          → Review
```

**Cross-list conflict:** a handle present in _both_ `keep_allowlist.txt`
and `drop_list.txt` is a contradiction. Fail loudly at load (in `run`),
naming the offending handle(s), before any scoring — mirroring the
duplicate-handle errors in `labels::parse` / `allowlist::parse`. Because
this fails first, `assign_bucket` never sees a both-listed handle, so
precedence between drop-list and keep-allowlist is moot by construction.

**Acceptance tests (all must pass):**

1. A drop-listed handle with `is_close_friend = true` and `keep_prob ≈ 1.0`
   → `Bucket::Unfollow`.
2. A drop-listed handle with `account_class = Brand` (would normally floor
   at Review) → `Unfollow`.
3. A drop-listed **and** `is_restricted` account → `Review` (restricted
   wins; pin this — it's the one exception).
4. A handle on both lists → `run` returns `Err`, message names the handle
   and both files.
5. Missing `config/drop_list.txt` → empty set, zero behavior change
   (the list is opt-in, like labels/allowlist).
6. Format errors (multi-token bare line) → hard parse error, reusing the
   shared parser's existing rules.
7. `decision_hint` for a drop-listed account → `"explicit drop-list"`.
8. A non-followee handle in `drop_list.txt` → silently ignored (no row),
   same as keep-allowlist non-followees.

## Commands

No new subcommand. Surfaces through existing flows:

- `ig-mgr init` — scaffolds `config/drop_list.txt` from the embedded
  `config/drop_list.txt.example` template (add to the `targets` array in
  `lib::init`).
- `ig-mgr run <export>` — loads the drop-list, enforces the cross-list
  conflict check, applies the gate. No new flags.
- `ig-mgr check <export>` — unchanged (parser-only; does not score or load
  handle lists). _Optional:_ surface the cross-list conflict here too as a
  config sanity check — deferred unless wanted.

## Project structure (files to create / modify)

**Create:**

- `config/drop_list.txt.example` — template mirroring
  `config/keep_allowlist.txt.example` (one handle per line, `#` comments;
  copy explains it forces Unfollow and that double-listing errors).

**Modify:**

- `src/allowlist.rs` — reuse the existing generic `parse(body, source)`;
  add `pub fn load_drop_list() -> Result<HashSet<String>>` (reads
  `config/drop_list.txt`, mirrors `load_default`). Update the module doc to
  say it now loads both per-user handle lists. _(Alternative: rename the
  module to `handle_lists` with `load_keep_allowlist` / `load_drop_list`.
  Deferred — more churn, same behavior. Recommendation: keep `allowlist.rs`,
  add the one function.)_
- `src/features/account_class.rs` — `Classifier` gains a
  `drop_list: HashSet<String>` field + `pub fn is_drop_listed(&self, &str)
-> bool` (case-insensitive, mirror of `is_allowlisted`). `Classifier::new`
  signature becomes `new(keep_allowlist, drop_list)`.
- `src/features/aggregate.rs` — `AccountFeatures` gains
  `pub is_drop_listed: bool`, populated in `aggregate` via
  `inputs.classifier.is_drop_listed(handle)` (mirror of `is_keep_allowlisted`
  on the adjacent line).
- `src/scoring.rs` — insert the rung-2 check in `assign_bucket` (above the
  `keep_min` check, below the `is_restricted` floor).
- `src/output/mod.rs` — `decision_hint` gains a top rule:
  `if f.is_drop_listed { return "explicit drop-list"; }` placed first
  (most decisive; can't co-occur with allowlist). Extend the table-driven
  precedence test.
- `src/lib.rs` — in `run`: load drop-list alongside the keep-allowlist,
  run the disjointness check (a small `ensure_disjoint(&keep, &drop)`
  helper — natural home is `allowlist.rs`), pass both into
  `Classifier::new`. Add a `-v` smoke line (`drop-list size on disk`).
- `.gitignore` — add `/config/drop_list.txt` (per-user data, never
  committed; the `.example` is the committed template).
- `CLAUDE.md` — Layout (new file), Conventions (drop-list bullet next to
  the keep-allowlist note), and the bucket-line section (drop-list is now
  the implemented fix for the hard mismatch).
- `docs/DESIGN.md` — document the new precedence rung in "Buckets".

**Known mechanical cost:** adding `is_drop_listed` to `AccountFeatures`
touches **every** struct literal — the builders in the `#[cfg(test)] mod
tests` of `scoring.rs`, `csv.rs`, `html.rs`, `markdown.rs`, `output/mod.rs`,
`aggregate.rs`, and `labels.rs`. Each needs `is_drop_listed: false` added.
This is expected, not scope creep.

## Code style

Match the existing allowlist machinery exactly — this feature is a mirror,
not a new pattern:

- `anyhow::Result` with `.context(...)`; loud errors naming the file +
  handle (reuse `bail!` style from `allowlist::parse`).
- Case-insensitive membership via pre-lowercased `HashSet<String>` (the
  `is_allowlisted` pattern).
- Tunables/data in files, not code. No new dependency.
- `#[serde(default)]` posture is irrelevant here (plain text file, not JSON).
- Comments only where the WHY is non-obvious (the restricted-wins exception
  and the cross-list error are the two worth a line).
- `unsafe` remains forbidden.

## Testing strategy

Integration-first, mirroring the existing coverage:

- **Unit (`account_class.rs`)**: `is_drop_listed` case-insensitive;
  drop-list membership does not change `account_class`.
- **Unit (`scoring.rs`)**: the 3 precedence cases (drop beats keep-signals;
  drop beats brand-gate; restricted beats drop). Use the existing
  `baseline_account` + `baseline_cfg` helpers.
- **Unit (`allowlist.rs`)**: `load_drop_list` reuses `parse`, so the format
  tests already cover it; add one `ensure_disjoint` test (overlap → Err
  naming the handle; disjoint → Ok).
- **Unit (`output/mod.rs`)**: extend the `decision_hint` precedence table
  with the `"explicit drop-list"` row at the top.
- **Unit (`aggregate.rs`)**: a drop-listed followee surfaces
  `is_drop_listed = true`; a non-followee drop entry creates no row.
- **Integration (`tests/cli.rs`)**: the `ig_mgr()` helper already runs with
  `cwd = temp_dir` so the repo's per-user files don't leak — keep that. Add
  a fixture-scoped case only if a synthetic `drop_list.txt` can be injected
  via the existing harness; otherwise the unit gate tests are sufficient.
- **Mutation**: the new `assign_bucket` rung and `is_drop_listed` should be
  caught by the precedence unit tests; re-run `cargo mutants` after, expect
  0 new unjustified survivors (update `.cargo/mutants.toml` only for proven-
  inert cases).

Bars that stay green: `cargo build --all-targets`, `cargo clippy
--all-targets -- -D warnings`, `cargo nextest run`, `cargo fmt --all`.

## Boundaries

**Always**

- Treat `config/drop_list.txt` as per-user private data — gitignored,
  never committed; only the `.example` template ships.
- Fail loudly on a both-lists conflict before scoring.
- Keep `is_restricted` as a Review floor above the drop-list.
- Use structural descriptors (not handle + intent) in any committed doc.

**Ask first**

- Adding a CSV column for drop-list status (changes the DESIGN.md "Output"
  header contract — default: do **not**; the `Unfollow` bucket + the
  `"explicit drop-list"` hint already convey it).
- Renaming `allowlist.rs` → `handle_lists.rs`.
- Surfacing the conflict check inside `check`.

**Never**

- Auto-unfollow or touch Instagram. The drop-list only changes the audit
  recommendation.
- Let the drop-list override `is_restricted`.
- Silently resolve a both-lists conflict (must error).
- Add a dependency for this.
