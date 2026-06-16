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
    /// An unrecognized escape, or a trailing lone backslash.
    BadEscape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytesLiteralError {
    /// Missing the surrounding `b"` … `"` delimiters.
    Unquoted,
    /// An unrecognized escape, a trailing lone backslash, or a malformed or
    /// truncated `\xNN` hex escape.
    BadEscape,
}

/// Decode a full string literal — surrounding quotes included — into its value.
pub fn decode_string_literal(text: &str) -> Result<String, StringLiteralError> {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or(StringLiteralError::Unquoted)?;
    decode_string_escapes(inner)
}

/// Decode escapes in already-unquoted text (interpolation segments use this
/// directly, having no quotes to strip).
pub fn decode_string_escapes(inner: &str) -> Result<String, StringLiteralError> {
    let mut decoded = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        let escaped = chars.next().ok_or(StringLiteralError::BadEscape)?;
        decoded.push(match escaped {
            '\\' => '\\',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            _ => return Err(StringLiteralError::BadEscape),
        });
    }
    Ok(decoded)
}

/// Decode a full bytes literal — surrounding `b"` … `"` included — into its bytes.
pub fn decode_bytes_literal(text: &str) -> Result<Vec<u8>, BytesLiteralError> {
    let inner = text
        .strip_prefix("b\"")
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or(BytesLiteralError::Unquoted)?;
    decode_bytes_escapes(inner)
}

/// Decode escapes in already-unquoted bytes-literal text. Ordinary characters
/// contribute their UTF-8 bytes; the five string escapes plus `\xNN` hex emit
/// individual byte values.
pub fn decode_bytes_escapes(inner: &str) -> Result<Vec<u8>, BytesLiteralError> {
    let mut decoded = Vec::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buffer = [0; 4];
            decoded.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        let escaped = chars.next().ok_or(BytesLiteralError::BadEscape)?;
        match escaped {
            '\\' => decoded.push(b'\\'),
            '"' => decoded.push(b'"'),
            'n' => decoded.push(b'\n'),
            'r' => decoded.push(b'\r'),
            't' => decoded.push(b'\t'),
            'x' => {
                let high = chars
                    .next()
                    .and_then(hex_digit)
                    .ok_or(BytesLiteralError::BadEscape)?;
                let low = chars
                    .next()
                    .and_then(hex_digit)
                    .ok_or(BytesLiteralError::BadEscape)?;
                decoded.push((high << 4) | low);
            }
            _ => return Err(BytesLiteralError::BadEscape),
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
        decode_string_escapes, decode_string_literal,
    };

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
                Err(StringLiteralError::BadEscape),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn rejects_a_trailing_backslash() {
        assert_eq!(
            decode_string_escapes("ends here\\"),
            Err(StringLiteralError::BadEscape)
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
        for bad in [r"\q", r"\x", r"\x1", r"\xg0", r"ok\"] {
            assert_eq!(
                decode_bytes_escapes(bad),
                Err(BytesLiteralError::BadEscape),
                "expected {bad:?} to be rejected"
            );
        }
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
