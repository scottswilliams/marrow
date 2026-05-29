//! The one canonical base64 codec, shared by the `std::bytes` builtins and the
//! `marrow serve` protocol so the two surfaces accept and reject exactly the
//! same inputs.
//!
//! This is standard RFC 4648 base64 with `+`/`/` and required `=` padding:
//! encoding always pads to a multiple of four, and decoding requires that same
//! canonical, fully-padded form. Decoding is deliberately strict — unpadded or
//! over-padded text is rejected — so the bytes a tool emits round-trip and no
//! second, laxer dialect can drift in.

/// The standard RFC 4648 base64 alphabet (with `+`/`/`; `=` is padding).
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes as standard, padded base64.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        out.push(ALPHABET[(bits >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(bits >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(bits >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Decode standard, padded base64, or `None` for malformed text (length not a
/// multiple of four, invalid characters, or `=` padding anywhere but the final
/// group).
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let group_count = bytes.len() / 4;
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let is_last = index + 1 == group_count;
        let pad_third = chunk[2] == b'=';
        let pad_fourth = chunk[3] == b'=';
        // `=` is allowed only in the final group, and a padded third byte forces a
        // padded fourth (`x=` alone is invalid).
        if (pad_third || pad_fourth) && !is_last {
            return None;
        }
        if pad_third && !pad_fourth {
            return None;
        }
        let third = if pad_third { 0 } else { value(chunk[2])? };
        let fourth = if pad_fourth { 0 } else { value(chunk[3])? };
        let bits = (value(chunk[0])? << 18) | (value(chunk[1])? << 12) | (third << 6) | fourth;
        out.push((bits >> 16) as u8);
        if !pad_third {
            out.push((bits >> 8) as u8);
        }
        if !pad_fourth {
            out.push(bits as u8);
        }
    }
    Some(out)
}

/// The 6-bit value of a base64 character, or `None` if it is not one.
fn value(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_arbitrary_bytes() {
        for bytes in [
            Vec::new(),
            vec![0u8],
            vec![1, 2],
            vec![1, 2, 3],
            b"foobar".to_vec(),
            vec![0, 255, 128, 64, 32],
        ] {
            assert_eq!(decode(&encode(&bytes)), Some(bytes));
        }
    }

    #[test]
    fn accepts_canonical_padded_forms() {
        assert_eq!(decode("Zm8="), Some(b"fo".to_vec()));
        assert_eq!(decode("Zg=="), Some(b"f".to_vec()));
        assert_eq!(decode("Zm9vYg=="), Some(b"foob".to_vec()));
    }

    #[test]
    fn rejects_non_canonical_padding() {
        // Unpadded and over-padded inputs are rejected: only the canonical,
        // fully-padded form decodes.
        for text in ["Zm8", "Zg", "Zm9vYg", "Zg===="] {
            assert_eq!(decode(text), None, "{text:?}");
        }
        // And so are genuinely invalid characters and misplaced padding.
        assert_eq!(decode("!!!!"), None);
        assert_eq!(decode("Z=m8"), None);
    }
}
