//! Detect and extract Instagram export archives.
//!
//! Instagram's "Download Your Information" ships as one of:
//! - a directory the user already extracted (`connections/` at the top)
//! - a single `.zip` archive
//! - a set of multipart `.zip` files (large exports are split by SIZE
//!   when downloaded — confirmed against the 8.4 GB / 4-part example
//!   in the user memory)
//!
//! Per the chunking note ([[project_ig_export_chunking]] memory),
//! multipart chunks overlap on path prefixes (the same thread folder
//! shows up in multiple parts) but the actual files inside each
//! overlap are disjoint, so a flat sequential extract into one cache
//! directory merges cleanly with no special handling — last-write-wins
//! on path collisions is safe.
//!
//! Cache layout: hidden directory next to the input. A `.complete`
//! marker file pins the last successful extract; the cache is reused
//! when the marker is at least as fresh as every source zip.
//! `rebuild = true` forces a clean re-extract.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use zip::ZipArchive;

const CACHE_MARKER: &str = ".complete";

/// Marker path that indicates "this directory is an extracted IG
/// export". Picked to overlap with [`crate::export::validate_shape`]'s
/// most-load-bearing check so the two stay aligned.
const EXTRACTED_MARKER: &str = "connections/followers_and_following/following.json";

/// Resolve an input path to an extracted-export directory.
///
/// - A directory that already contains [`EXTRACTED_MARKER`] → returned
///   as-is.
/// - A directory containing one or more `*.zip` files → all extracted
///   to `<dir>/.igsift-extracted/`.
/// - A single `.zip` file → extracted to
///   `<parent>/.igsift-extracted-<stem>/`.
/// - Anything else → returned as-is so the caller's existing
///   `validate_shape` can surface a precise diagnosis.
///
/// `progress_enabled` matches the run-level flag — disabled when the
/// user passed `-v` or stderr isn't a TTY.
pub fn resolve(input: &Path, rebuild: bool, progress_enabled: bool) -> Result<PathBuf> {
    if input.is_dir() {
        if input.join(EXTRACTED_MARKER).is_file() {
            return Ok(input.to_path_buf());
        }
        let zips = find_zip_parts(input)?;
        if !zips.is_empty() {
            let cache = input.join(".igsift-extracted");
            return extract_or_reuse(&zips, &cache, rebuild, progress_enabled);
        }
        // No marker, no zips — fall through and let validate_shape
        // produce the "missing X, Y, Z" diagnosis.
        return Ok(input.to_path_buf());
    }
    if input.is_file() && is_zip(input) {
        let parent = input.parent().unwrap_or_else(|| Path::new("."));
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("export");
        let cache = parent.join(format!(".igsift-extracted-{stem}"));
        return extract_or_reuse(&[input.to_path_buf()], &cache, rebuild, progress_enabled);
    }
    Ok(input.to_path_buf())
}

fn is_zip(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn find_zip_parts(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut zips: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_zip(p))
        .collect();
    // Sort by filename. Instagram ships parts as
    // `instagram-<handle>-YYYY-MM-DD-<hash>-N.zip` so lexicographic
    // order matches part order; if a future export uses different
    // naming, last-write-wins on overlapping files still merges
    // correctly per the chunking memory.
    zips.sort();
    Ok(zips)
}

fn extract_or_reuse(
    zips: &[PathBuf],
    cache: &Path,
    rebuild: bool,
    progress_enabled: bool,
) -> Result<PathBuf> {
    if rebuild && cache.exists() {
        fs::remove_dir_all(cache).context("removing cache for --rebuild-cache")?;
    }
    if cache_is_fresh(cache, zips)? {
        return Ok(extracted_root(cache));
    }
    if cache.exists() {
        fs::remove_dir_all(cache).context("removing stale cache")?;
    }
    fs::create_dir_all(cache)
        .with_context(|| format!("creating cache directory {}", cache.display()))?;
    extract_zips_with_progress(zips, cache, progress_enabled)?;
    fs::write(cache.join(CACHE_MARKER), marker_body(zips)?).context("writing cache marker")?;
    Ok(extracted_root(cache))
}

/// Serialized cache fingerprint: `{count}\n{total_compressed_bytes}\n`.
///
/// Stored in `.complete` and re-derived from the current zip set on
/// every resolve. Mtime alone is insufficient — `cp -p`, `rsync
/// --times`, and remounted NFS volumes all preserve mtimes through
/// content replacement, leaving the cache "fresh" with stale
/// extracted data. Also catches deleted parts (count drops) and
/// added parts (count rises) without any mtime change.
fn marker_body(zips: &[PathBuf]) -> Result<String> {
    let mut total: u64 = 0;
    for z in zips {
        total = total.saturating_add(fs::metadata(z)?.len());
    }
    Ok(format!("{}\n{total}\n", zips.len()))
}

