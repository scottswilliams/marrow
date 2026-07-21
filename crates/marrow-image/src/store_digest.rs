//! Domain-separated digests for the persistent store's durability contracts (F02a).
//!
//! A persistent store records a typed envelope and a typed logical active head, each
//! sealed by a digest over its canonical payload, and reserves a data-root digest slot
//! (FR01 §2) a later full-walk operation populates. Each is a distinct typed 32-byte
//! domain-separated SHA-256 (the identity rule): `SHA-256( kind ‖ u64_be(len(payload)) ‖
//! payload )`, minted through the shared [`frame_id`](crate::demand::frame_id) framing so
//! the same payload frames to distinct digests under distinct kinds. The mint lives here,
//! beside every other image identity, because `sha2` is this crate's only dependency and
//! the store owner (`marrow-lifecycle`) composes these into its envelope and head without
//! taking a hash dependency of its own.
//!
//! These are store durability identities, disjoint from the image identities
//! ([`ImageId`](crate::ImageId), [`DurableContractId`](crate::DurableContractId), and the
//! rest): the kind byte-strings are pairwise distinct, so no store digest can ever validate
//! image bytes or another store digest's payload.

use crate::demand::frame_id;

/// The digest kind of the persistent store envelope (writer/engine identity and the
/// store's own instance identity).
pub const STORE_ENVELOPE_KIND: &[u8] = b"marrow.store.env.v0";

/// The digest kind of the store's logical active head (the active binding, the reserved
/// sequencing/data-digest slots, and the head identity map).
pub const STORE_HEAD_KIND: &[u8] = b"marrow.store.head.v0";

/// The digest kind of the store's logical data-root digest (FR01 §2): the digest over the
/// canonical logical cell stream, reserved by F02a and populated only by a later full-walk
/// operation (audit/backup/restore at F04+).
pub const STORE_DATA_KIND: &[u8] = b"marrow.store.data.v0";

/// Render 32 identity bytes as their 64-character lowercase hex spelling, for diagnostics
/// and tests. Shared by the store digest newtypes below.
fn to_hex(bytes: &[u8; 32]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in bytes {
        hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
        hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
    }
    hex
}

macro_rules! store_digest {
    ($(#[$meta:meta])* $name:ident, $kind:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);

        impl $name {
            /// Compute the digest over `payload` under this contract's domain-separated kind.
            pub fn compute(payload: &[u8]) -> Self {
                Self(frame_id($kind, payload))
            }

            /// Reconstruct the digest from its 32 raw bytes — for a reader that decodes a
            /// persisted slot and independently recomputes to compare.
            pub fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            /// The 32 digest bytes.
            pub fn bytes(&self) -> &[u8; 32] {
                &self.0
            }

            /// The lowercase hex spelling of the digest, for diagnostics and tests.
            pub fn to_hex(self) -> String {
                to_hex(&self.0)
            }
        }
    };
}

store_digest! {
    /// The persistent store envelope digest (kind [`STORE_ENVELOPE_KIND`]).
    StoreEnvelopeDigest, STORE_ENVELOPE_KIND
}
store_digest! {
    /// The store logical-head digest (kind [`STORE_HEAD_KIND`]).
    StoreHeadDigest, STORE_HEAD_KIND
}
store_digest! {
    /// The store data-root digest (kind [`STORE_DATA_KIND`], FR01 §2). Reserved by F02a and
    /// populated only by a later full-walk operation; F02a never computes one.
    StoreDataDigest, STORE_DATA_KIND
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CEILING_KIND, DEMAND_SET_KIND, DURABLE_CONTRACT_KIND, EXPORT_ID_KIND, IMAGE_DIGEST_KIND,
        INTERFACE_ID_KIND, image_id,
    };

    /// The three store digest kinds domain-separate from one another and from every image
    /// identity kind: the same payload frames to a distinct digest under each. This is the
    /// identity-rule recurrence gate — a future kind reusing another's byte-string would
    /// collide two of these and fail here.
    #[test]
    fn store_digest_kinds_domain_separate() {
        let payload = b"the same canonical payload under every kind";

        let envelope = StoreEnvelopeDigest::compute(payload);
        let head = StoreHeadDigest::compute(payload);
        let data = StoreDataDigest::compute(payload);

        // The three store digests over one payload are pairwise distinct.
        assert_ne!(envelope.bytes(), head.bytes());
        assert_ne!(envelope.bytes(), data.bytes());
        assert_ne!(head.bytes(), data.bytes());

        // And each is distinct from the image byte digest over the same payload.
        let image = image_id(payload);
        assert_ne!(envelope.bytes(), &image.0);
        assert_ne!(head.bytes(), &image.0);
        assert_ne!(data.bytes(), &image.0);

        // Every kind byte-string is pairwise distinct from every other identity kind in the
        // crate: domain separation is exactly the distinctness of these strings.
        let kinds: [&[u8]; 9] = [
            STORE_ENVELOPE_KIND,
            STORE_HEAD_KIND,
            STORE_DATA_KIND,
            IMAGE_DIGEST_KIND,
            DURABLE_CONTRACT_KIND,
            EXPORT_ID_KIND,
            DEMAND_SET_KIND,
            CEILING_KIND,
            INTERFACE_ID_KIND,
        ];
        for (i, left) in kinds.iter().enumerate() {
            for (j, right) in kinds.iter().enumerate() {
                if i != j {
                    assert_ne!(left, right, "identity kinds must be pairwise distinct");
                }
            }
        }
    }

    /// The digest is a stable, deterministic function of the payload: two computations agree,
    /// a changed payload changes the digest, and a round trip through `from_bytes` preserves
    /// it. A frozen known-answer vector pins the exact bytes so the framing can never drift.
    #[test]
    fn store_data_digest_is_stable_and_reconstructible() {
        let digest = StoreDataDigest::compute(b"abc");
        assert_eq!(digest, StoreDataDigest::compute(b"abc"), "deterministic");
        assert_ne!(
            digest.bytes(),
            StoreDataDigest::compute(b"abd").bytes(),
            "a changed payload changes the digest",
        );
        assert_eq!(
            StoreDataDigest::from_bytes(*digest.bytes()),
            digest,
            "a round trip through from_bytes preserves the digest",
        );

        // Known-answer: SHA-256( "marrow.store.data.v0" ‖ u64_be(3) ‖ "abc" ), frozen.
        assert_eq!(
            digest.to_hex(),
            "f76b881a4ff4a3d209e1d5f9f81316196d90f12cfebcb934cebf8529dcda2c41",
            "the store data digest framing is frozen; a change here is a durability-format \
             break",
        );
    }
}
