//! Command-line surface (clap derive).
//!
//! Three entry points share one binary:
//!
//! - Default / `run` — score an export and write the audit. The legacy
//!   form `ig-mgr <export_dir>` is preserved; `ig-mgr run <export_dir>`
//!   is its explicit alias.
//! - `init` — scaffold the per-user `config/` files (keep allowlist,
//!   labels template) for a fresh checkout.
//! - `check <export_dir>` — validate that an export folder is parseable
//!   without scoring it. Fast pre-flight for a freshly-extracted export
//!   or a CI dry-run.
//!
//! `args_conflicts_with_subcommands = true` plus an optional
//! `command` field means the legacy positional + flags continue to
//! work as the implicit Run, and the explicit subcommands only need to
//! be reached for the new entry points.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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
    args_conflicts_with_subcommands = true,
)]
pub struct Cli {
    /// Legacy / default Run args. Used when no subcommand is provided;
    /// equivalent to `ig-mgr run <export_dir>`. clap's
    /// `args_conflicts_with_subcommands` ensures these can't be mixed
    /// with `init` / `check` at the same invocation.
    #[command(flatten)]
    pub run_args: RunArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Path to the unzipped Instagram "Download Your Information" export
    /// (the folder containing `connections/` and `your_instagram_activity/`).
    ///
    /// Optional only because clap requires every flatten-d arg to be
    /// optional when a subcommand could be used instead. In practice
    /// the run path errors if this is missing AND no subcommand is given.
    pub export_dir: Option<PathBuf>,

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

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Score an export and write the audit (explicit form of the
    /// default invocation).
    Run(RunArgs),

    /// Scaffold per-user config files (`config/keep_allowlist.txt`,
    /// `config/labels.txt`) from their checked-in templates.
    Init {
        /// Overwrite existing config files. Default: skip existing.
        #[arg(long)]
        force: bool,
    },

    /// Validate that an export folder is parseable without running
    /// the scorer. Exits non-zero if any source fails to parse.
    Check {
        /// Path to the unzipped Instagram export.
        export_dir: PathBuf,
    },
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

  # Scaffold config/keep_allowlist.txt + config/labels.txt from templates
  ig-mgr init

  # Dry-run: validate the export shape without scoring
  ig-mgr check ./ig-exported-data

The export directory must be the unzipped \"Download Your Information\"
folder — the one containing connections/ and your_instagram_activity/.
";
