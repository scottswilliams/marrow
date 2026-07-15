//! The 32-byte identity carried in a wire message.
//!
//! Every stable identity that crosses the wire — a launch nonce, a session token,
//! an export identity, an interface identity — is 32 bytes, spelled as 64 lowercase
//! hexadecimal characters. The wire crate treats them all as opaque [`Id32`] byte
//! arrays; the runner maps them to and from the compiler's typed identities. The
//! crate stays free of any dependency on the identity-owning crates this way.

/// An opaque 32-byte wire identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id32([u8; 32]);

impl Id32 {
    /// Wrap 32 raw bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Id32(bytes)
    }

    /// The 32 raw bytes.
    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The 64-character lowercase hex spelling.
    pub fn to_hex(&self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            hex.push(nibble(byte >> 4));
            hex.push(nibble(byte & 0xf));
        }
        hex
    }

    /// Parse a 64-character lowercase-hex spelling, or `None` if it is not exactly
    /// that. Uppercase hex is rejected so an id has one spelling.
    pub fn from_hex(text: &str) -> Option<Id32> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, pair) in bytes.chunks_exact(2).enumerate() {
            out[i] = (lower_hex(pair[0])? << 4) | lower_hex(pair[1])?;
        }
        Some(Id32(out))
    }
}

fn nibble(value: u8) -> char {
    char::from_digit(u32::from(value), 16).expect("nibble is one hex digit")
}

/// A single lowercase-hex digit's value, rejecting uppercase and non-hex bytes.
fn lower_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::Id32;

    #[test]
    fn hex_round_trips() {
        let id = Id32::from_bytes([0xab; 32]);
        assert_eq!(id.to_hex(), "ab".repeat(32));
        assert_eq!(Id32::from_hex(&id.to_hex()), Some(id));
    }

    #[test]
    fn bad_hex_is_rejected() {
        assert_eq!(Id32::from_hex("00"), None); // too short
        assert_eq!(Id32::from_hex(&"AB".repeat(32)), None); // uppercase
        assert_eq!(Id32::from_hex(&"zz".repeat(32)), None); // non-hex
    }
}
