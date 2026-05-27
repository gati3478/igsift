//! Output writers — the audit artifacts the run materializes.
//!
//! Three files per run, sharing a filename stem:
//!
//! - **CSV** (data) — one row per account, sortable / filterable /
//!   diffable in a spreadsheet. Columns per
//!   [`docs/DESIGN.md`](../../docs/DESIGN.md) "Output". The `*_90d` /
//!   `*_180d` columns are raw fixed-window counts for human context —
//!   distinct from the decay-weighted values that drive `keep_prob`.
//! - **Markdown** (skim) — a decision-oriented summary: bucket
//!   counts plus per-account cards in `Unfollow` and `Review`, with
//!   the dominant features and a one-line hint. Built for "decide
//!   whether to open the CSV at all".
//! - **HTML** (browse) — self-contained single-file report with
//!   sortable, filterable per-bucket tables. Built for the "I want
//!   to triage this in a browser" workflow; no server, no JS deps,
//!   double-click to open.
//!
//! ## Ordering
//!
//! The CSV is emitted in **ascending `keep_prob` order** (worst-first).
//! That means the rows the user is most likely to act on — the `Unfollow`
//! and low-end `Review` bucket — surface at the top of the file. The
//! user can sort otherwise in their spreadsheet; this is just the
//! "actionable rows first" default.
//!
//! ## Filenames
//!
//! Default stem: `following-audit_<YYYY-MM-DD>` next to the export
//! directory, with `.csv`, `.md`, and `.html` appended. `--out PATH`
//! overrides; `Path::with_extension` handles either bare-stem
//! (`/tmp/foo`) or extension-bearing (`/tmp/foo.csv`) inputs symmetrically.

mod csv;
mod html;
mod markdown;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::scoring::ScoredAccount;

/// Paths the writer produced. Always three: CSV, Markdown, HTML.
#[derive(Debug)]
pub struct WrittenPaths {
    pub csv: PathBuf,
    pub md: PathBuf,
    pub html: PathBuf,
}

/// Write the CSV, Markdown, and HTML artifacts at `<stem>.{csv,md,html}`.
/// Returns the three paths actually written for the caller to surface in
/// the run summary.
pub fn write(scored: &[ScoredAccount], stem: &Path) -> Result<WrittenPaths> {
    let csv_path = stem.with_extension("csv");
    let md_path = stem.with_extension("md");
    let html_path = stem.with_extension("html");

    let csv_file = File::create(&csv_path)
        .with_context(|| format!("creating CSV at {}", csv_path.display()))?;
    csv::write_to(scored, BufWriter::new(csv_file))
        .with_context(|| format!("writing CSV to {}", csv_path.display()))?;

    let md_file =
        File::create(&md_path).with_context(|| format!("creating MD at {}", md_path.display()))?;
    markdown::write_to(scored, BufWriter::new(md_file))
        .with_context(|| format!("writing MD to {}", md_path.display()))?;

    let html_file = File::create(&html_path)
        .with_context(|| format!("creating HTML at {}", html_path.display()))?;
    html::write_to(scored, BufWriter::new(html_file))
        .with_context(|| format!("writing HTML to {}", html_path.display()))?;

    Ok(WrittenPaths {
        csv: csv_path,
        md: md_path,
        html: html_path,
    })
}
