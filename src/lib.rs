//! `ig-mgr` — local-first analysis of an Instagram personal data export.
//!
//! The library crate holds the pipeline; the `ig-mgr` binary ([`main`]) is a
//! thin shell that parses arguments and calls [`run`]. Integration tests in
//! `tests/` drive the same code paths.
//!
//! Pipeline shape (see `docs/DESIGN.md` for the full design):
//!
//! ```text
//! export dir ──▶ export::*  (parse JSON)
//!            ──▶ features    (per-account feature aggregation)
//!            ──▶ scoring     (keep-probability + bucketing)
//!            ──▶ output::*   (CSV + Markdown writers)
//! ```
//!
//! Status: scaffold only. No parsing or scoring is implemented yet.

pub mod cli;
pub mod config;
pub mod export;
pub mod features;
pub mod output;
pub mod scoring;

use anyhow::Result;

use crate::cli::Cli;

/// Initialize structured logging. Verbosity comes from `-v`/`-vv` flags, with
/// `RUST_LOG` (if set) taking precedence.
pub fn init_tracing(verbose: u8) {
    use tracing_subscriber::EnvFilter;

    let default_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("ig_mgr={default_level}")));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// Entry point for the analysis run.
///
/// At this stage the pipeline parses relationships and DM threads and prints
/// the four count lines that gate the parser-pass acceptance criteria. The
/// feature aggregation, scoring, and output writers land in later ROADMAP
/// steps.
pub fn run(cli: Cli) -> Result<()> {
    use anyhow::ensure;

    ensure!(
        cli.export_dir.is_dir(),
        "export directory does not exist or is not a directory: {}",
        cli.export_dir.display()
    );

    let following = export::read_following(&cli.export_dir)?;
    let followers = export::read_followers(&cli.export_dir)?;
    let threads = export::read_inbox(&cli.export_dir)?;
    let total_messages: usize = threads.iter().map(|t| t.messages.len()).sum();

    let close_friends = export::read_close_friends(&cli.export_dir)?;
    let favorited = export::read_favorited(&cli.export_dir)?;
    let blocked = export::read_blocked(&cli.export_dir)?;
    let restricted = export::read_restricted(&cli.export_dir)?;
    let hide_story_from = export::read_hide_story_from(&cli.export_dir)?;
    let recently_unfollowed = export::read_recently_unfollowed(&cli.export_dir)?;
    let removed_suggestions = export::read_removed_suggestions(&cli.export_dir)?;
    let message_request_threads = export::read_message_requests(&cli.export_dir)?;

    // `hide_story_from.json` is a single shape-C entry, not an array. With
    // every field carrying `#[serde(default)]`, an empty object `{}` parses
    // successfully — so "the file is shaped right" isn't enough to count
    // someone as a real hide. Treat the entry as real iff it carries at
    // least one label value (the username sits inside that list).
    let hide_story_from_count = usize::from(!hide_story_from.label_values.is_empty());

    println!("following count: {}", following.len());
    println!("followers count: {}", followers.len());
    println!("DM thread count: {}", threads.len());
    println!("total DM messages: {total_messages}");
    println!("close friends count: {}", close_friends.len());
    println!("favorited count: {}", favorited.len());
    println!("blocked count: {}", blocked.len());
    println!("restricted count: {}", restricted.len());
    println!("hide_story_from count: {hide_story_from_count}");
    println!("recently unfollowed count: {}", recently_unfollowed.len());
    println!("removed suggestions count: {}", removed_suggestions.len());
    println!(
        "message request thread count: {}",
        message_request_threads.len()
    );

    Ok(())
}
