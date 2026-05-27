//! Load `config/keep_allowlist.txt` into an in-memory [`HashSet`].
//!
//! Format mirrors [`crate::labels`]: one handle per line, `#` introduces a
//! comment to end-of-line, blank lines are ignored. Stored values are
//! ASCII-lowercased on insert so
//! [`Classifier::is_allowlisted`](crate::features::Classifier::is_allowlisted)
//! lookups don't re-allocate per query.
//!
//! Missing file → empty set. The allowlist is opt-in; a fresh install
//! has no entries (only the comment-only template), and the brand
//! lexicon is the primary defence.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};

const DEFAULT_PATH: &str = "config/keep_allowlist.txt";

/// Load the allowlist from the default path. Missing file → empty set.
pub fn load_default() -> Result<HashSet<String>> {
    let path = Path::new(DEFAULT_PATH);
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let body =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(parse(&body))
}

/// Parse a newline-separated allowlist body. Strips `#`-comments and
/// blanks; ASCII-lowercases entries.
pub fn parse(body: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for raw in body.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        out.insert(line.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let set = parse(body);
        assert!(set.contains("natgeo"));
        assert!(set.contains("close_friend_handle"));
        assert!(set.contains("cased_handle"), "ASCII-lowercased on insert");
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn empty_body_yields_empty_set() {
        assert!(parse("").is_empty());
        assert!(parse("# only comments\n# more\n").is_empty());
    }

    #[test]
    fn whitespace_only_lines_are_ignored() {
        // Defends against editor trailing whitespace polluting the set
        // with empty-string keys — would otherwise allowlist every
        // followee whose handle is empty.
        let set = parse("   \n\t\n  alice  \n");
        assert_eq!(set.len(), 1);
        assert!(set.contains("alice"));
    }
}
