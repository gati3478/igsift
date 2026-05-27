//! Output writers — the recommendation artifacts the run materializes.
//!
//! Two files per run, sharing a filename stem:
//!
//! - **CSV** (primary) — one row per account, sortable / filterable /
//!   diffable. Columns per [`docs/DESIGN.md`](../../docs/DESIGN.md)
//!   "Output". Serialized from a `#[derive(Serialize)]` row struct via
//!   the `csv` crate. The `*_90d` / `*_180d` columns are raw fixed-window
//!   counts for human context — distinct from the decay-weighted values
//!   that drive `keep_prob`.
//! - **Markdown** (secondary) — a skim summary: bucket counts plus
//!   top-N unfollow and top-N keep tables, each row carrying the
//!   `dominant_feature` that drove the call. Built for "decide whether
//!   to open the CSV at all".
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
//! directory, with `.csv` and `.md` appended. `--out PATH` overrides;
//! `Path::with_extension` handles either bare-stem (`/tmp/foo`) or
//! extension-bearing (`/tmp/foo.csv`) inputs symmetrically — both yield
//! `/tmp/foo.csv` + `/tmp/foo.md`.

mod csv;
mod markdown;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::scoring::ScoredAccount;

/// Write both the CSV and Markdown artifacts to `<stem>.csv` and
/// `<stem>.md`. Returns the two paths actually written for the caller
/// to surface in the run summary.
pub fn write(scored: &[ScoredAccount], stem: &Path) -> Result<(PathBuf, PathBuf)> {
    let csv_path = stem.with_extension("csv");
    let md_path = stem.with_extension("md");

    let csv_file = File::create(&csv_path)
        .with_context(|| format!("creating CSV at {}", csv_path.display()))?;
    csv::write_to(scored, BufWriter::new(csv_file))
        .with_context(|| format!("writing CSV to {}", csv_path.display()))?;

    let md_file =
        File::create(&md_path).with_context(|| format!("creating MD at {}", md_path.display()))?;
    markdown::write_to(scored, BufWriter::new(md_file))
        .with_context(|| format!("writing MD to {}", md_path.display()))?;

    Ok((csv_path, md_path))
}
