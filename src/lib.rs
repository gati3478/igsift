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
/// Currently a scaffold: it validates that the export directory exists and
/// reports that the pipeline is not yet implemented. Future sessions wire in
/// [`export`], [`features`], [`scoring`], and [`output`].
pub fn run(cli: Cli) -> Result<()> {
    use anyhow::ensure;

    ensure!(
        cli.export_dir.is_dir(),
        "export directory does not exist or is not a directory: {}",
        cli.export_dir.display()
    );

    tracing::warn!("ig-mgr scaffold: analysis pipeline is not implemented yet");
    println!(
        "ig-mgr {} — scaffold only.\n  export dir : {}\n  config     : {}\n  output     : {}",
        env!("CARGO_PKG_VERSION"),
        cli.export_dir.display(),
        cli.config
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<resolved default>".to_string()),
        cli.out
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<next to export dir>".to_string()),
    );

    Ok(())
}
