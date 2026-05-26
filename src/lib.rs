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

    println!("following count: {}", following.len());
    println!("followers count: {}", followers.len());
    println!("DM thread count: {}", threads.len());
    println!("total DM messages: {total_messages}");

    Ok(())
}
