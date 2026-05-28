//! CSV row writer.
//!
//! Materializes one [`CsvRow`] per scored account. Columns match
//! [`docs/DESIGN.md`](../../../docs/DESIGN.md) "Output" exactly — that
//! header is the inter-run diff contract and changes only when the
//! design doc moves first.
//!
//! ## Why `Option<u32>` rather than empty strings
//!
//! Fields like `last_dm_days` and `follow_tenure_days` are genuinely
//! optional — an account with no DMs has no last-DM date, and a
//! followee whose `followed_at` was missing from the export has no
//! tenure. `csv` + `serde` serialize `None` as an empty field by
//! default, which is the right encoding for "this row doesn't have a
//! value" vs. "this row has a zero." `0` would lie.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::io::Write;

use anyhow::{Context, Result};
use serde::{Serialize, Serializer};

use crate::scoring::ScoredAccount;

/// Serialized CSV row. The handle-keyed string fields borrow from the
/// underlying `ScoredAccount` so the writer doesn't clone per row;
/// `profile_url` is the one owned field — built once per row from the
/// handle and reused by the Markdown writer if it ever shares this row.
#[derive(Serialize)]
struct CsvRow<'a> {
    username: Cow<'a, str>,
    display_name: Cow<'a, str>,
    profile_url: String,
    bucket: &'static str,
    /// Three-decimal float matching the stdout summary so a user can
    /// cross-reference CSV rows with the printed top-10 / bottom-10
    /// without arithmetic.
    #[serde(serialize_with = "fmt_three_decimals")]
    keep_prob: f64,
    dm_msgs: u32,
    last_dm_days: Option<u32>,
    reactions_given_180d: u32,
    reactions_received_180d: u32,
    likes_given_90d: u32,
    comments_given_90d: u32,
    follow_tenure_days: Option<u32>,
    account_class: &'static str,
    /// `true` iff the followee also follows back. Decision support only —
    /// scoring is neutral on reciprocity (see `is_mutual` doc in
    /// `AccountFeatures`). Emitted as the bare word `true`/`false` so a
    /// spreadsheet filter on `mutual == false` works without arithmetic.
    mutual: bool,
    /// The `dominant_feature` from scoring — the term with the largest
    /// signed contribution to `score_raw`. Tells the user **why** this
    /// account landed where it did at a glance.
    notes: Cow<'a, str>,
}

fn fmt_three_decimals<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{v:.3}"))
}

/// Guard against CSV formula injection (CWE-1236). Excel, Google Sheets,
/// and LibreOffice evaluate a cell whose first byte is `=`, `+`, `-`, `@`,
/// TAB, or CR as a formula. `display_name` is free-form text controlled by
/// the third-party account, so a name like `=HYPERLINK("http://evil")`
/// would execute when the audit CSV is opened. Prefix any such cell with a
/// `'` so the spreadsheet renders it as literal text; borrow through
/// untouched in the (overwhelmingly common) safe case.
fn sanitize_cell(s: &str) -> Cow<'_, str> {
    match s.as_bytes().first() {
        Some(b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r') => Cow::Owned(format!("'{s}")),
        _ => Cow::Borrowed(s),
    }
}

/// Build the canonical Instagram profile URL for a handle. Instagram
/// handles are restricted to `[A-Za-z0-9._]`, so no URL encoding is
/// needed. Shared with the Markdown writer.
pub(super) fn profile_url(handle: &str) -> String {
    format!("https://www.instagram.com/{handle}/")
}

