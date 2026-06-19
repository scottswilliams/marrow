//! The one canonical lowercase hex codec, shared by the `std::bytes` hex
//! helpers and the deterministic UUID rendering in `std_id`.
//!
//! Encoding always emits lowercase digits; decoding accepts either case so it
//! is a true inverse of any conventional hex spelling.

const LOWER_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Encode bytes as lowercase hex.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for &byte in data {
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Decode hex text, or `None` for an odd length or any non-hex character.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        out.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
    }
    Some(out)
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
    fn round_trips_arbitrary_bytes() {
        for bytes in [
            Vec::new(),
            vec![0u8],
            vec![0xff, 0x00, 0x80],
            b"marrow".to_vec(),
        ] {
            assert_eq!(decode(&encode(&bytes)), Some(bytes));
        }
    }

    #[test]
    fn decodes_either_case() {
        assert_eq!(decode("ABcd"), Some(vec![0xab, 0xcd]));
    }

    #[test]
    fn rejects_odd_length_and_non_hex() {
        assert_eq!(decode("abc"), None);
        assert_eq!(decode("zz"), None);
    }
}