fn cache_is_fresh(cache: &Path, zips: &[PathBuf]) -> Result<bool> {
    let marker = cache.join(CACHE_MARKER);
    if !marker.is_file() {
        return Ok(false);
    }
    // Content fingerprint takes precedence over mtime — a same-mtime
    // replacement still changes size, which we detect.
    let recorded = fs::read_to_string(&marker).context("reading cache marker")?;
    if recorded != marker_body(zips)? {
        return Ok(false);
    }
    Ok(true)
}

/// After extraction, find the directory that actually contains the
/// IG export tree. Instagram's zips ship with a single top-level
/// wrapper directory (`instagram-<handle>-YYYY-...`); without this
/// descent the cache root would have one subdir and `validate_shape`
/// would fail with "missing connections/…" even though the data is
/// there one level down.
fn extracted_root(cache: &Path) -> PathBuf {
    if cache.join(EXTRACTED_MARKER).is_file() {
        return cache.to_path_buf();
    }
    if let Ok(entries) = fs::read_dir(cache) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join(EXTRACTED_MARKER).is_file() {
                return p;
            }
        }
    }
    // No descent helps — return the cache root and let validate_shape
    // diagnose. The bail is delayed by one function call but the user
    // gets the missing-paths list instead of a generic archive error.
    cache.to_path_buf()
}

fn extract_zips_with_progress(
    zips: &[PathBuf],
    cache: &Path,
    progress_enabled: bool,
) -> Result<()> {
    let total_bytes: u64 = zips
        .iter()
        .map(|z| fs::metadata(z).map(|m| m.len()).unwrap_or(0))
        .sum();
    let target = if progress_enabled {
        ProgressDrawTarget::stderr()
    } else {
        ProgressDrawTarget::hidden()
    };
    let bar = ProgressBar::with_draw_target(Some(total_bytes), target);
    bar.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} extracting [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}",
        )
        .expect("static template is valid")
        .progress_chars("##-"),
    );

    for zip_path in zips {
        let zip_size = fs::metadata(zip_path)?.len();
        let label = zip_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("zip");
        bar.set_message(label.to_owned());
        extract_one(zip_path, cache)
            .with_context(|| format!("extracting {}", zip_path.display()))?;
        bar.inc(zip_size);
    }
    bar.finish_and_clear();
    Ok(())
}

