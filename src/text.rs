//! String repair for Instagram's export-encoding bug.
//!
//! Instagram's "Download Your Information" exporter has shipped a known
//! UTF-8-as-Latin-1 mis-encoding for years: it takes the UTF-8 bytes of a
//! display name, re-interprets each byte as a Latin-1 codepoint, then
//! JSON-escapes the result. `Hüseyin` (`48 c3 bc 73 65 79 69 6e`) ships as
//! `"HÃ¼seyin"`, which any JSON parser decodes to `HÃ¼seyin`.
//!
//! [`fix_mojibake`] reverses the damage by re-encoding each char as its
//! Latin-1 byte and decoding the byte sequence as UTF-8. The fix is
//! heuristic but safe in practice: it only commits when every char fits
//! in one byte AND the byte sequence is valid UTF-8 AND the result
//! differs from the input. Plain ASCII handles, correctly-encoded
//! European names with diacritics, and any string containing chars
//! `> U+00FF` pass through untouched.
//!
//! Double-application: a small minority of strings (Arabic, some
//! emoji) round-trip through IG's exporter twice and arrive as
//! `ÃÂÃÂ`-shaped sequences. The function iterates up to three passes,
//! stopping when an additional pass would not produce a strictly
//! shorter byte sequence — monotonic improvement guards against the
//! repair "fixing" a string that was already correct.

use std::borrow::Cow;

/// Repair the known Instagram-export UTF-8-as-Latin-1 mojibake.
///
/// Returns `Cow::Borrowed(s)` when the input is already clean (the
/// common case for ASCII handles and correctly-encoded names), or
/// `Cow::Owned(_)` with the repaired string. See module docs for
/// the policy.
pub fn fix_mojibake(s: &str) -> Cow<'_, str> {
    let Some(first) = single_pass(s) else {
        return Cow::Borrowed(s);
    };
    // Try up to two more passes for double-mojibake (Arabic, some
    // emoji). Stop as soon as a pass fails to shorten the byte
    // sequence — equal-length output means the repair has converged
    // OR is over-applying, and either way we should not continue.
    let mut current = first;
    for _ in 0..2 {
        let Some(next) = single_pass(&current) else {
            break;
        };
        if next.len() >= current.len() {
            break;
        }
        current = next;
    }
    Cow::Owned(current)
}

/// One mojibake pass. Returns `Some(repaired)` only if the input is
/// fully Latin-1 (every char ≤ U+00FF), the resulting byte sequence
/// is valid UTF-8, AND the decoded string contains at least one
/// multibyte sequence (i.e., it is meaningfully different from the
/// input, not just a no-op re-encoding of pure ASCII).
fn single_pass(s: &str) -> Option<String> {
    let bytes: Vec<u8> = s
        .chars()
        .map(|c| u8::try_from(c as u32).ok())
        .collect::<Option<_>>()?;
    let repaired = String::from_utf8(bytes).ok()?;
    // Require an actual encoding change: pure-ASCII input round-trips
    // to itself byte-for-byte, and committing `Cow::Owned` there would
    // allocate without changing anything.
    (repaired != s).then_some(repaired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through_borrowed() {
        let s = "alice_h";
        let out = fix_mojibake(s);
        assert_eq!(out, "alice_h");
        assert!(matches!(out, Cow::Borrowed(_)), "ASCII must not allocate");
    }

    #[test]
    fn correctly_encoded_utf8_passes_through() {
        // Correctly-encoded "Hüseyin": single 'ü' codepoint U+00FC. As
        // a u8 it's 0xFC, which is not a valid UTF-8 leading byte, so
        // single_pass returns None and we pass through unchanged.
        let s = "Hüseyin";
        let out = fix_mojibake(s);
        assert_eq!(out, "Hüseyin");
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "valid UTF-8 must not allocate"
        );
    }

    /// Build a string from raw bytes by reading each byte as its Latin-1
    /// codepoint. Models exactly what JSON `\u00XX` escapes produce when
    /// IG's exporter writes UTF-8 bytes as if they were Latin-1.
    fn from_latin1_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|&b| b as char).collect()
    }

    #[test]
    fn repairs_turkish_diacritic() {
        // 'ü' UTF-8 = c3 bc. IG ships those two bytes as two Latin-1
        // codepoints: Ã (U+00C3) and ¼ (U+00BC).
        let mojibake = from_latin1_bytes(b"H\xc3\xbcseyin");
        assert_eq!(fix_mojibake(&mojibake), "Hüseyin");
    }

    #[test]
    fn repairs_4byte_emoji() {
        // 🍕 (U+1F355) UTF-8 = f0 9f 8d 95. Three of the four bytes
        // land in the C1 control range — invisible in most terminals
        // but exact in byte form.
        let mojibake = from_latin1_bytes(b"Terry P \xf0\x9f\x8d\x95");
        assert_eq!(fix_mojibake(&mojibake), "Terry P 🍕");
    }

    #[test]
    fn repairs_georgian() {
        // 'ლ' (U+10DA, GEORGIAN LETTER LAS) UTF-8 = e1 83 9a.
        // Common in the user's labeled set; pin behavior.
        let mojibake = from_latin1_bytes(b"\xe1\x83\x9aali");
        assert_eq!(fix_mojibake(&mojibake), "ლali");
    }

    #[test]
    fn handles_string_with_no_latin1_high_bytes() {
        // "Sam" is pure ASCII — no chars in 0x80-0xff range, so
        // single_pass's `repaired != s` check rejects (round-trips
        // to itself). Must pass through unchanged.
        assert_eq!(fix_mojibake("Sam"), "Sam");
    }

    #[test]
    fn empty_string_passes_through() {
        let out = fix_mojibake("");
        assert_eq!(out, "");
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn rejects_repair_when_byte_sequence_invalid_utf8() {
        // Lone Latin-1 high byte that doesn't form a valid UTF-8
        // start. `Ñ` alone is `D1` which is a UTF-8 leading byte
        // expecting a continuation — by itself, invalid. Pass through.
        let s = "Ñurpis";
        assert_eq!(fix_mojibake(s), "Ñurpis");
    }

    #[test]
    fn idempotent_on_already_repaired_string() {
        // Running the fix twice on the same input must produce the
        // same output as running it once. Real callsites only invoke
        // the fix once per string, but idempotency is the cheap
        // safety property that prevents a future double-fix from
        // corrupting good data.
        let mojibake = "HÃ¼seyin";
        let pass_one = fix_mojibake(mojibake).into_owned();
        let pass_two = fix_mojibake(&pass_one).into_owned();
        assert_eq!(pass_one, pass_two);
        assert_eq!(pass_one, "Hüseyin");
    }

    #[test]
    fn repairs_double_mojibake_via_multiple_passes() {
        // A minority of strings round-trip through IG's exporter twice and
        // arrive double-encoded (the ÃÂÃÂ shape). fix_mojibake iterates
        // until a pass stops shortening the byte sequence. This pins that
        // convergence loop: a `>=`→`<` mutation on the guard would stop
        // after one pass and leave the string still single-mojibake'd.
        fn mojibake_once(s: &str) -> String {
            s.bytes().map(|b| b as char).collect()
        }
        let clean = "Hüseyin";
        let double = mojibake_once(&mojibake_once(clean));
        assert_ne!(double, clean, "double mojibake must differ from clean");
        assert_eq!(fix_mojibake(&double), clean);
    }
}
