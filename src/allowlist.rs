//! Load the two per-user handle lists — `config/keep_allowlist.txt`
//! (never-unfollow) and `config/drop_list.txt` (always-unfollow) — into
//! in-memory [`HashSet`]s. Both reuse the same [`parse`] rules.
//!
//! Format mirrors [`crate::labels`]: one handle per line, `#` introduces a
//! comment to end-of-line, blank lines are ignored. Multi-token bare lines
//! (e.g. `alice bob` without a `#`) are a HARD parse error — Instagram
//! handles do not contain whitespace, and silently dropping the second
//! token would create a phantom entry that never matches.
//!
//! Stored values are ASCII-lowercased on insert so
//! [`Classifier::is_allowlisted`](crate::features::Classifier::is_allowlisted)
//! and [`Classifier::is_drop_listed`](crate::features::Classifier::is_drop_listed)
//! lookups don't re-allocate per query.
//!
//! Missing file → empty set. Both lists are opt-in; a fresh install
//! has no entries (only the comment-only templates), and the brand
//! lexicon is the primary defence on the keep side.
//!
//! The two lists must be disjoint — a handle on both is a keep/drop
//! contradiction. [`ensure_disjoint`] enforces this loudly at load.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};

const DEFAULT_PATH: &str = "config/keep_allowlist.txt";
const DROP_LIST_PATH: &str = "config/drop_list.txt";

/// Load the keep-allowlist from the default path. Missing file → empty set.
pub fn load_default() -> Result<HashSet<String>> {
    load_handle_list(Path::new(DEFAULT_PATH))
}

/// Load the drop-list from the default path. Missing file → empty set.
/// Exact mirror of [`load_default`] against `config/drop_list.txt`.
pub fn load_drop_list() -> Result<HashSet<String>> {
    load_handle_list(Path::new(DROP_LIST_PATH))
}

fn load_handle_list(path: &Path) -> Result<HashSet<String>> {
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let body =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&body, &path.display().to_string())
}

/// Bail if any handle appears in both the keep-allowlist and the drop-list.
///
/// A both-listed handle is a contradiction (never-unfollow vs. always-
/// unfollow); resolving it silently would be a lie about user intent.
/// Called in `lib::run` before scoring, so `assign_bucket` never sees a
/// both-listed handle and the drop-vs-keep precedence is moot by
/// construction. The error names every offending handle (sorted, for a
/// deterministic message) and both files.
pub fn ensure_disjoint(keep: &HashSet<String>, drop: &HashSet<String>) -> Result<()> {
    let mut overlap: Vec<&str> = keep.intersection(drop).map(String::as_str).collect();
    if overlap.is_empty() {
        return Ok(());
    }
    overlap.sort_unstable();
    bail!(
        "handle(s) appear in BOTH {DEFAULT_PATH} and {DROP_LIST_PATH}: {} \
         — a keep/drop contradiction; remove each from one list before scoring",
        overlap.join(", "),
    );
}

