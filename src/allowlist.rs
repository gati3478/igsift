//! Load `config/keep_allowlist.txt` into an in-memory [`HashSet`].
//!
//! Format mirrors [`crate::labels`]: one handle per line, `#` introduces a
//! comment to end-of-line, blank lines are ignored. Multi-token bare lines
//! (e.g. `alice bob` without a `#`) are a HARD parse error — Instagram
//! handles do not contain whitespace, and silently dropping the second
//! token would create a phantom allowlist entry that never matches.
//!
//! Stored values are ASCII-lowercased on insert so
//! [`Classifier::is_allowlisted`](crate::features::Classifier::is_allowlisted)
//! lookups don't re-allocate per query.
//!
//! Missing file → empty set. The allowlist is opt-in; a fresh install
//! has no entries (only the comment-only template), and the brand
//! lexicon is the primary defence.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};

const DEFAULT_PATH: &str = "config/keep_allowlist.txt";

/// Load the allowlist from the default path. Missing file → empty set.
pub fn load_default() -> Result<HashSet<String>> {
    let path = Path::new(DEFAULT_PATH);
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let body =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&body, &path.display().to_string())
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
}
