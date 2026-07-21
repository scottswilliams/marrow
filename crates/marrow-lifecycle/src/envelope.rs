//! The persistent store envelope: the store's own identity and the provenance of the
//! writer and engine that wrote it (FR01 §8, reservation R2).
//!
//! The envelope is a versioned, big-endian, length-prefixed container sealed by a
//! [`StoreEnvelopeDigest`]. It records, at provision and on every activation that rewrites
//! it: the envelope format version; the exact released toolchain that performed the write;
//! the engine tuple (engine kind + engine on-disk format version); and the store's own
//! nonforgeable [`StoreInstanceId`]. Its future readers are the floor-compatibility gate
//! (does this build read what wrote the store?), forensic attribution, and the typed
//! refusal of a store written by an unknown future writer — each a concrete reader of one
//! field, so the tuple earns its place under reservation minimalism.
//!
//! Decode is strict: the magic, the version, every length, and the sealed digest are all
//! checked, and trailing bytes reject. An unknown envelope version is a typed
//! [`FormatError::UnknownVersion`], never a best-effort decode.

use marrow_image::StoreEnvelopeDigest;

use crate::codec::{FormatError, Reader, put_u32};
use crate::instance::StoreInstanceId;

/// The envelope magic: "MWSE" (Marrow Store Envelope).
const MAGIC: &[u8; 4] = b"MWSE";

/// The envelope container format version this build writes and reads.
const ENVELOPE_VERSION: u8 = 0x00;

/// The largest writer-toolchain-version string the envelope records, bounding the decode
/// allocation. A released toolchain version is a short semantic-version string well within
/// this.
const MAX_TOOLCHAIN_BYTES: u32 = 64;

/// The ordered-byte engine a store is written over. A closed discriminant set: a byte
/// outside it is a typed [`FormatError::UnknownDiscriminant`], never a silent default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    /// The redb-backed native engine.
    Redb,
}

impl EngineKind {
    fn tag(self) -> u8 {
        match self {
            EngineKind::Redb => 0x01,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, FormatError> {
        match tag {
            0x01 => Ok(EngineKind::Redb),
            _ => Err(FormatError::UnknownDiscriminant {
                field: "engine kind",
            }),
        }
    }
}

/// The persisted store envelope. Rewritten atomically on provision and on every activation
/// that changes the writer/engine provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreEnvelope {
    /// The store's own nonforgeable instance identity.
    pub instance: StoreInstanceId,
    /// The exact released toolchain version that performed this write.
    pub writer_toolchain: String,
    /// The ordered-byte engine kind the store is written over.
    pub engine_kind: EngineKind,
    /// The engine's on-disk format version — redb's format generation for a redb store.
    pub engine_format_version: u32,
}

impl StoreEnvelope {
    /// The canonical body bytes the digest seals: magic, version, instance id, the
    /// length-prefixed writer toolchain, the engine kind tag, and the engine format
    /// version. The digest is computed over exactly these bytes and appended by
    /// [`Self::encode`].
    fn body(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(ENVELOPE_VERSION);
        out.extend_from_slice(self.instance.bytes());
        let toolchain = self.writer_toolchain.as_bytes();
        put_u32(&mut out, toolchain.len() as u32);
        out.extend_from_slice(toolchain);
        out.push(self.engine_kind.tag());
        put_u32(&mut out, self.engine_format_version);
        out
    }

    /// The envelope's canonical bytes: its body followed by the 32-byte
    /// [`StoreEnvelopeDigest`] sealing that body.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.body();
        let digest = StoreEnvelopeDigest::compute(&out);
        out.extend_from_slice(digest.bytes());
        out
    }

    /// Decode an envelope from `bytes`, rejecting a bad magic, an unknown version, a
    /// toolchain string beyond its bound, an unknown engine kind, a digest that does not
    /// reseal the body, or trailing bytes. The digest is recomputed over the decoded body
    /// and compared, so an altered body or a swapped digest is a typed
    /// [`FormatError::DigestMismatch`].
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut reader = Reader::new(bytes);
        reader.magic(MAGIC)?;
        let version = reader.u8()?;
        if version != ENVELOPE_VERSION {
            return Err(FormatError::UnknownVersion { found: version });
        }
        let instance = StoreInstanceId::from_bytes(reader.array::<16>()?);
        let toolchain_len = reader.u32()?;
        if toolchain_len > MAX_TOOLCHAIN_BYTES {
            return Err(FormatError::LengthOverflow {
                field: "writer toolchain",
            });
        }
        let toolchain_bytes = reader.take_vec(toolchain_len as usize)?;
        let writer_toolchain =
            String::from_utf8(toolchain_bytes).map_err(|_| FormatError::Malformed {
                reason: "writer toolchain is not valid UTF-8",
            })?;
        let engine_kind = EngineKind::from_tag(reader.u8()?)?;
        let engine_format_version = reader.u32()?;
        let sealed = reader.array::<32>()?;
        reader.finish()?;

