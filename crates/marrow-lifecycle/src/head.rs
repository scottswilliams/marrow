//! The store's logical active head: which program is active, the reserved sequencing and
//! data-digest slots, and the head identity map (FR01 §1/§2/§3/§8).
//!
//! The head is a versioned, big-endian, length-prefixed container sealed by a
//! [`StoreHeadDigest`]. It records the active binding — the active image's identity plus the
//! binding facts a binding-only rebind compares for exact equality — the reserved
//! `commit_position`, `data_digest`, and `data_digest_position` slots, and the
//! [`HeadMap`]. Provision writes the reserved slots as zero and maintains no commit
//! position: provision is not a population point (FR01 §2), so a later full-walk operation
//! is what turns sequencing on and populates the data digest under a head-version bump.
//!
//! Decode is strict: magic, version, every fixed field, the embedded head map's bounds and
//! bijection, the sealing digest, and no trailing bytes.

use marrow_image::StoreHeadDigest;

use crate::codec::{FormatError, Reader, put_u32, put_u64};
use crate::headmap::HeadMap;

/// The head magic: "MWSH" (Marrow Store Head).
const MAGIC: &[u8; 4] = b"MWSH";

/// The head container format version this build writes and reads. Version 1 records the
/// store's accepted deployment ceiling — the separately owned maximum authority the
/// atom-granular admission check enforces at attach (G03) — as a length-prefixed atom-set
/// payload after the head map. A future decision to populate the reserved digest slots at
/// provision, or to maintain the commit position from birth, bumps this version (FR01 §2).
const HEAD_VERSION: u8 = 0x01;

/// A fixed upper bound on the accepted-ceiling payload the head carries, validated before
/// allocation (campaign law 9). Comfortably above any real program's whole-demand atom-set
/// encoding and far below memory exhaustion.
const MAX_ACCEPTED_CEILING_BYTES: u32 = 4 * 1024 * 1024;

/// The active binding recorded in the head: the active image's byte identity plus the
/// binding-fact identities a binding-only rebind compares. The image id changes on any body
/// edit; the binding facts (`durable_contract`, `interface`) are what must be exactly equal
/// for a rebind to be legal — an equal-facts attach rebinds the active image with no user
/// action, any delta is a typed lifecycle refusal (not corruption). The
/// `image_format_version` rides here (FR01 §6), so a stale binding after a toolchain update
/// is a typed regenerate-and-rebind refusal, never a decode error.
///
/// Authority is *not* a binding fact here. The deployment ceiling is a separately owned
/// standing maximum recorded once at provision ([`LogicalHead::accepted_ceiling`]) and
/// enforced atom-granularly at attach: an image whose demand fits within the accepted
/// ceiling is admitted (even when its demand is narrower than a prior image's), and an image
/// whose demand exceeds it is refused naming the exceeding place and effect. So a demand
/// change within the ceiling is not a contract change, and a demand change beyond it is a
/// distinct, more actionable refusal than "the binding differs".
///
/// The binding-fact set is the subset with a concrete identity on this line; host-import and
/// dependency facts are reserved to join it when those identities exist, extending the
/// comparison without a head-format break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveBinding {
    /// The image container format version (rides the writer tuple, FR01 §6).
    pub image_format_version: u8,
    /// The active program image's byte identity. Changes on any body edit.
    pub image_id: [u8; 32],
    /// The durable-contract identity — the durable graph over ledger ids. A binding fact:
    /// a rebind requires it unchanged.
    pub durable_contract: [u8; 32],
    /// The interface identity — the exported call surface. A binding fact.
    pub interface: [u8; 32],
}

impl ActiveBinding {
    /// Whether `self` and `other` agree on every binding fact — the exact-equality test the
    /// binding-only rebind classification performs. The image id is deliberately excluded:
    /// a body-only edit changes it while preserving the durable contract, and that is
    /// exactly the rebind case. Authority (the accepted ceiling) is not a binding fact —
    /// it is a separately owned standing maximum enforced by the atom-granular admission
    /// check at attach, not by binding equality.
    pub fn facts_equal(&self, other: &ActiveBinding) -> bool {
        self.durable_contract == other.durable_contract && self.interface == other.interface
    }
}

