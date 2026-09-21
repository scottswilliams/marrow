//! Store identity, writer provenance and persistent publication state.
//!
//! Only the complete versioned record can be encoded or decoded. Metadata alone
//! cannot imply Active. Lengths, discriminants, digest and trailing bytes are checked
//! strictly.

use marrow_image::{StoreEnvelopeDigest, StoreHeadDigest};

use crate::codec::{
    ARTIFACT_PREFIX_BYTES, FormatError, FormatField, MalformedReason, Reader, artifact_version,
    put_u32,
};
use crate::instance::StoreInstanceId;

/// The envelope magic: "MWSE" (Marrow Store Envelope).
const MAGIC: &[u8; 4] = b"MWSE";

/// The envelope container format version this build writes and reads.
const ENVELOPE_VERSION: u8 = 0x01;

/// The largest writer-toolchain-version string the envelope records, bounding the decode
/// allocation. A released toolchain version is a short semantic-version string well within
/// this.
const MAX_TOOLCHAIN_BYTES: u32 = 64;

/// The maximum envelope: magic, version, instance, the bounded toolchain string, engine
/// kind and format, one state byte, at most two head digests and the sealing digest.
/// Admission enforces it before allocation.
pub(crate) const MAX_ENVELOPE_FILE_BYTES: u64 = 30 + MAX_TOOLCHAIN_BYTES as u64 + 32 + 1 + 2 * 32;

/// The record ceiling for a recognized version, before allocating its body.
pub(crate) fn file_ceiling(prefix: &[u8; ARTIFACT_PREFIX_BYTES]) -> Result<u64, FormatError> {
    match artifact_version(prefix, MAGIC)? {
        ENVELOPE_VERSION => Ok(MAX_ENVELOPE_FILE_BYTES),
        found => Err(FormatError::UnknownVersion { found }),
    }
}

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
                field: FormatField::EngineKind,
            }),
        }
    }
}

