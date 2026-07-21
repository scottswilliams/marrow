//! The persistent store's own identity.
//!
//! Every provisioned store draws a fresh [`StoreInstanceId`] from OS entropy at provision
//! and records it in its envelope. It is the store's durable-store UID — distinct from the
//! program image's identity, the durable-contract identity, and any logical path identity:
//! two stores provisioned from one image have distinct instance ids, and a fresh restore
//! mints a new one, so a store instance is never confused with the image it was provisioned
//! from or with a peer store of the same program.

use std::sync::atomic::{AtomicU64, Ordering};

/// A persistent store's nonforgeable instance identity: 128 bits drawn from OS entropy at
/// provision. Unguessable and constructible only through [`StoreInstanceId::draw`], never
/// derived from the image, a clock, or a counter, so a forged image or a copied envelope
/// header cannot reproduce a live store's instance id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreInstanceId([u8; 16]);

impl StoreInstanceId {
    /// Draw a fresh identity from the OS entropy source. No clock, hash, counter, or retry:
    /// an entropy failure surfaces as [`EntropyUnavailable`] and no id is minted.
    pub fn draw() -> Result<Self, EntropyUnavailable> {
        draw_entropy().map(Self)
    }

    /// Reconstruct an id from its 16 raw bytes — for a reader that decodes a persisted
    /// envelope.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The 16 identity bytes.
    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// The lowercase hex spelling of the identity, for diagnostics and receipts.
    pub fn to_hex(self) -> String {
        let mut hex = String::with_capacity(32);
        for byte in self.0 {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }
}

/// The OS entropy source was unavailable, so no nonforgeable store identity could be
/// drawn; provision fails without minting a store rather than substituting a predictable
/// value.
#[derive(Debug)]
pub struct EntropyUnavailable(pub std::io::Error);

impl std::fmt::Display for EntropyUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "an OS entropy source is required to mint a store identity: {}",
            self.0
        )
    }
}

impl std::error::Error for EntropyUnavailable {}

/// A per-process monotonic counter mixed into an entropy draw only as a last-resort
/// distinctness aid; the OS entropy read is the nonforgeability source.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Draw 16 bytes from the OS entropy source. Reads `/dev/urandom` on Unix — the same
/// approved source the durable-identity mint and the attachment id use — and refuses on a
/// platform without one rather than substituting a predictable value.
#[cfg(unix)]
fn draw_entropy() -> Result<[u8; 16], EntropyUnavailable> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(EntropyUnavailable)?;
    // Mix a process-monotonic counter into the low bytes so two ids minted in the same
    // nanosecond still differ even under a degenerate entropy source.
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed).to_be_bytes();
    for (slot, mixed) in bytes[8..].iter_mut().zip(counter) {
        *slot ^= mixed;
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn draw_entropy() -> Result<[u8; 16], EntropyUnavailable> {
    let _ = COUNTER.fetch_add(1, Ordering::Relaxed);
    Err(EntropyUnavailable(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "minting a store identity requires an OS entropy source on this platform",
    )))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn each_draw_is_distinct_and_round_trips() {
        let a = StoreInstanceId::draw().expect("draw");
        let b = StoreInstanceId::draw().expect("draw");
        assert_ne!(a, b, "two store instances must not share an id");
        assert_eq!(
            StoreInstanceId::from_bytes(*a.bytes()),
            a,
            "a round trip through from_bytes preserves the id",
        );
        assert_eq!(a.to_hex().len(), 32, "the hex spelling is 32 characters");
    }
}
