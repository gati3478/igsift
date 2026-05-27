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
#[command(
    name = "ig-mgr",
    version,
    about,
    long_about = None,
    after_help = EXAMPLES,
    after_long_help = EXAMPLES,
)]
pub struct Cli {
    /// Path to the unzipped Instagram "Download Your Information" export
    /// (the folder containing `connections/` and `your_instagram_activity/`).
    pub export_dir: PathBuf,

    /// Where to write the audit. The CSV and Markdown outputs share
    /// this stem. Defaults to `following-audit_<DATE>.{csv,md}` next to the
    /// export directory.
    #[arg(short, long, value_name = "PATH")]
    pub out: Option<PathBuf>,

    /// Scoring configuration (feature weights and decay constants). When
    /// omitted, the path is resolved (`./config/scoring.toml` in the cwd →
    /// compiled-in default) — see [`crate::config`]. A platform config dir
    /// is in the comments of `config.rs` but not yet wired.
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Increase log verbosity (`-v` for debug, `-vv` for trace). `RUST_LOG`
    /// overrides this when set.
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Print the full per-term scoring breakdown for one handle. The handle
    /// must be in the followings set (after blocked/recently-unfollowed
    /// exclusions); otherwise the run errors. Intended for weight tuning —
    /// answers "why did this account rank where it did?" without grepping
    /// scoring code.
    #[arg(long, value_name = "HANDLE")]
    pub trace: Option<String>,
}

/// Worked-example block appended to `--help` and `--help` (long). Kept
/// short and copy-pasteable; the README has the longer narrative.
const EXAMPLES: &str = "\
EXAMPLES:
  # Basic run — writes following-audit_<DATE>.{csv,md} next to the export
  ig-mgr ./ig-exported-data

  # Custom output stem (writes /tmp/audit.csv + /tmp/audit.md)
  ig-mgr ./ig-exported-data --out /tmp/audit

  # Explain why one account landed where it did
  ig-mgr ./ig-exported-data --trace some_handle

  # Debug verbosity (or use RUST_LOG=ig_mgr=debug to override)
  ig-mgr ./ig-exported-data -v

The export directory must be the unzipped \"Download Your Information\"
folder — the one containing connections/ and your_instagram_activity/.
";
