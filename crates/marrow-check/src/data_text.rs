//! The escape vocabulary for string keys and values in the `data` tools' text
//! format, shared by the `data dump`/`data get` emitter and the saved-path
//! string-key parser so the two are exact inverses.
//!
//! This is deliberately broader than the `.mw` string-literal grammar. A stored
//! string may hold any byte, including control bytes such as NUL, BEL, or ESC
//! that the language gives no escaped spelling. A raw NUL cannot survive a shell
//! argument and other control bytes corrupt tab-separated and terminal output,
//! so the text format must render every control byte. It uses the five language
//! escapes plus `\xNN` (lowercase hex, the bytes-literal escape) for any other
//! control byte, leaving a dumped path always feedable back to `data get`. The
//! `.mw` literal grammar is unaffected; only this tooling format is total.

use crate::hex::push_lower_hex;

/// A control byte the text format escapes as `\xNN`: anything below `0x20` that
/// is not one of the five named escapes, plus `DEL`.
fn needs_hex_escape(ch: char) -> bool {
    matches!(ch, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}')
}

/// Append `value` with the text format's escapes applied, no surrounding quotes.
pub(crate) fn push_data_text_escapes(text: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => text.push_str("\\\\"),
            '"' => text.push_str("\\\""),
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            _ if needs_hex_escape(ch) => {
                text.push_str("\\x");
                push_lower_hex(text, &[ch as u8]);
            }
            _ => text.push(ch),
        }
    }
}

/// Encode a string into the quoted, escaped text-format spelling that
/// [`decode_data_text_escapes`] is the exact inverse of.
pub(crate) fn encode_data_text_string(value: &str) -> String {
    let mut text = String::with_capacity(value.len() + 2);
    text.push('"');
    push_data_text_escapes(&mut text, value);
    text.push('"');
    text
}

/// An unrecognized escape, a trailing lone backslash, or a malformed or
/// truncated `\xNN` escape in text-format string content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DataTextEscapeError;

/// Decode already-unquoted text-format string content, the inverse of
/// [`push_data_text_escapes`]. `\xNN` decodes one control byte; because the
/// format only ever escapes bytes below `0x80`, each forms one valid scalar.
pub(crate) fn decode_data_text_escapes(inner: &str) -> Result<String, DataTextEscapeError> {
    let mut decoded = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        match chars.next().ok_or(DataTextEscapeError)? {
            '\\' => decoded.push('\\'),
            '"' => decoded.push('"'),
            'n' => decoded.push('\n'),
            'r' => decoded.push('\r'),
            't' => decoded.push('\t'),
            'x' => {
                let high = chars
                    .next()
                    .and_then(hex_digit)
                    .ok_or(DataTextEscapeError)?;
                let low = chars
                    .next()
                    .and_then(hex_digit)
                    .ok_or(DataTextEscapeError)?;
                let byte = (high << 4) | low;
                decoded.push(char::from(byte));
            }
            _ => return Err(DataTextEscapeError),
        }
    }
    Ok(decoded)
}

fn hex_digit(ch: char) -> Option<u8> {
    ch.to_digit(16).and_then(|digit| u8::try_from(digit).ok())
}

#[cfg(test)]
mod tests {
    use super::{decode_data_text_escapes, encode_data_text_string};

    #[test]
    fn control_bytes_round_trip_through_hex_escapes() {
        for value in [
            "plain",
            "a\\b\"c\nd\re\tf",
            "a\u{0}b\u{7}c\u{b}d\u{c}e\u{1b}f\u{7f}g",
            "caf\u{e9}",
        ] {
            let encoded = encode_data_text_string(value);
            let inner = encoded
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .expect("quoted");
            assert_eq!(
                decode_data_text_escapes(inner).unwrap(),
                value,
                "round trip failed for {value:?} via {encoded:?}"
            );
        }
    }

    #[test]
    fn control_bytes_render_as_lowercase_hex_escapes() {
        assert_eq!(
            encode_data_text_string("\u{0}\u{7}\u{1b}\u{7f}"),
            r#""\x00\x07\x1b\x7f""#
        );
        // The five named escapes keep their canonical spelling, not `\xNN`.
        assert_eq!(
            encode_data_text_string("a\\b\"c\nd\re\tf"),
            r#""a\\b\"c\nd\re\tf""#
        );
        // Non-control non-ASCII stays literal.
        assert_eq!(encode_data_text_string("caf\u{e9}"), "\"caf\u{e9}\"");
    }

    #[test]
    fn decode_rejects_bad_and_truncated_escapes() {
        for bad in [r"\q", r"\u", r"\x", r"\x1", r"\xg0", r"ends\"] {
            assert_eq!(
                decode_data_text_escapes(bad),
                Err(super::DataTextEscapeError),
                "expected {bad:?} to be rejected"
            );
        }
    }
}