/// Parse a newline-separated allowlist body, naming `source` in error
/// messages. Strips `#`-comments and blanks, ASCII-lowercases entries,
/// and bails on multi-token bare lines (paralleling [`crate::labels::load`]).
pub fn parse(body: &str, source: &str) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    for (idx, raw) in body.lines().enumerate() {
        let line_no = idx + 1;
        // Same `#`-strip-then-trim shape as labels.rs — `#` is the comment
        // delimiter, so an inline `alice  # close friend from college` works.
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let handle = parts.next().ok_or_else(|| {
            anyhow::anyhow!("{source}:{line_no}: empty entry after comment strip")
        })?;
        if parts.next().is_some() {
            bail!(
                "{source}:{line_no}: extra tokens after `{handle}` \
                 (Instagram handles do not contain whitespace — use `#` for comments)"
            );
        }
        out.insert(handle.to_ascii_lowercase());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(body: &str) -> HashSet<String> {
        parse(body, "test").expect("body parses cleanly")
    }

    #[test]
    fn parses_handles_and_strips_comments_and_blanks() {
        let body = "\
            # leading comment\n\
            \n\
            natgeo\n\
            close_friend_handle    # inline comment\n\
            CASED_HANDLE\n\
            \n\
            # trailing comment\n\
        ";
        let set = parse_ok(body);
        assert!(set.contains("natgeo"));
        assert!(set.contains("close_friend_handle"));
        assert!(set.contains("cased_handle"), "ASCII-lowercased on insert");
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn empty_body_yields_empty_set() {
        assert!(parse_ok("").is_empty());
        assert!(parse_ok("# only comments\n# more\n").is_empty());
    }

    #[test]
    fn whitespace_only_lines_are_ignored() {
        // Defends against editor trailing whitespace polluting the set
        // with empty-string keys — would otherwise allowlist every
        // followee whose handle is empty.
        let set = parse_ok("   \n\t\n  alice  \n");
        assert_eq!(set.len(), 1);
        assert!(set.contains("alice"));
    }

    #[test]
    fn multi_token_bare_line_is_a_hard_error() {
        // Without this, `alice bob` would be silently dropped (the previous
        // parse stored `"alice bob"` as one key, which can never match a
        // real handle — a phantom entry the user never sees). Matches the
        // labels-track precedent.
        let err = parse("alice bob\n", "test").expect_err("must reject multi-token line");
        let msg = err.to_string();
        assert!(msg.contains("test:1"), "must name the line: {msg}");
        assert!(msg.contains("alice"), "must name the handle: {msg}");
    }

    #[test]
    fn inline_comment_after_handle_is_fine() {
        // The bail must NOT fire on `handle # comment` — that shape is
        // explicitly supported and the labels track uses it too.
        let set = parse_ok("alice  # a close friend from college\n");
        assert_eq!(set.len(), 1);
        assert!(set.contains("alice"));
    }

    fn set_of(handles: &[&str]) -> HashSet<String> {
        handles.iter().map(|h| (*h).to_owned()).collect()
    }

    #[test]
    fn disjoint_lists_pass() {
        let keep = set_of(&["alice", "bob"]);
        let drop = set_of(&["carol", "dave"]);
        ensure_disjoint(&keep, &drop).expect("disjoint lists must be Ok");
    }

    #[test]
    fn empty_lists_are_disjoint() {
        ensure_disjoint(&HashSet::new(), &HashSet::new()).expect("two empty sets are disjoint");
        ensure_disjoint(&set_of(&["alice"]), &HashSet::new()).expect("one empty side is disjoint");
    }

    #[test]
    fn overlapping_handle_is_a_hard_error_naming_both_files() {
        // A handle on both lists is a keep/drop contradiction. The error
        // must name the offending handle AND both files so the user can
        // fix it without guessing which line to delete.
        let keep = set_of(&["alice", "shared_handle"]);
        let drop = set_of(&["shared_handle", "carol"]);
        let err = ensure_disjoint(&keep, &drop).expect_err("overlap must error");
        let msg = err.to_string();
        assert!(msg.contains("shared_handle"), "must name the handle: {msg}");
        assert!(
            msg.contains("config/keep_allowlist.txt"),
            "must name the keep-allowlist file: {msg}",
        );
        assert!(
            msg.contains("config/drop_list.txt"),
            "must name the drop-list file: {msg}",
        );
    }

    #[test]
    fn disjointness_compares_case_folded_when_built_via_parse() {
        // The contract: `ensure_disjoint` assumes pre-lowercased input and
        // both production sets come through `parse` (which lowercases on
        // insert). Pin that end to end — `Alice` in one file and `alice`
        // in the other IS a conflict, even though the raw casing differs.
        let keep = parse("Alice\n", "keep").expect("keep parses");
        let drop = parse("alice\n", "drop").expect("drop parses");
        let err = ensure_disjoint(&keep, &drop).expect_err("case-differing dup must conflict");
        assert!(err.to_string().contains("alice"), "{err}");
    }

    #[test]
    fn multiple_overlaps_are_all_named() {
        // All conflicting handles surface in one error, sorted for a
        // deterministic message — not just the first one found.
        let keep = set_of(&["zeta", "alpha"]);
        let drop = set_of(&["zeta", "alpha"]);
        let err = ensure_disjoint(&keep, &drop).expect_err("overlap must error");
        let msg = err.to_string();
        assert!(msg.contains("alpha"), "must name alpha: {msg}");
        assert!(msg.contains("zeta"), "must name zeta: {msg}");
        // Sorted: alpha precedes zeta in the rendered list.
        let a = msg.find("alpha").unwrap();
        let z = msg.find("zeta").unwrap();
        assert!(a < z, "handles must render sorted: {msg}");
    }
}
