//! Canonical decoding of string- and bytes-literal text into their runtime values.
//!
//! A Marrow string literal recognizes exactly five escapes: `\\`, `\"`, `\n`,
//! `\r`, and `\t`. A bytes literal recognizes those same five plus `\xNN` hex.
//! Any other backslash escape, a trailing backslash with no following character,
//! and a malformed or truncated `\xNN` are rejected. Every layer that interprets
//! literal text — the evaluator, the checker's literal validation and constant
//! defaults, and the saved-path key parser — decodes through here so each escape
//! grammar has a single owner.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringLiteralError {
    /// Missing the surrounding double quotes.
    Unquoted,
    /// An unrecognized escape, or a trailing lone backslash. The offset is the
    /// byte position of the opening backslash within the decoded text, so a
    /// diagnostic can point at the escape rather than the whole literal.
    BadEscape { offset: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytesLiteralError {
    /// Missing the surrounding `b"` … `"` delimiters.
    Unquoted,
    /// An unrecognized escape, a trailing lone backslash, or a malformed or
    /// truncated `\xNN` hex escape. The offset is the byte position of the
    /// opening backslash within the decoded text.
    BadEscape { offset: usize },
}

/// Decode a full string literal — surrounding quotes included — into its value.
/// A bad-escape offset is reported relative to the full literal, so it accounts
/// for the opening quote stripped here.
pub fn decode_string_literal(text: &str) -> Result<String, StringLiteralError> {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or(StringLiteralError::Unquoted)?;
    decode_string_escapes(inner).map_err(|error| shift_string_offset(error, 1))
}

fn shift_string_offset(error: StringLiteralError, by: usize) -> StringLiteralError {
    match error {
        StringLiteralError::BadEscape { offset } => StringLiteralError::BadEscape {
            offset: offset + by,
        },
        other => other,
    }
}

/// Encode a string into the quoted, escaped spelling that [`decode_string_literal`]
/// is the exact inverse of: the five recognized escapes for `\`, `"`, newline,
/// carriage return, and tab; every other scalar — control characters and
/// non-ASCII alike — emitted literally as `string_text`. This is the canonical
/// encoder for any tool, such as the saved-path renderer, whose output must
/// re-parse through the language's string grammar.
pub fn encode_string_literal(value: &str) -> String {
    let mut text = String::with_capacity(value.len() + 2);
    text.push('"');
    push_string_escapes(&mut text, value);
    text.push('"');
    text
}

/// Append `value` to `text` with the five recognized escapes applied, no quotes.
pub fn push_string_escapes(text: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => text.push_str("\\\\"),
            '"' => text.push_str("\\\""),
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            _ => text.push(ch),
        }
    }
}

/// Decode escapes in already-unquoted text (interpolation segments use this
/// directly, having no quotes to strip).
pub fn decode_string_escapes(inner: &str) -> Result<String, StringLiteralError> {
    let mut decoded = String::with_capacity(inner.len());
    let mut chars = inner.char_indices();
    while let Some((offset, ch)) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        let bad = StringLiteralError::BadEscape { offset };
        let (_, escaped) = chars.next().ok_or(bad)?;
        decoded.push(match escaped {
            '\\' => '\\',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            _ => return Err(bad),
        });
    }
    Ok(decoded)
}

/// Decode a full bytes literal — surrounding `b"` … `"` included — into its bytes.
/// A bad-escape offset is reported relative to the full literal, so it accounts
/// for the `b"` prefix stripped here.
pub fn decode_bytes_literal(text: &str) -> Result<Vec<u8>, BytesLiteralError> {
    let inner = text
        .strip_prefix("b\"")
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or(BytesLiteralError::Unquoted)?;
    decode_bytes_escapes(inner).map_err(|error| shift_bytes_offset(error, 2))
}

fn shift_bytes_offset(error: BytesLiteralError, by: usize) -> BytesLiteralError {
    match error {
        BytesLiteralError::BadEscape { offset } => BytesLiteralError::BadEscape {
            offset: offset + by,
        },
        other => other,
    }
}

