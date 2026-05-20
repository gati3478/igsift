//! Command-line surface (clap derive).
//!
//! The CLI is intentionally small: one positional input (the export folder),
//! an output override, a scoring-config path, and verbosity. A future TUI
//! review subcommand (`ig-mgr review`) is an out-of-scope v2 idea — keep this
//! struct flat until that lands.

use std::path::PathBuf;

use clap::Parser;

/// Score Instagram followings from a personal data export and rank who to
/// unfollow vs. keep.
#[derive(Debug, Parser)]
#[command(name = "ig-mgr", version, about, long_about = None)]
pub struct Cli {
    /// Path to the unzipped Instagram "Download Your Information" export
    /// (the folder containing `connections/` and `your_instagram_activity/`).
    pub export_dir: PathBuf,

    /// Where to write the recommendations. The CSV and Markdown outputs share
    /// this stem. Defaults to `recommendations_<DATE>.{csv,md}` next to the
    /// export directory.
    #[arg(short, long, value_name = "PATH")]
    pub out: Option<PathBuf>,

    /// Scoring configuration (feature weights and decay constants).
    #[arg(
        short,
        long,
        value_name = "PATH",
        default_value = "config/scoring.toml"
    )]
    pub config: PathBuf,

    /// Increase log verbosity (`-v` for debug, `-vv` for trace). `RUST_LOG`
    /// overrides this when set.
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}
