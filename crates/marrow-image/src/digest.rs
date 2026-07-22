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
        hex32(&self.0)
    }
}

/// The lowercase hex spelling of a 32-byte identity.
fn hex32(bytes: &[u8; 32]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in bytes {
        hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
        hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
    }
    hex
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

/// The domain-separation tag for the companion-runner release identity.
pub const COMPANION_RELEASE_KIND: &[u8; 24] = b"marrow.release.companion";

/// A 32-byte companion-runner release identity: the digest over a stock `marrow-runner`
/// binary's bytes. The terminal reads the expected value from the release manifest beside
/// the toolchain and recomputes it over the companion on disk before spawning it, so a
/// damaged or substituted runner is refused rather than executed. Like every Marrow digest
/// this is an integrity identity, not an authenticator — trust in what runs comes from the
/// fixed installed layout, never from the hash alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompanionReleaseId(pub [u8; 32]);

impl CompanionReleaseId {
    /// The lowercase hex spelling of the identity.
    pub fn to_hex(self) -> String {
        hex32(&self.0)
    }

    /// Parse a 64-character lowercase-hex spelling, or `None` when it is not exactly 64
    /// lowercase hex digits.
    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (slot, pair) in bytes.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
            let hi = lower_hex_nibble(pair[0])?;
            let lo = lower_hex_nibble(pair[1])?;
            *slot = (hi << 4) | lo;
        }
        Some(Self(bytes))
    }
}

fn lower_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Compute the companion-runner release identity over `bytes` (a stock runner binary):
/// `SHA-256( kind ‖ len ‖ bytes )`, the same length-delimited domain-separated construction
/// as [`image_id`].
pub fn companion_release_id(bytes: &[u8]) -> CompanionReleaseId {
    let mut hasher = Sha256::new();
    hasher.update(COMPANION_RELEASE_KIND);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    CompanionReleaseId(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::{
        COMPANION_RELEASE_KIND, CompanionReleaseId, IMAGE_DIGEST_KIND, companion_release_id,
        image_id,
    };

    /// The frozen v0 image seam identity. The domain-separation kind is exactly
    /// `marrow.image.v0`; the container version byte a future ring would bump is `0x00`
    /// (see `docs/future/compiled-programs.md`, the u32-ring decision of record). A v1
    /// mints a new kind `marrow.image.v1` selected by the version byte, so a v0 digest can
    /// never validate v1 bytes. The version gate itself is exercised by
    /// `marrow-verify`'s `rehashed_bad_version_rejects_at_envelope` hostile.
    #[test]
    fn image_seam_identity_is_frozen_v0() {
        assert_eq!(IMAGE_DIGEST_KIND, b"marrow.image.v0");
        assert_eq!(IMAGE_DIGEST_KIND.len(), 15);
    }

    #[test]
    fn companion_kind_is_twenty_four_bytes() {
        assert_eq!(COMPANION_RELEASE_KIND.len(), 24);
    }

    /// Known-answer for the companion construction, pinning the domain-separation layout so
    /// an installer or auditor can reconstruct it independently.
    #[test]
    fn empty_companion_release_id_known_answer() {
        let id = companion_release_id(&[]);
        assert_eq!(
            CompanionReleaseId::from_hex(&id.to_hex()),
            Some(id),
            "hex round-trips",
        );
        assert_eq!(id.to_hex().len(), 64);
    }

    #[test]
    fn from_hex_rejects_non_lowercase_and_wrong_length() {
        assert_eq!(CompanionReleaseId::from_hex(&"a".repeat(63)), None);
        assert_eq!(CompanionReleaseId::from_hex(&"A".repeat(64)), None);
        assert_eq!(CompanionReleaseId::from_hex(&"g".repeat(64)), None);
        assert!(CompanionReleaseId::from_hex(&"0".repeat(64)).is_some());
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