/// Decode escapes in already-unquoted bytes-literal text. Ordinary characters
/// contribute their UTF-8 bytes; the five string escapes plus `\xNN` hex emit
/// individual byte values.
pub fn decode_bytes_escapes(inner: &str) -> Result<Vec<u8>, BytesLiteralError> {
    let mut decoded = Vec::with_capacity(inner.len());
    let mut chars = inner.char_indices();
    while let Some((offset, ch)) = chars.next() {
        if ch != '\\' {
            let mut buffer = [0; 4];
            decoded.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        let bad = BytesLiteralError::BadEscape { offset };
        let (_, escaped) = chars.next().ok_or(bad)?;
        match escaped {
            '\\' => decoded.push(b'\\'),
            '"' => decoded.push(b'"'),
            'n' => decoded.push(b'\n'),
            'r' => decoded.push(b'\r'),
            't' => decoded.push(b'\t'),
            'x' => {
                let high = chars.next().and_then(|(_, c)| hex_digit(c)).ok_or(bad)?;
                let low = chars.next().and_then(|(_, c)| hex_digit(c)).ok_or(bad)?;
                decoded.push((high << 4) | low);
            }
            _ => return Err(bad),
        }
    }
    Ok(decoded)
}

fn hex_digit(ch: char) -> Option<u8> {
    ch.to_digit(16).and_then(|digit| u8::try_from(digit).ok())
}

#[cfg(test)]
mod tests {
    use super::{
        BytesLiteralError, StringLiteralError, decode_bytes_escapes, decode_bytes_literal,
        decode_string_escapes, decode_string_literal, encode_string_literal,
    };

    #[test]
    fn encode_string_literal_inverts_decode() {
        // A raw control char (ESC) and a non-ASCII scalar are `string_text`, so they
        // must survive a round trip literally; only the five recognized characters are
        // escaped. The encoder is the exact inverse the saved-path renderer relies on.
        for value in [
            "plain",
            "a\\b\"c\nd\re\tf",
            "k\u{1b}\u{00e9}z",
            "\u{0} \u{7f} \u{1b}",
        ] {
            let encoded = encode_string_literal(value);
            assert_eq!(
                decode_string_literal(&encoded).unwrap(),
                value,
                "round trip failed for {value:?} via {encoded:?}"
            );
        }
    }

    #[test]
    fn encode_string_literal_emits_only_the_five_escapes() {
        assert_eq!(
            encode_string_literal("a\\b\"c\nd\re\tf"),
            r#""a\\b\"c\nd\re\tf""#
        );
        // A raw ESC stays literal rather than becoming a Rust-style `\u{1b}`.
        assert_eq!(encode_string_literal("k\u{1b}z"), "\"k\u{1b}z\"");
    }

    #[test]
    fn decodes_the_five_escapes() {
        assert_eq!(
            decode_string_escapes(r#"a\\b\"c\nd\re\tf"#).unwrap(),
            "a\\b\"c\nd\re\tf"
        );
    }

    #[test]
    fn passes_through_unescaped_text() {
        assert_eq!(
            decode_string_escapes("plain text 123").unwrap(),
            "plain text 123"
        );
    }

    #[test]
    fn rejects_unknown_escapes() {
        for bad in [r"\0", r"\x41", r"\a", r"\u", r"\1"] {
            assert_eq!(
                decode_string_escapes(bad),
                Err(StringLiteralError::BadEscape { offset: 0 }),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn reports_the_offset_of_a_bad_escape() {
        // The offset points at the backslash, not the start of the text, and the
        // full-literal decoder shifts it past the opening quote.
        assert_eq!(
            decode_string_escapes("ok then \\q"),
            Err(StringLiteralError::BadEscape { offset: 8 })
        );
        assert_eq!(
            decode_string_literal(r#""ok then \q""#),
            Err(StringLiteralError::BadEscape { offset: 9 })
        );
    }

    #[test]
    fn rejects_a_trailing_backslash() {
        assert_eq!(
            decode_string_escapes("ends here\\"),
            Err(StringLiteralError::BadEscape { offset: 9 })
        );
    }

    #[test]
    fn decode_string_literal_strips_quotes() {
        assert_eq!(
            decode_string_literal(r#""hi\nthere""#).unwrap(),
            "hi\nthere"
        );
    }

    #[test]
    fn decode_string_literal_requires_both_quotes() {
        for unquoted in [r#"hi""#, r#""hi"#, "hi", ""] {
            assert_eq!(
                decode_string_literal(unquoted),
                Err(StringLiteralError::Unquoted),
                "expected {unquoted:?} to be unquoted"
            );
        }
    }

    #[test]
    fn bytes_decode_the_five_escapes_and_hex() {
        assert_eq!(
            decode_bytes_escapes(r#"a\\b\"c\nd\re\tf\xff"#).unwrap(),
            b"a\\b\"c\nd\re\tf\xff"
        );
    }

    #[test]
    fn bytes_pass_through_unescaped_utf8() {
        assert_eq!(
            decode_bytes_escapes("plain \u{00e9}").unwrap(),
            "plain \u{00e9}".as_bytes()
        );
    }

    #[test]
    fn bytes_reject_unknown_and_truncated_escapes() {
        for (bad, offset) in [
            (r"\q", 0),
            (r"\x", 0),
            (r"\x1", 0),
            (r"\xg0", 0),
            (r"ok\", 2),
        ] {
            assert_eq!(
                decode_bytes_escapes(bad),
                Err(BytesLiteralError::BadEscape { offset }),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn bytes_literal_offset_accounts_for_the_b_prefix() {
        // The `b"` prefix is two bytes, so a bad escape at inner offset 1 lands at
        // full-literal offset 3.
        assert_eq!(
            decode_bytes_literal(r#"b"a\q""#),
            Err(BytesLiteralError::BadEscape { offset: 3 })
        );
    }

    #[test]
    fn decode_bytes_literal_strips_delimiters() {
        assert_eq!(decode_bytes_literal(r#"b"\xff\n""#).unwrap(), b"\xff\n");
    }

    #[test]
    fn decode_bytes_literal_requires_delimiters() {
        for unquoted in [r#""hi""#, r#"b"hi"#, "hi", ""] {
            assert_eq!(
                decode_bytes_literal(unquoted),
                Err(BytesLiteralError::Unquoted),
                "expected {unquoted:?} to be unquoted"
            );
        }
    }
}
