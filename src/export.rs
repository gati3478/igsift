//! Parsers for the Instagram personal data export (JSON).
//!
//! Every path and JSON key the parsers depend on is **presumed stale** until
//! re-verified against a freshly downloaded export — Instagram silently rotates
//! the schema. See `docs/DESIGN.md` ("Inputs") and `ROADMAP.md`.
//!
//! Robustness approach (when implemented):
//!
//! - `serde` structs with `#[serde(default)]` + `Option<T>` so missing keys
//!   degrade gracefully rather than aborting the whole run.
//! - `serde_path_to_error` to report *which* key failed when a file does not
//!   match the expected shape.
//!
//! Planned submodules (promote this file to `export/mod.rs` when they arrive):
//!
//! - `relationships` — followers, following, close friends, recently unfollowed.
//! - `messages` — DM threads: volume, recency, per-direction counts.
//! - `likes` — liked posts and liked comments.
//! - `comments` — post and reels comments.
//! - `stories` — story interactions, inbound and outbound.
//! - `content` — posts/reels/stories I authored (tags & mentions).
//! - `saved` — saved posts.
//! - `searches` — profile searches (latent-interest signal).
//!
//! Status: scaffold only.
