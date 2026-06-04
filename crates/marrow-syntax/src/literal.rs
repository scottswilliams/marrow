//! Canonical decoding of string-literal text into its runtime value.
//!
//! A Marrow string literal recognizes exactly five escapes: `\\`, `\"`, `\n`,
//! `\r`, and `\t`. Any other backslash escape, and a trailing backslash with no
//! following character, is rejected. Every layer that interprets string-literal
//! text — the evaluator, the checker's constant defaults, and the saved-path
//! key parser — decodes through here so the escape set has a single owner.
//!
//! Bytes literals (`b"..."`) recognize a wider set including `\xHH` and are
//! decoded by the runtime's bytes codec, not here.

/// Why decoding a string literal failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringLiteralError {
    /// The text was not wrapped in the surrounding double quotes a literal needs.
    Unquoted,
    /// An escape sequence is not one of the recognized five, or the text ends on
    /// a lone backslash.
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

/// Decode the escapes in already-unquoted string text. Interpolation literal
/// segments arrive without quotes and use this directly.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