/// The store's logical active head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalHead {
    /// The active binding: the active image and its binding facts.
    pub binding: ActiveBinding,
    /// The monotone confirmed-commit sequence position (FR01 §1 R1). Reserved: zero means
    /// unsequenced, and F02a maintains no position.
    pub commit_position: u64,
    /// The logical data-root digest (FR01 §2). Reserved: all zero until a later full-walk
    /// operation populates it under a head-version bump.
    pub data_digest: [u8; 32],
    /// The commit position the data digest was computed at (FR01 §2). Reserved: zero.
    pub data_digest_position: u64,
    /// The head identity map: the ledger-id ↔ number bijection (FR01 §3).
    pub head_map: HeadMap,
    /// The store's accepted deployment ceiling — the separately owned standing maximum
    /// authority — as a canonical [`marrow_image::ExportDemand`] atom-set payload
    /// (`marrow_image::CeilingDescriptor::atom_set_payload`). Written once at provision from
    /// the provisioning image's demand union and preserved verbatim across a binding-only
    /// rebind; the atom-granular admission check reconstructs it at attach with
    /// `CeilingDescriptor::from_payload` and refuses an image whose demand exceeds it. The
    /// whole head is digest-sealed, so a tampered ceiling payload rejects on decode.
    pub accepted_ceiling: Vec<u8>,
}

impl LogicalHead {
    /// The head a provision writes: the active binding, its head map, and the accepted
    /// deployment ceiling, with every reserved slot zero and no commit position maintained
    /// (FR01 §2 — provision is not a population point).
    pub fn provision(binding: ActiveBinding, accepted_ceiling: Vec<u8>, head_map: HeadMap) -> Self {
        Self {
            binding,
            commit_position: 0,
            data_digest: [0u8; 32],
            data_digest_position: 0,
            head_map,
            accepted_ceiling,
        }
    }

    /// The canonical body bytes the digest seals.
    fn body(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(HEAD_VERSION);
        out.push(self.binding.image_format_version);
        out.extend_from_slice(&self.binding.image_id);
        out.extend_from_slice(&self.binding.durable_contract);
        out.extend_from_slice(&self.binding.interface);
        put_u64(&mut out, self.commit_position);
        out.extend_from_slice(&self.data_digest);
        put_u64(&mut out, self.data_digest_position);
        self.head_map.encode(&mut out);
        put_u32(&mut out, self.accepted_ceiling.len() as u32);
        out.extend_from_slice(&self.accepted_ceiling);
        out
    }

