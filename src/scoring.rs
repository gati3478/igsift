//! Scoring and bucketing.
//!
//! Combines a feature record (from [`crate::features`]) with the weights in
//! [`crate::config`] into `keep_probability ∈ [0, 1]` via a sigmoid, then
//! assigns a bucket:
//!
//! - `keep` — `keep_prob >= 0.7`, or a hard-boost account (close friend).
//! - `review` — `0.3 <= keep_prob < 0.7`, or any public-figure / brand.
//! - `unfollow` — `keep_prob < 0.3` AND `account_class = personal` AND not a
//!   close friend.
//!
//! Scoring is embarrassingly parallel across accounts — a `rayon` `par_iter`
//! over feature records when this is implemented.
//!
//! Status: scaffold only.
