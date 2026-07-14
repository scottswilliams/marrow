//! The `ImageId` integrity digest (design §C).
//!
//! `image_id = SHA-256( kind ‖ len ‖ payload )` with `kind` the 15 ASCII bytes
//! `marrow.image.v0`, `len` the big-endian `u64` byte length of `payload`, and
//! `payload` every image byte after the digest slot. This is an integrity
//! identity, not compiler authentication: anyone can mint a valid digest, so trust
//! comes from verification, never from the hash.

use sha2::{Digest, Sha256};

/// The domain-separation tag for the image digest.
pub const IMAGE_DIGEST_KIND: &[u8; 15] = b"marrow.image.v0";

/// A 32-byte program-image digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageId(pub [u8; 32]);

impl ImageId {
    /// The lowercase hex spelling of the digest.
    pub fn to_hex(self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }
}

/// Compute the domain-separated image digest over `payload` (every image byte
/// after the 32-byte digest slot).
pub fn image_id(payload: &[u8]) -> ImageId {
    let mut hasher = Sha256::new();
    hasher.update(IMAGE_DIGEST_KIND);
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(payload);
    ImageId(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::{IMAGE_DIGEST_KIND, image_id};

    #[test]
    fn digest_kind_is_fifteen_bytes() {
        assert_eq!(IMAGE_DIGEST_KIND.len(), 15);
    }

    /// Known-answer for the digest construction: an empty payload hashes
    /// `kind ‖ 0u64 ‖ <nothing>`. Freezing this pins the domain-separation layout
    /// so a later reader can reconstruct it independently.
    #[test]
    fn empty_payload_digest_known_answer() {
        // SHA-256 of the 15 kind bytes followed by eight zero length bytes.
        assert_eq!(
            image_id(&[]).to_hex(),
            "26c7fe78e40a5df727f096f8cd4a66860cb29b6b6f61bf45c9d34a8fca6efc51",
            "digest must be SHA-256(kind || len || payload)"
        );
    }
}