    /// The head's canonical bytes: its body followed by the 32-byte [`StoreHeadDigest`]
    /// sealing that body.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.body();
        let digest = StoreHeadDigest::compute(&out);
        out.extend_from_slice(digest.bytes());
        out
    }

    /// Decode a head from `bytes`, rejecting a bad magic, an unknown version, an embedded
    /// head map beyond its bounds or violating its bijection, a digest that does not reseal
    /// the body, or trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let mut reader = Reader::new(bytes);
        reader.magic(MAGIC)?;
        let version = reader.u8()?;
        if version != HEAD_VERSION {
            return Err(FormatError::UnknownVersion { found: version });
        }
        let binding = ActiveBinding {
            image_format_version: reader.u8()?,
            image_id: reader.array::<32>()?,
            durable_contract: reader.array::<32>()?,
            interface: reader.array::<32>()?,
        };
        let commit_position = reader.u64()?;
        let data_digest = reader.array::<32>()?;
        let data_digest_position = reader.u64()?;
        // A v1 head carries no claim in the reserved slots: sequencing and the data digest are
        // turned on only under a later head-version bump (FR01 §2, coherence finding F-4).
        // Enforcing zero on read — not only on write — makes the incoherent "stale digest reads
        // current" state unrepresentable even for a validly-resealed forged head.
        if commit_position != 0 || data_digest != [0u8; 32] || data_digest_position != 0 {
            return Err(FormatError::Malformed {
                reason: "head reserves the sequencing and data-digest slots as zero",
            });
        }
        let head_map = HeadMap::decode(&mut reader)?;
        // The accepted deployment ceiling: a length-prefixed atom-set payload, bounded before
        // allocation. The payload's internal grammar is validated only when it is reconstructed
        // into a ceiling at attach (CeilingDescriptor::from_payload); here the head decode only
        // bounds and captures the bytes the digest seals.
        let ceiling_len = reader.u32()?;
        if ceiling_len > MAX_ACCEPTED_CEILING_BYTES {
            return Err(FormatError::LengthOverflow {
                field: "accepted_ceiling",
            });
        }
        let accepted_ceiling = reader.take_vec(ceiling_len as usize)?;
        let sealed = reader.array::<32>()?;
        reader.finish()?;

        let head = Self {
            binding,
            commit_position,
            data_digest,
            data_digest_position,
            head_map,
            accepted_ceiling,
        };
        if StoreHeadDigest::from_bytes(sealed) != StoreHeadDigest::compute(&head.body()) {
            return Err(FormatError::DigestMismatch);
        }
        Ok(head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marrow_image::LedgerIdBytes;

    fn binding() -> ActiveBinding {
        ActiveBinding {
            image_format_version: 0,
            image_id: [0x11; 32],
            durable_contract: [0x22; 32],
            interface: [0x33; 32],
        }
    }

    /// A sample accepted-ceiling payload. The head decode bounds and captures these bytes
    /// verbatim (their internal atom-set grammar is validated only when reconstructed into a
    /// ceiling at attach), so a fixed non-empty byte pattern exercises the length-prefixed
    /// section without depending on marrow-image's encoder.
    fn ceiling_payload() -> Vec<u8> {
        vec![0x44, 0x45, 0x46, 0x47]
    }

    fn head() -> LogicalHead {
        let map =
            HeadMap::assign(&[LedgerIdBytes::from_bytes([0x01; 16])]).expect("assign head map");
        LogicalHead::provision(binding(), ceiling_payload(), map)
    }

    #[test]
    fn head_round_trips_and_reseals() {
        let head = head();
        let bytes = head.encode();
        let decoded = LogicalHead::decode(&bytes).expect("decode");
        assert_eq!(decoded, head);
        assert_eq!(decoded.accepted_ceiling, ceiling_payload());
    }

    /// The head container version is 1: a head byte stream tagged v0 (the pre-G03 layout,
    /// which had no accepted-ceiling section) is refused, never best-effort decoded.
    #[test]
    fn decode_rejects_a_v0_head() {
        let mut bytes = head().encode();
        bytes[4] = 0x00; // the version byte, right after the 4-byte magic
        let body_len = bytes.len() - 32;
        let resealed = StoreHeadDigest::compute(&bytes[..body_len]);
        bytes[body_len..].copy_from_slice(resealed.bytes());
        assert_eq!(
            LogicalHead::decode(&bytes),
            Err(FormatError::UnknownVersion { found: 0 }),
        );
    }

    /// A tampered accepted-ceiling byte breaks the sealing digest: the head is digest-sealed
    /// over the whole body including the ceiling payload, so authority cannot be silently
    /// widened by editing the persisted ceiling.
    #[test]
    fn a_tampered_accepted_ceiling_breaks_the_digest() {
        let head = head();
        let mut bytes = head.encode();
        // The ceiling payload's first byte sits after: magic(4)+ver(1)+imgfmt(1)+3×id(32)
        // +commit(8)+ddig(32)+ddpos(8) + head_map + u32 ceiling_len. Rather than compute the
        // head-map length, flip the last payload byte, which is 32 (digest) + 0 back from the
        // digest start minus the ceiling length; simplest: flip the byte just before the digest.
        let ceiling_last = bytes.len() - 32 - 1;
        bytes[ceiling_last] ^= 0xFF;
        assert_eq!(LogicalHead::decode(&bytes), Err(FormatError::DigestMismatch));
    }

    /// The FR01 reserved slots are zero at provision: the commit position, the data digest,
    /// and the data-digest position each read back all-zero. Provision is not a population
    /// point, so these carry no claim (FR01 §2).
    #[test]
    fn provision_leaves_the_reserved_slots_zero() {
        let head = head();
        assert_eq!(head.commit_position, 0, "unsequenced at provision");
        assert_eq!(head.data_digest, [0u8; 32], "no data digest at provision");
        assert_eq!(
            head.data_digest_position, 0,
            "no digest position at provision"
        );

        // And the exact reserved bytes are zero in the encoding: after the three 32-byte
        // binding ids (image id, durable contract, interface), the commit position (8) + data
        // digest (32) + data-digest position (8) are all zero.
        let bytes = head.encode();
        let reserved_start = 6 + 32 * 3; // magic(4)+ver(1)+imgfmt(1)+3×id(32)
        let reserved = &bytes[reserved_start..reserved_start + 8 + 32 + 8];
        assert!(reserved.iter().all(|&b| b == 0), "reserved slots are zero");
    }

    /// The binding-fact equality excludes the image id: a body-only edit that changes the
    /// image id but preserves the durable contract, interface, and ceiling still compares
    /// equal — the binding-only rebind case — while any binding-fact delta compares unequal.
    #[test]
    fn binding_facts_equality_ignores_the_image_id_and_catches_a_fact_delta() {
        let a = binding();
        let mut body_edit = a;
        body_edit.image_id = [0x99; 32]; // a body-only edit changes only the image id.
        assert!(
            a.facts_equal(&body_edit),
            "a body-only edit preserves the facts"
        );

        let mut contract_change = a;
        contract_change.durable_contract = [0x99; 32];
        assert!(
            !a.facts_equal(&contract_change),
            "a durable-contract change is a binding-fact delta",
        );
    }

    /// A v0 head whose reserved slots are forged nonzero — and validly resealed so the digest
    /// passes — is rejected on decode (FR01 §2, coherence finding F-4): zero-ness is enforced
    /// on read, not only on write, so the incoherent "stale digest reads current" state is
    /// unrepresentable.
    #[test]
    fn decode_rejects_forged_nonzero_reserved_slots() {
        // The commit position sits right after the three 32-byte binding ids: offset
        // magic(4)+ver(1)+imgfmt(1)+3×32 = 102.
        let commit_pos_at = 6 + 32 * 3;
        for offset in [
            commit_pos_at,      // commit_position (first byte of the u64)
            commit_pos_at + 8,  // data_digest (first byte)
            commit_pos_at + 40, // data_digest_position (first byte, after the 32-byte digest)
        ] {
            let mut bytes = head().encode();
            bytes[offset] = 0x01; // forge a nonzero reserved byte
            // Reseal the body so the digest passes and the reserved-zero rule is the rejector.
            let body_len = bytes.len() - 32;
            let resealed = StoreHeadDigest::compute(&bytes[..body_len]);
            bytes[body_len..].copy_from_slice(resealed.bytes());
            assert_eq!(
                LogicalHead::decode(&bytes),
                Err(FormatError::Malformed {
                    reason: "head reserves the sequencing and data-digest slots as zero"
                }),
                "a forged nonzero reserved slot at offset {offset} must reject",
            );
        }
    }

    #[test]
    fn decode_rejects_a_tampered_head_and_trailing_bytes() {
        let mut bytes = head().encode();
        bytes[6] ^= 0xFF; // flip an image-id byte: the digest no longer reseals.
        assert_eq!(
            LogicalHead::decode(&bytes),
            Err(FormatError::DigestMismatch)
        );

        let mut bytes = head().encode();
        bytes.push(0x00);
        assert_eq!(LogicalHead::decode(&bytes), Err(FormatError::TrailingBytes));
    }

    #[test]
    fn decode_rejects_a_forged_head_map_non_bijection() {
        // Build a head whose embedded head map forges a reused number, then reseal so the
        // head-map bijection check — not the digest — is what rejects.
        let head = head();
        let mut bytes = head.encode();
        // The head map's entry count sits after the fixed prefix + high-water u32. Rather
        // than surgically forge, assert the whole-head decode still enforces the map's
        // bijection by corrupting the high-water to be below an existing number.
        // Fixed prefix: magic(4)+ver(1)+imgfmt(1)+3×id(32)+commit(8)+ddig(32)+ddpos(8) = 150.
        let map_start = 4 + 1 + 1 + 32 * 3 + 8 + 32 + 8;
        // The single entry is numbered 0 with high-water 1; set the high-water to 0 so the
        // number 0 is now at/above the high-water.
        bytes[map_start..map_start + 4].copy_from_slice(&0u32.to_be_bytes());
        // Reseal the body so the digest passes and the bijection check is the rejector.
        let body_len = bytes.len() - 32;
        let resealed = StoreHeadDigest::compute(&bytes[..body_len]);
        bytes[body_len..].copy_from_slice(resealed.bytes());
        assert_eq!(
            LogicalHead::decode(&bytes),
            Err(FormatError::Malformed {
                reason: "head map number at or above the high-water"
            }),
        );
    }
}
