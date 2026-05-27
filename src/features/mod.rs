//! Per-account feature aggregation.
//!
//! Folds the parsed [`crate::export`] signals into one feature record per
//! followed account: DM totals/recency/balance, likes given, comments given,
//! story interactions (in and out), saves, and the account-class hint.
//! Counts are recency-weighted with exponential decay. See
//! [`docs/DESIGN.md`](../../docs/DESIGN.md) for the full feature spec and
//! scoring composition.
//!
//! Status:
//!
//! - [`aggregate`] / [`AccountFeatures`] — slice 7A: handle-keyed booleans,
//!   `follow_tenure_days`, and raw activity counts (likes / comments /
//!   story interactions / stories viewed / saves). DM features and decay
//!   weighting defer to slice 7B.
//! - [`name_resolution`] — bridges DM thread participant **display names**
//!   to **handles** via the seven `label_values` files (`close_friends`,
//!   `profiles_you've_favorited`, etc.) since `following.json` /
//!   `followers_*.json` ship handle-only. Coverage on the 2026-05-11
//!   export: 217/581 (37%) of 1:1 DM threads resolve under the strict
//!   collision policy. Name collisions surface as unresolvable rather
//!   than being guessed.
//!
//! Reciprocity is only partially observable — the export omits
//! likes/comments _others_ made on my posts — so inbound signals (DM
//! `reactions[].actor`, `message_requests/`) stand in as proxies.

pub mod aggregate;
pub mod name_resolution;

pub use aggregate::{AccountClass, AccountFeatures, aggregate};
