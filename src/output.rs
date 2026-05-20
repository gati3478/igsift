//! Output writers.
//!
//! Two artifacts per run, sharing a filename stem:
//!
//! - **CSV** (primary) — one row per account, sortable/filterable/diffable.
//!   Columns: `username, display_name, bucket, keep_prob, dm_msgs,
//!   last_dm_days, likes_given_90d, comments_given_90d, story_in_180d,
//!   account_class, notes`. Serialized from a `#[derive(Serialize)]` row
//!   struct via the `csv` crate.
//! - **Markdown** (secondary) — a skim summary: top 20 unfollow candidates
//!   and top 20 keepers, each with the dominant feature behind the call.
//!
//! Default filenames: `recommendations_<YYYY-MM-DD>.{csv,md}` next to the
//! export directory, overridable via `--out`.
//!
//! Planned submodules (promote to `output/mod.rs` when they arrive):
//!
//! - `csv` — the CSV row writer.
//! - `markdown` — the summary writer.
//!
//! Status: scaffold only.
