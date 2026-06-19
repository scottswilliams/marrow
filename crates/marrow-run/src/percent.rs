//! RFC 3986 percent-encoding for the `std::text` URL helpers.
//!
//! The unreserved set (`A-Z`, `a-z`, `0-9`, `-`, `.`, `_`, `~`) stays literal;
//! every other byte is `%XX` with uppercase hex. Decoding accepts either hex
//! case, treats `+` as a literal plus rather than a space, and is the exact
//! inverse of [`encode`] on any UTF-8 input.

const UPPER_HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Percent-encode the UTF-8 bytes of `text`.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if is_unreserved(byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(UPPER_HEX_DIGITS[usize::from(byte >> 4)]));
            out.push(char::from(UPPER_HEX_DIGITS[usize::from(byte & 0x0f)]));
        }
    }
    out
}

/// Decode percent escapes, or `None` for a malformed escape or decoded bytes
/// that are not valid UTF-8.
pub fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let high = bytes.get(index + 1).copied().and_then(nibble)?;
            let low = bytes.get(index + 2).copied().and_then(nibble)?;
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn is_unreserved(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~')
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn round_trips_reserved_and_non_ascii() {
        for text in ["", "plain", "a b/c", "café — 文字", "100%~done.test_v-1"] {
            assert_eq!(decode(&encode(text)).as_deref(), Some(text));
        }
    }

    #[test]
    fn keeps_unreserved_literal_and_uppercases_hex() {
        assert_eq!(encode("a b/é"), "a%20b%2F%C3%A9");
    }

    #[test]
    fn plus_is_a_literal_plus() {
        assert_eq!(decode("a+b").as_deref(), Some("a+b"));
    }

    #[test]
    fn rejects_malformed_escapes_and_invalid_utf8() {
        assert_eq!(decode("%4"), None);
        assert_eq!(decode("%zz"), None);
        assert_eq!(decode("%ff"), None);
    }
}
