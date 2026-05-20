//! Per-account feature aggregation.
//!
//! Folds the parsed [`crate::export`] signals into one feature record per
//! followed account: DM totals/recency/balance, likes given, comments given,
//! story interactions (in and out), tags, saves, searches, and the
//! account-class hint. Counts are recency-weighted with exponential decay.
//!
//! Reciprocity is only partially observable — the export omits likes/comments
//! *others* made on my posts — so inbound signals (story reactions to me, DM
//! direction balance, tag-backs) stand in as proxies. See `docs/DESIGN.md`.
//!
//! Status: scaffold only.