/// Store identity and writer/engine provenance. Publication state is retained by
/// the private persisted record; this metadata alone cannot be written as a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreEnvelope {
    /// The store's instance identity.
    pub instance: StoreInstanceId,
    /// The declared toolchain version performing the envelope write.
    pub writer_toolchain: String,
    /// The ordered-byte engine kind the store is written over.
    pub engine_kind: EngineKind,
    /// Marrow's native engine representation stamp, independent of the backend's
    /// private physical file-format version.
    pub engine_format_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvelopeState {
    Active,
    Provision {
        head: StoreHeadDigest,
    },
    Rebind {
        old: StoreHeadDigest,
        new: StoreHeadDigest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnvelopeRecord {
    pub(crate) metadata: StoreEnvelope,
    pub(crate) state: EnvelopeState,
}

impl EnvelopeRecord {
    /// Encode bounded metadata and the complete publication state into one canonical body.
    fn body(&self) -> Result<Vec<u8>, FormatError> {
        let metadata = &self.metadata;
        if metadata.writer_toolchain.len() > MAX_TOOLCHAIN_BYTES as usize {
            return Err(FormatError::LengthOverflow {
                field: FormatField::WriterToolchain,
            });
        }
        let mut out = Vec::with_capacity(MAX_ENVELOPE_FILE_BYTES as usize);
        out.extend_from_slice(MAGIC);
        out.push(ENVELOPE_VERSION);
        out.extend_from_slice(metadata.instance.bytes());
        let toolchain = metadata.writer_toolchain.as_bytes();
        put_u32(&mut out, toolchain.len() as u32);
        out.extend_from_slice(toolchain);
        out.push(metadata.engine_kind.tag());
        put_u32(&mut out, metadata.engine_format_version);
        match self.state {
            EnvelopeState::Active => out.push(0),
            EnvelopeState::Provision { head } => {
                out.push(1);
                out.extend_from_slice(head.bytes());
            }
            EnvelopeState::Rebind { old, new } => {
                out.push(2);
                out.extend_from_slice(old.bytes());
                out.extend_from_slice(new.bytes());
            }
        }
        Ok(out)
    }

    /// The envelope's canonical bytes: its body followed by the 32-byte
    /// [`StoreEnvelopeDigest`] sealing that body.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, FormatError> {
        let mut out = self.body()?;
        let digest = StoreEnvelopeDigest::compute(&out);
        out.extend_from_slice(digest.bytes());
        Ok(out)
    }

    /// Decode an envelope from `bytes`, rejecting a bad magic, an unknown version, a
    /// toolchain string beyond its bound, an unknown engine kind, a digest that does not
    /// reseal the body, or trailing bytes. The digest is recomputed over the encoded body
    /// and compared, so an altered body or a swapped digest is a typed
    /// [`FormatError::DigestMismatch`].
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
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
                field: FormatField::WriterToolchain,
            });
        }
        let toolchain_bytes = reader.take_vec(toolchain_len as usize)?;
        let writer_toolchain =
            String::from_utf8(toolchain_bytes).map_err(|_| FormatError::Malformed {
                reason: MalformedReason::ToolchainNotUtf8,
            })?;
        let engine_kind = EngineKind::from_tag(reader.u8()?)?;
        let engine_format_version = reader.u32()?;
        let state = match reader.u8()? {
            0 => EnvelopeState::Active,
            1 => EnvelopeState::Provision {
                head: StoreHeadDigest::from_bytes(reader.array::<32>()?),
            },
            2 => EnvelopeState::Rebind {
                old: StoreHeadDigest::from_bytes(reader.array::<32>()?),
                new: StoreHeadDigest::from_bytes(reader.array::<32>()?),
            },
            _ => {
                return Err(FormatError::UnknownDiscriminant {
                    field: FormatField::PublicationState,
                });
            }
        };
        let sealed = reader.array::<32>()?;
        reader.finish()?;

        let metadata = StoreEnvelope {
            instance,
            writer_toolchain,
            engine_kind,
            engine_format_version,
        };
        if StoreEnvelopeDigest::from_bytes(sealed)
            != StoreEnvelopeDigest::compute(&bytes[..bytes.len() - 32])
        {
            return Err(FormatError::DigestMismatch);
        }
        Ok(Self { metadata, state })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EnvelopeRecord {
        EnvelopeRecord {
            metadata: StoreEnvelope {
                instance: StoreInstanceId::from_bytes([0x7A; 16]),
                writer_toolchain: "0.1.0".into(),
                engine_kind: EngineKind::Redb,
                engine_format_version: 1,
            },
            state: EnvelopeState::Active,
        }
    }

    #[test]
    fn envelope_round_trips_and_reseals() {
        let envelope = sample();
        let bytes = envelope.encode().expect("encode record");
        assert_eq!(
            EnvelopeRecord::decode(&bytes).expect("decode"),
            envelope,
            "the envelope round-trips through its own codec",
        );
    }

    #[test]
    fn every_current_state_is_preserved_and_the_maximum_is_exact() {
        let head = StoreHeadDigest::from_bytes([0x31; 32]);
        let new = StoreHeadDigest::from_bytes([0x32; 32]);
        for state in [
            EnvelopeState::Active,
            EnvelopeState::Provision { head },
            EnvelopeState::Rebind { old: head, new },
        ] {
            let mut record = sample();
            record.state = state;
            record.metadata.writer_toolchain = "v".repeat(64);
            let bytes = record.encode().expect("bounded record");
            assert!(bytes.len() as u64 <= MAX_ENVELOPE_FILE_BYTES);
            if matches!(state, EnvelopeState::Rebind { .. }) {
                assert_eq!(bytes.len() as u64, MAX_ENVELOPE_FILE_BYTES);
            }
            assert_eq!(EnvelopeRecord::decode(&bytes).expect("decode"), record);
            record.metadata.writer_toolchain.push('v');
            assert_eq!(
                record.encode(),
                Err(FormatError::LengthOverflow {
                    field: FormatField::WriterToolchain
                })
            );
        }
    }

    #[test]
    fn unknown_state_is_not_active_even_when_the_digest_matches() {
        let mut bytes = sample().encode().expect("record");
        bytes[35] = 0xff;
        let digest = StoreEnvelopeDigest::compute(&bytes[..36]);
        bytes[36..].copy_from_slice(digest.bytes());
        assert_eq!(
            EnvelopeRecord::decode(&bytes),
            Err(FormatError::UnknownDiscriminant {
                field: FormatField::PublicationState
            })
        );
    }

    /// The frozen envelope layout KAT: the exact body bytes and the sealing digest position,
    /// so the durability contract cannot drift silently.
    #[test]
    fn envelope_body_layout_is_frozen() {
        let envelope = sample();
        let bytes = envelope.encode().expect("encode record");
        // magic(4) + version(1) + instance(16) + toolchain_len(4) + "0.1.0"(5) + kind(1)
        // + engine_format(4) + active_state(1) = 36 body bytes, then a 32-byte digest.
        assert_eq!(bytes.len(), 36 + 32);
        assert_eq!(bytes[35], 0);
        assert_eq!(&bytes[0..4], b"MWSE");
        assert_eq!(bytes[4], ENVELOPE_VERSION);
        assert_eq!(&bytes[5..21], &[0x7A; 16]);
        assert_eq!(&bytes[21..25], &[0x00, 0x00, 0x00, 0x05]); // toolchain length 5
        assert_eq!(&bytes[25..30], b"0.1.0");
        assert_eq!(bytes[30], 0x01); // EngineKind::Redb
        assert_eq!(&bytes[31..35], &[0x00, 0x00, 0x00, 0x01]); // engine format version 1
        assert_eq!(
            &bytes[36..68],
            StoreEnvelopeDigest::compute(&bytes[0..36]).bytes(),
        );
    }

    #[test]
    fn decode_rejects_a_tampered_body() {
        let mut bytes = sample().encode().expect("encode record");
        bytes[5] ^= 0xFF; // flip an instance-id byte; the digest no longer reseals.
        assert_eq!(
            EnvelopeRecord::decode(&bytes),
            Err(FormatError::DigestMismatch)
        );
    }

    #[test]
    fn decode_rejects_an_unknown_version_and_bad_magic() {
        let mut bytes = sample().encode().expect("encode record");
        bytes[4] = 0x09;
        assert_eq!(
            EnvelopeRecord::decode(&bytes),
            Err(FormatError::UnknownVersion { found: 0x09 })
        );

        let mut bytes = sample().encode().expect("encode record");
        bytes[0] = b'X';
        assert_eq!(EnvelopeRecord::decode(&bytes), Err(FormatError::BadMagic));
    }

    #[test]
    fn decode_rejects_trailing_bytes_and_truncation() {
        let mut bytes = sample().encode().expect("encode record");
        bytes.push(0x00);
        assert_eq!(
            EnvelopeRecord::decode(&bytes),
            Err(FormatError::TrailingBytes)
        );

        let bytes = sample().encode().expect("encode record");
        assert_eq!(
            EnvelopeRecord::decode(&bytes[..bytes.len() - 1]),
            Err(FormatError::Truncated)
        );
    }

    #[test]
    fn decode_rejects_an_unknown_engine_kind() {
        let mut bytes = sample().encode().expect("encode record");
        // The engine-kind tag sits at body offset 30; flip it to an undefined discriminant,
        // then reseal so the digest passes and the discriminant check is what rejects.
        bytes[30] = 0x7F;
        let resealed = StoreEnvelopeDigest::compute(&bytes[0..36]);
        bytes[36..68].copy_from_slice(resealed.bytes());
        assert_eq!(
            EnvelopeRecord::decode(&bytes),
            Err(FormatError::UnknownDiscriminant {
                field: FormatField::EngineKind
            }),
        );
    }
}