/// Write all scored accounts as CSV to `writer`, ascending by
/// `keep_prob`. See [module docs](self) for the ordering rationale.
pub fn write_to(scored: &[ScoredAccount], writer: impl Write) -> Result<()> {
    let mut sorted: Vec<&ScoredAccount> = scored.iter().collect();
    sorted.sort_by(|a, b| {
        a.keep_prob
            .partial_cmp(&b.keep_prob)
            .unwrap_or(Ordering::Equal)
            // Stable secondary key on handle so two accounts at identical
            // keep_prob (e.g. both 1.000 from a saturated boost) sort
            // deterministically across runs.
            .then_with(|| a.features.username.cmp(&b.features.username))
    });

    let mut wtr = ::csv::Writer::from_writer(writer);
    for s in sorted {
        let row = CsvRow {
            username: sanitize_cell(&s.features.username),
            display_name: sanitize_cell(s.features.display_name.as_deref().unwrap_or("")),
            profile_url: profile_url(&s.features.username),
            bucket: s.bucket.as_str(),
            keep_prob: s.keep_prob,
            dm_msgs: s.features.dm_messages_total,
            last_dm_days: s.features.dm_recency_days,
            reactions_given_180d: s.features.dm_reactions_given_180d,
            reactions_received_180d: s.features.dm_reactions_received_180d,
            likes_given_90d: s.features.likes_given_90d,
            comments_given_90d: s.features.comments_given_90d,
            follow_tenure_days: s.features.follow_tenure_days,
            account_class: s.features.account_class.as_str(),
            mutual: s.features.is_mutual,
            notes: sanitize_cell(s.dominant_feature),
        };
        wtr.serialize(&row).context("serializing CSV row")?;
    }
    wtr.flush().context("flushing CSV writer")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The CSV header is the inter-run diff contract — pin it explicitly
    //! so a column rename or reorder is loud at PR time. The full output
    //! shape is covered by the integration test in `tests/cli.rs`.
    use super::*;
    use crate::features::{AccountClass, AccountFeatures};
    use crate::scoring::Bucket;

    /// Minimal fake. The `csv` crate auto-emits the header from struct
    /// field names on the FIRST `serialize` call — an empty input writes
    /// no file content, so every assertion below seeds at least one row.
    fn make_scored(handle: &str, keep_prob: f64) -> ScoredAccount {
        ScoredAccount {
            features: AccountFeatures {
                username: handle.to_owned(),
                display_name: None,
                account_class: AccountClass::default(),
                follow_tenure_days: None,
                is_close_friend: false,
                is_favorited: false,
                is_blocked: false,
                is_restricted: false,
                is_hide_story_from: false,
                is_removed_suggestion: false,
                recently_unfollowed: false,
                is_mutual: false,
                is_keep_allowlisted: false,
                likes_given: 0,
                comments_given: 0,
                story_interactions_out: 0,
                stories_viewed: 0,
                saved_their_content: 0,
                dm_messages_total: 0,
                dm_recency_days: None,
                dm_balance: None,
                dm_reactions_given: 0,
                dm_reactions_received: 0,
                inbound_dm_request: false,
                likes_given_decayed: 0.0,
                comments_given_decayed: 0.0,
                story_interactions_out_decayed: 0.0,
                stories_viewed_decayed: 0.0,
                saved_their_content_decayed: 0.0,
                dm_messages_total_decayed: 0.0,
                dm_reactions_given_decayed: 0.0,
                dm_reactions_received_decayed: 0.0,
                likes_given_90d: 0,
                comments_given_90d: 0,
                dm_reactions_given_180d: 0,
                dm_reactions_received_180d: 0,
            },
            score_raw: 0.0,
            keep_prob,
            bucket: Bucket::Review,
            dominant_feature: "none",
            top_terms: [("", 0.0); 3],
        }
    }

    fn render(scored: &[ScoredAccount]) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write_to(scored, &mut buf).expect("write");
        String::from_utf8(buf).expect("utf-8 csv")
    }

    #[test]
    fn header_matches_design_doc() {
        let scored = vec![make_scored("a", 0.5)];
        let csv = render(&scored);
        let header = csv.lines().next().expect("at least one line");
        assert_eq!(
            header,
            "username,display_name,profile_url,bucket,keep_prob,dm_msgs,last_dm_days,\
             reactions_given_180d,reactions_received_180d,\
             likes_given_90d,comments_given_90d,follow_tenure_days,\
             account_class,mutual,notes",
            "CSV header must match DESIGN.md 'Output' section verbatim",
        );
    }

    #[test]
    fn profile_url_is_built_from_handle() {
        let scored = vec![make_scored("alice_h", 0.5)];
        let csv = render(&scored);
        assert!(
            csv.contains("https://www.instagram.com/alice_h/"),
            "profile_url must be present and canonical: {csv}",
        );
    }

    #[test]
    fn rows_sorted_ascending_keep_prob() {
        let scored = vec![
            make_scored("high", 0.9),
            make_scored("low", 0.1),
            make_scored("mid", 0.5),
        ];
        let csv = render(&scored);
        let handles: Vec<&str> = csv
            .lines()
            .skip(1)
            .filter_map(|l| l.split(',').next())
            .collect();
        assert_eq!(handles, ["low", "mid", "high"]);
    }

    #[test]
    fn option_none_serializes_as_empty_field() {
        // `follow_tenure_days: None` and `last_dm_days: None` must emit
        // empty fields, not "0" — a missing value is not the same as a
        // zero value. The CSV crate's default Option serialization gets
        // this right; this test pins it so a future serializer swap
        // can't silently coerce None to "0".
        let scored = vec![make_scored("a", 0.5)];
        let csv = render(&scored);
        let row = csv.lines().nth(1).expect("data row");
        // last_dm_days and follow_tenure_days columns must be empty for
        // a default-constructed account.
        let fields: Vec<&str> = row.split(',').collect();
        // After profile_url at index 2 shifts everything by one:
        // last_dm_days is column 6, follow_tenure_days is column 11.
        assert_eq!(fields[6], "", "last_dm_days must be empty for None");
        assert_eq!(fields[11], "", "follow_tenure_days must be empty for None",);
    }

    #[test]
    fn formula_injection_in_display_name_is_neutralized() {
        // A third-party display name beginning with a spreadsheet formula
        // trigger must be prefixed with `'` so Excel/Sheets render it as
        // literal text rather than executing it (CWE-1236).
        let mut scored = make_scored("victim", 0.5);
        scored.features.display_name = Some("=HYPERLINK(\"http://evil\",\"x\")".to_owned());
        let csv = render(std::slice::from_ref(&scored));
        assert!(
            csv.contains("'=HYPERLINK"),
            "formula cell must be quoted with a leading apostrophe: {csv}",
        );
        assert!(
            !csv.contains(",=HYPERLINK") && !csv.contains("\"=HYPERLINK"),
            "raw formula must not reach a cell boundary unescaped: {csv}",
        );
    }

    #[test]
    fn ordinary_display_name_is_not_rewritten() {
        // The guard must not touch a benign name — only leading
        // `= + - @ TAB CR` triggers the prefix.
        let mut scored = make_scored("normal", 0.5);
        scored.features.display_name = Some("Sarah Connor".to_owned());
        let csv = render(std::slice::from_ref(&scored));
        assert!(
            csv.contains("Sarah Connor"),
            "benign name passes through: {csv}"
        );
        assert!(
            !csv.contains("'Sarah"),
            "benign name must not be prefixed: {csv}"
        );
    }
}