fn extract_one(zip_path: &Path, cache: &Path) -> Result<()> {
    let file =
        fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive =
        ZipArchive::new(file).with_context(|| format!("opening zip {}", zip_path.display()))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // `enclosed_name()` rejects entries that would escape the
        // extraction directory via `../` or absolute paths — the
        // zip-slip guard. Skipping the entry is the safe fail mode.
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let outpath = cache.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&outpath)
                .with_context(|| format!("creating {}", outpath.display()))?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let mut out = fs::File::create(&outpath)
                .with_context(|| format!("creating {}", outpath.display()))?;
            io::copy(&mut entry, &mut out)
                .with_context(|| format!("writing {}", outpath.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_export")
    }

    /// Zip the sanitized fixture into a temp file and return its path.
    /// Mirrors the IG export shape: a single top-level wrapper folder
    /// inside the zip so we exercise the [`extracted_root`] descent.
    fn synth_zip(test_id: &str, with_wrapper: bool) -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("igsift-archive-{test_id}.zip"));
        let _ = fs::remove_file(&tmp);
        let file = fs::File::create(&tmp).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        let root = fixture_root();
        let wrapper = if with_wrapper {
            "instagram-test-2026-05-11-abc/"
        } else {
            ""
        };
        write_dir_to_zip(&mut writer, &root, &root, wrapper, options);
        writer.finish().expect("finalize zip");
        tmp
    }

    fn write_dir_to_zip(
        writer: &mut zip::ZipWriter<fs::File>,
        base: &Path,
        dir: &Path,
        prefix: &str,
        options: zip::write::SimpleFileOptions,
    ) {
        for entry in fs::read_dir(dir).expect("read fixture dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let rel = path.strip_prefix(base).expect("strip base");
            let zip_path = format!("{prefix}{}", rel.to_string_lossy());
            if path.is_dir() {
                writer.add_directory(&zip_path, options).expect("add dir");
                write_dir_to_zip(writer, base, &path, prefix, options);
            } else {
                writer.start_file(&zip_path, options).expect("start file");
                let bytes = fs::read(&path).expect("read file");
                writer.write_all(&bytes).expect("write zip entry");
            }
        }
    }

    #[test]
    fn already_extracted_dir_passes_through() {
        let resolved = resolve(&fixture_root(), false, false).expect("resolve");
        assert_eq!(resolved, fixture_root());
    }

    #[test]
    fn single_zip_extracts_and_descends_wrapper() {
        let zip_path = synth_zip("single", true);
        let resolved = resolve(&zip_path, true, false).expect("resolve");
        // Auto-descent: resolved should point inside the wrapper folder,
        // not at the cache root. validate_shape's marker must be findable.
        assert!(
            resolved.join(EXTRACTED_MARKER).is_file(),
            "expected wrapper-descent to find marker at {}",
            resolved.display(),
        );
        let _ = fs::remove_dir_all(zip_path.parent().unwrap().join(format!(
            ".igsift-extracted-{}",
            zip_path.file_stem().unwrap().to_string_lossy()
        )));
        let _ = fs::remove_file(&zip_path);
    }

    #[test]
    fn single_zip_without_wrapper_extracts_to_cache_root() {
        let zip_path = synth_zip("flat", false);
        let resolved = resolve(&zip_path, true, false).expect("resolve");
        assert!(resolved.join(EXTRACTED_MARKER).is_file());
        let _ = fs::remove_dir_all(zip_path.parent().unwrap().join(format!(
            ".igsift-extracted-{}",
            zip_path.file_stem().unwrap().to_string_lossy()
        )));
        let _ = fs::remove_file(&zip_path);
    }

    #[test]
    fn cache_is_reused_on_second_resolve() {
        let zip_path = synth_zip("cache-reuse", false);
        let first = resolve(&zip_path, true, false).expect("first resolve");
        // Touch a sentinel file inside the cache root that won't be
        // re-written if cache reuse fires. If the cache were rebuilt,
        // the sentinel disappears (re-extract clears the cache).
        let sentinel = first.join(".sentinel");
        fs::write(&sentinel, "x").expect("sentinel");
        let second = resolve(&zip_path, false, false).expect("second resolve");
        assert_eq!(first, second);
        assert!(
            sentinel.is_file(),
            "cache must not be rebuilt on warm resolve"
        );

        // Now force rebuild — sentinel must be gone.
        let _ = resolve(&zip_path, true, false).expect("rebuild resolve");
        assert!(
            !sentinel.is_file(),
            "rebuild=true must clear the cache before extracting"
        );

        let _ = fs::remove_dir_all(zip_path.parent().unwrap().join(format!(
            ".igsift-extracted-{}",
            zip_path.file_stem().unwrap().to_string_lossy()
        )));
        let _ = fs::remove_file(&zip_path);
    }

    #[test]
    fn cache_invalidates_when_zip_size_changes_without_mtime_change() {
        // Simulates `cp -p` / `rsync --times` content replacement —
        // same mtime, different size. mtime-only invalidation would
        // miss this; content fingerprint catches it.
        let zip_path = synth_zip("cache-resize", false);
        let first = resolve(&zip_path, true, false).expect("first resolve");
        let sentinel = first.join(".sentinel");
        fs::write(&sentinel, "x").expect("sentinel");

        // Rebuild the zip into the SAME path so the mtime resets to
        // "now" (close to the marker's). Then synth_zip is called a
        // second time to a different path and the bytes are copied
        // over, preserving content-change-with-fresh-mtime. The
        // marker's recorded size differs from the new zip's size →
        // invalidation fires.
        let bigger_zip = synth_zip("cache-resize-bigger", true);
        fs::copy(&bigger_zip, &zip_path).expect("overwrite");
        let _ = fs::remove_file(&bigger_zip);

        let _ = resolve(&zip_path, false, false).expect("resolve after size change");
        assert!(
            !sentinel.is_file(),
            "cache must be invalidated when source zip size changes",
        );

        let parent = zip_path.parent().unwrap();
        let stem = zip_path.file_stem().unwrap().to_string_lossy();
        let _ = fs::remove_dir_all(parent.join(format!(".igsift-extracted-{stem}")));
        let _ = fs::remove_file(&zip_path);
        let _ =
            fs::remove_dir_all(parent.join(format!(".igsift-extracted-{}", "cache-resize-bigger")));
    }

    #[test]
    fn multipart_zips_merge_into_one_extracted_tree() {
        // The headline feature: IG ships large exports as multipart
        // zips that overlap on path prefixes (same thread folder
        // across parts) but contain disjoint files inside. The
        // resolver concatenates them into one cache dir with
        // last-write-wins safe per the chunking memory. This test
        // builds two synthetic parts with overlapping folders +
        // disjoint files, runs resolve(), and asserts the union is
        // present.

        let dir = std::env::temp_dir().join(format!(
            "igsift-multipart-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond(),
        ));
        fs::create_dir_all(&dir).expect("mktemp");

        // Part 1: half the fixture
        let part1 = dir.join("export-part-1.zip");
        write_partial_zip(&part1, &["connections", "personal_information"]);
        // Part 2: the other half (your_instagram_activity — required
        // by validate_shape as the third marker)
        let part2 = dir.join("export-part-2.zip");
        write_partial_zip(&part2, &["your_instagram_activity"]);

        let resolved = resolve(&dir, true, false).expect("resolve multipart");

        // Auto-descent + cache merge: both halves must be present
        // under one root.
        assert!(
            resolved.join(EXTRACTED_MARKER).is_file(),
            "part 1 contribution missing: {}",
            resolved.display(),
        );
        assert!(
            resolved.join("your_instagram_activity").is_dir(),
            "part 2 contribution missing: {}",
            resolved.display(),
        );
        assert!(
            resolved
                .join("personal_information/personal_information/personal_information.json")
                .is_file(),
            "part 1 personal_information contribution missing",
        );

        // Re-run with a new part added — count changes, cache must
        // invalidate.
        let part3 = dir.join("export-part-3.zip");
        write_partial_zip(&part3, &["connections"]);
        let sentinel = resolved.join(".sentinel");
        fs::write(&sentinel, "x").expect("sentinel");
        let _ = resolve(&dir, false, false).expect("resolve with new part");
        assert!(
            !sentinel.is_file(),
            "adding a new zip part must invalidate the cache",
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Write a zip containing only the named top-level dirs from the
    /// fixture. Used to synthesize multipart parts where each carries
    /// a disjoint subtree of the export.
    fn write_partial_zip(zip_path: &Path, top_dirs: &[&str]) {
        use std::io::Write as _;
        let file = fs::File::create(zip_path).expect("create part zip");
        let mut writer = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let root = fixture_root();
        for top in top_dirs {
            let dir = root.join(top);
            if dir.is_dir() {
                write_dir_to_zip(&mut writer, &root, &dir, "", opts);
            }
        }
        // Ensure tests don't depend on Drop ordering of writer.
        writer.finish().expect("finalize part zip");
        // Silence unused-import warning under test.
        let _ = std::io::stderr().flush();
    }

    #[test]
    fn marker_body_is_count_plus_total_bytes() {
        // Pin the on-disk format so a future serialization change is
        // explicit (and not a silent cache-incompatibility bump).
        let zip_path = synth_zip("marker-format", false);
        let body = marker_body(std::slice::from_ref(&zip_path)).expect("marker");
        let expected_size = fs::metadata(&zip_path).unwrap().len();
        assert_eq!(body, format!("1\n{expected_size}\n"));
        let _ = fs::remove_file(&zip_path);
    }

    #[test]
    fn zip_slip_entries_are_rejected_benign_still_extracts() {
        // The `enclosed_name()` guard is the only thing between an
        // untrusted third-party export and arbitrary filesystem writes.
        // It is correct today, but a refactor to the traversal-vulnerable
        // `name()` / `mangled_name()` APIs would silently reintroduce
        // zip-slip with a green suite. Build a hostile zip carrying a
        // `../` traversal entry and an absolute-path entry, extract it,
        // and prove neither escapes the cache dir while a benign sibling
        // still lands. This test FAILS if the guard is weakened.
        let dir = std::env::temp_dir().join(format!(
            "igsift-zipslip-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond(),
        ));
        let cache = dir.join("cache");
        fs::create_dir_all(&cache).expect("mktemp cache");

        // Absolute-path entry target, kept unique to avoid cross-run
        // collisions; if the guard fails, the file appears here.
        let abs_escape = dir.join("abs-escape-target.txt");

        let zip_path = dir.join("hostile.zip");
        {
            let file = fs::File::create(&zip_path).expect("create hostile zip");
            let mut writer = zip::ZipWriter::new(file);
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            // `../escape-rel.txt` would land in `dir/` (cache's parent)
            // if the entry name were joined onto the cache dir raw.
            writer
                .start_file("../escape-rel.txt", opts)
                .expect("rel traversal entry");
            writer.write_all(b"pwned").expect("write rel");
            writer
                .start_file(abs_escape.to_string_lossy(), opts)
                .expect("absolute-path entry");
            writer.write_all(b"pwned").expect("write abs");
            writer.start_file("safe.txt", opts).expect("benign entry");
            writer.write_all(b"ok").expect("write safe");
            writer.finish().expect("finalize hostile zip");
        }

        extract_one(&zip_path, &cache).expect("extraction must not error");

        assert!(
            !dir.join("escape-rel.txt").exists(),
            "zip-slip: `../` entry escaped the cache into its parent dir",
        );
        assert!(
            !abs_escape.exists(),
            "zip-slip: absolute-path entry escaped the cache dir",
        );
        assert!(
            cache.join("safe.txt").is_file(),
            "benign entry must still extract after hostile entries are skipped",
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