        let envelope = Self {
            instance,
            writer_toolchain,
            engine_kind,
            engine_format_version,
        };
        if StoreEnvelopeDigest::from_bytes(sealed) != StoreEnvelopeDigest::compute(&envelope.body())
        {
            return Err(FormatError::DigestMismatch);
        }
        Ok(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StoreEnvelope {
        StoreEnvelope {
            instance: StoreInstanceId::from_bytes([0x7A; 16]),
            writer_toolchain: "0.1.0".into(),
            engine_kind: EngineKind::Redb,
            engine_format_version: 1,
        }
    }

    #[test]
    fn envelope_round_trips_and_reseals() {
        let envelope = sample();
        let bytes = envelope.encode();
        assert_eq!(
            StoreEnvelope::decode(&bytes).expect("decode"),
            envelope,
            "the envelope round-trips through its own codec",
        );
    }

    /// The frozen envelope layout KAT: the exact body bytes and the sealing digest position,
    /// so the durability contract cannot drift silently.
    #[test]
    fn envelope_body_layout_is_frozen() {
        let envelope = sample();
        let bytes = envelope.encode();
        // magic(4) + version(1) + instance(16) + toolchain_len(4) + "0.1.0"(5) + kind(1)
        // + engine_format(4) = 35 body bytes, then a 32-byte digest.
        assert_eq!(bytes.len(), 35 + 32);
        assert_eq!(&bytes[0..4], b"MWSE");
        assert_eq!(bytes[4], ENVELOPE_VERSION);
        assert_eq!(&bytes[5..21], &[0x7A; 16]);
        assert_eq!(&bytes[21..25], &[0x00, 0x00, 0x00, 0x05]); // toolchain length 5
        assert_eq!(&bytes[25..30], b"0.1.0");
        assert_eq!(bytes[30], 0x01); // EngineKind::Redb
        assert_eq!(&bytes[31..35], &[0x00, 0x00, 0x00, 0x01]); // engine format version 1
        assert_eq!(
            &bytes[35..67],
            StoreEnvelopeDigest::compute(&bytes[0..35]).bytes(),
        );
    }

    #[test]
    fn decode_rejects_a_tampered_body() {
        let mut bytes = sample().encode();
        bytes[5] ^= 0xFF; // flip an instance-id byte; the digest no longer reseals.
        assert_eq!(
            StoreEnvelope::decode(&bytes),
            Err(FormatError::DigestMismatch)
        );
    }

    #[test]
    fn decode_rejects_an_unknown_version_and_bad_magic() {
        let mut bytes = sample().encode();
        bytes[4] = 0x09;
        assert_eq!(
            StoreEnvelope::decode(&bytes),
            Err(FormatError::UnknownVersion { found: 0x09 })
        );

        let mut bytes = sample().encode();
        bytes[0] = b'X';
        assert_eq!(StoreEnvelope::decode(&bytes), Err(FormatError::BadMagic));
    }

    #[test]
    fn decode_rejects_trailing_bytes_and_truncation() {
        let mut bytes = sample().encode();
        bytes.push(0x00);
        assert_eq!(
            StoreEnvelope::decode(&bytes),
            Err(FormatError::TrailingBytes)
        );

        let bytes = sample().encode();
        assert_eq!(
            StoreEnvelope::decode(&bytes[..bytes.len() - 1]),
            Err(FormatError::Truncated)
        );
    }

    #[test]
    fn decode_rejects_an_unknown_engine_kind() {
        let mut bytes = sample().encode();
        // The engine-kind tag sits at body offset 30; flip it to an undefined discriminant,
        // then reseal so the digest passes and the discriminant check is what rejects.
        bytes[30] = 0x7F;
        let resealed = StoreEnvelopeDigest::compute(&bytes[0..35]);
        bytes[35..67].copy_from_slice(resealed.bytes());
        assert_eq!(
            StoreEnvelope::decode(&bytes),
            Err(FormatError::UnknownDiscriminant {
                field: "engine kind"
            }),
        );
    }
}
