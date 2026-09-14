//! The head identity map: the store-local bijection between a durable node's stable
//! 16-byte ledger id and a compact never-reused number (FR01 §3).
//!
//! Cell keys under the id-keyed layout are prefixed by a node's compact number rather than
//! its source spelling, so a rename is zero-cell metadata: the ledger id is unchanged, the
//! number is unchanged, and only the source anchor moves. The head pins this bijection as
//! part of the accepted schema state, committed atomically at activation. Numbers are never
//! reused within a store — a retired node's number retires with it, mirroring the ledger's
//! tombstone/high-water law — so the map carries a `next_number` high-water that a later
//! activation allocates fresh numbers from, above every number the store has ever used.
//!
//! The number is a `u32`, chosen for the store's lifetime headroom, and is deliberately
//! independent of the program image's `u16` table rings (FR01 §4): the store outlives
//! toolchains and its identifiers are never reused, so its number width is not the image's.
//! The map count is a separate `u32` store framing length bounded by [`MAX_HEAD_MAP_ENTRIES`]
//! before any allocation, so a hostile head can never drive an unbounded reservation.

use std::collections::HashSet;

use marrow_image::LedgerIdBytes;

use crate::codec::{FormatError, Reader, put_u32};

/// Maximum entries in one Head map, bounding decode allocation. This count limit matches
/// the kernel's `MAX_STORE_NODES`, enforced by a drift test. Physical addresses and the
/// high-water mark use an independent `u32` lifetime space.
pub const MAX_HEAD_MAP_ENTRIES: u32 = 1 << 16;

/// One binding in the head map: a durable node's ledger id and its compact store-local
/// number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadMapEntry {
    pub ledger_id: LedgerIdBytes,
    pub number: u32,
}

/// The head identity map: the current ledger-id ↔ number bindings plus the never-reused
/// high-water the next activation allocates from. Both directions are a total lookup, and
/// the bijection is enforced on construction and on decode (no ledger id or number appears
/// twice, and every number is below the high-water).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadMap {
    entries: Vec<HeadMapEntry>,
    next_number: u32,
}

impl HeadMap {
    /// Assign fresh contiguous numbers `0, 1, …` to `ledger_ids` in the given order — the
    /// deterministic pre-order numbering a fresh provision performs over the durable graph's
    /// nodes. The high-water is the count, so a later activation's fresh nodes start above
    /// every number this provision used. Rejects a duplicate ledger id
    /// ([`FormatError::Malformed`]) or a node count beyond [`MAX_HEAD_MAP_ENTRIES`].
    pub fn assign(ledger_ids: &[LedgerIdBytes]) -> Result<Self, FormatError> {
        if ledger_ids.len() as u64 > u64::from(MAX_HEAD_MAP_ENTRIES) {
            return Err(FormatError::LengthOverflow {
                field: "head map entries",
            });
        }
        let mut entries = Vec::with_capacity(ledger_ids.len());
        for (number, ledger_id) in ledger_ids.iter().enumerate() {
            entries.push(HeadMapEntry {
                ledger_id: *ledger_id,
                number: number as u32,
            });
        }
        let map = Self {
            next_number: entries.len() as u32,
            entries,
        };
        map.check_bijection()?;
        Ok(map)
    }

    /// The compact number bound to `ledger_id`, or `None` when the node is not in the map.
    pub fn number_of(&self, ledger_id: &LedgerIdBytes) -> Option<u32> {
        self.entries
            .iter()
            .find(|entry| entry.ledger_id.bytes() == ledger_id.bytes())
            .map(|entry| entry.number)
    }

    /// The ledger id bound to `number`, or `None` when no node holds it (a retired number).
    pub fn ledger_id_of(&self, number: u32) -> Option<LedgerIdBytes> {
        self.entries
            .iter()
            .find(|entry| entry.number == number)
            .map(|entry| entry.ledger_id)
    }

    /// The number of current bindings.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map is empty (a store with no durable nodes — never produced by a valid
    /// provision, which always has at least one root).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The high-water: the next number a future activation allocates, above every number the
    /// store has ever used (including retired ones).
    pub fn next_number(&self) -> u32 {
        self.next_number
    }

    /// Extend a proved additive graph from the next unused lifetime number. The
    /// lifecycle caller supplies only newly admitted occurrences; this owner keeps
    /// every old address and rejects duplicates, count overflow and exhaustion.
    pub(crate) fn extend(&self, additions: &[LedgerIdBytes]) -> Result<Self, FormatError> {
        let count = self
            .entries
            .len()
            .checked_add(additions.len())
            .filter(|&count| count <= MAX_HEAD_MAP_ENTRIES as usize)
            .ok_or(FormatError::LengthOverflow {
                field: "head map entries",
            })?;
        let added = u32::try_from(additions.len()).map_err(|_| FormatError::LengthOverflow {
            field: "head map lifetime numbers",
        })?;
        let next_number =
            self.next_number
                .checked_add(added)
                .ok_or(FormatError::LengthOverflow {
                    field: "head map lifetime numbers",
                })?;
        let mut entries = Vec::with_capacity(count);
        entries.extend_from_slice(&self.entries);
        for (offset, &ledger_id) in additions.iter().enumerate() {
            entries.push(HeadMapEntry {
                ledger_id,
                number: self.next_number + offset as u32,
            });
        }
        let result = Self {
            entries,
            next_number,
        };
        result.check_bijection()?;
        Ok(result)
    }

    /// The current bindings, in encoding order.
    pub fn entries(&self) -> &[HeadMapEntry] {
        &self.entries
    }

    /// Reject a map that reuses a ledger id or a number, or that binds a number at or above
    /// the high-water. This is the bijection-and-high-water invariant every construction and
    /// every decode passes. Linear in the entry count via two hash sets, so a hostile head at
    /// the [`MAX_HEAD_MAP_ENTRIES`] bound cannot drive a quadratic decode-time stall.
    fn check_bijection(&self) -> Result<(), FormatError> {
        let mut seen_numbers: HashSet<u32> = HashSet::with_capacity(self.entries.len());
        let mut seen_ids: HashSet<[u8; 16]> = HashSet::with_capacity(self.entries.len());
        for entry in &self.entries {
            if entry.number >= self.next_number {
                return Err(FormatError::Malformed {
                    reason: "head map number at or above the high-water",
                });
            }
            if !seen_numbers.insert(entry.number) {
                return Err(FormatError::Malformed {
                    reason: "head map reuses a number",
                });
            }
            if !seen_ids.insert(*entry.ledger_id.bytes()) {
                return Err(FormatError::Malformed {
                    reason: "head map reuses a ledger id",
                });
            }
        }
        Ok(())
    }

    /// Append the map's canonical bytes: the `u32` high-water, the `u32` entry count, then
    /// per entry the 16-byte ledger id and the `u32` number. Every integer is big-endian;
    /// the number's `u32` width is a frozen durability-contract byte (FR01 §3).
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        put_u32(out, self.next_number);
        put_u32(out, self.entries.len() as u32);
        for entry in &self.entries {
            out.extend_from_slice(entry.ledger_id.bytes());
            put_u32(out, entry.number);
        }
    }

    /// Decode a head map from `reader`, validating the entry count against
    /// [`MAX_HEAD_MAP_ENTRIES`] before allocating and enforcing the bijection-and-high-water
    /// invariant, so a hostile head cannot forge a reused number, a reused ledger id, or an
    /// unbounded allocation.
    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, FormatError> {
        let next_number = reader.u32()?;
        let count = reader.u32()?;
        if count > MAX_HEAD_MAP_ENTRIES {
            return Err(FormatError::LengthOverflow {
                field: "head map entries",
            });
        }
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let ledger_id = LedgerIdBytes::from_bytes(reader.array::<16>()?);
            let number = reader.u32()?;
            entries.push(HeadMapEntry { ledger_id, number });
        }
        let map = Self {
            entries,
            next_number,
        };
        map.check_bijection()?;
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_preserves_holes_and_allocates_from_next_unused() {
        let old = HeadMap {
            entries: vec![HeadMapEntry {
                ledger_id: id(1),
                number: 17,
            }],
            next_number: 90,
        };
        let extended = old.extend(&[id(2), id(3)]).expect("bounded extension");
        assert_eq!(extended.entries()[0], old.entries()[0]);
        assert_eq!(extended.number_of(&id(2)), Some(90));
        assert_eq!(extended.number_of(&id(3)), Some(91));
        assert_eq!(extended.next_number(), 92);
        assert!(old.extend(&[id(1)]).is_err());
        assert!(old.extend(&[id(2), id(2)]).is_err());
        let exhausted = HeadMap {
            next_number: u32::MAX,
            ..old
        };
        assert!(exhausted.extend(&[id(2)]).is_err());
        assert_eq!(exhausted.extend(&[]).expect("no allocation"), exhausted);
    }

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    /// The head map frames the same number space the kernel admits projections into: its
    /// entry bound is a hand-copy of the kernel's [`MAX_STORE_NODES`] and may not drift, or
    /// a store the kernel numbers could fail to persist its bijection.
    #[test]
    fn the_entry_bound_mirrors_the_kernel_number_space() {
        assert_eq!(
            MAX_HEAD_MAP_ENTRIES,
            marrow_kernel::durable::MAX_STORE_NODES
        );
    }

    #[test]
    fn assign_numbers_nodes_in_order_and_round_trips() {
        let map = HeadMap::assign(&[id(0x10), id(0x20), id(0x30)]).expect("assign");
        assert_eq!(map.len(), 3);
        assert_eq!(map.next_number(), 3);
        assert_eq!(map.number_of(&id(0x20)), Some(1));
        assert_eq!(map.ledger_id_of(2), Some(id(0x30)));
        assert_eq!(map.number_of(&id(0x99)), None);
        assert_eq!(map.ledger_id_of(9), None);

        let mut bytes = Vec::new();
        map.encode(&mut bytes);
        let mut reader = Reader::new(&bytes);
        let decoded = HeadMap::decode(&mut reader).expect("decode");
        reader.finish().expect("consumed exactly");
        assert_eq!(decoded, map, "the head map round-trips byte-for-byte");
    }

    /// The number is a `u32` (four bytes), independent of the image's `u16` rings: one
    /// entry encodes to exactly 4 (high-water) + 4 (count) + 16 (ledger id) + 4 (number) =
    /// 28 bytes, and the trailing four bytes are the number big-endian. This is the frozen
    /// head-map width KAT (FR01 §3/§4).
    #[test]
    fn head_map_number_width_is_u32_frozen() {
        let map = HeadMap::assign(&[id(0xAB)]).expect("assign");
        let mut bytes = Vec::new();
        map.encode(&mut bytes);
        assert_eq!(
            bytes,
            vec![
                0x00, 0x00, 0x00, 0x01, // high-water = 1
                0x00, 0x00, 0x00, 0x01, // one entry
                0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, // ledger id (16 bytes)
                0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, //
                0x00, 0x00, 0x00, 0x00, // number 0 as u32 big-endian
            ],
        );
        assert_eq!(bytes.len(), 28, "one entry is 28 bytes; the number is u32");
    }

    #[test]
    fn assign_rejects_a_duplicate_ledger_id() {
        assert_eq!(
            HeadMap::assign(&[id(0x01), id(0x01)]),
            Err(FormatError::Malformed {
                reason: "head map reuses a ledger id"
            }),
        );
    }

    #[test]
    fn decode_rejects_a_reused_number() {
        // Two entries both numbered 0, high-water 1 — a forged non-bijection.
        let mut bytes = Vec::new();
        put_u32(&mut bytes, 1); // high-water
        put_u32(&mut bytes, 2); // two entries
        bytes.extend_from_slice(&[0x01; 16]);
        put_u32(&mut bytes, 0);
        bytes.extend_from_slice(&[0x02; 16]);
        put_u32(&mut bytes, 0);
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            HeadMap::decode(&mut reader),
            Err(FormatError::Malformed {
                reason: "head map reuses a number"
            }),
        );
    }

    #[test]
    fn decode_rejects_a_number_at_or_above_the_high_water() {
        // One entry numbered 5 but a high-water of 1 — a stale-number forgery.
        let mut bytes = Vec::new();
        put_u32(&mut bytes, 1);
        put_u32(&mut bytes, 1);
        bytes.extend_from_slice(&[0x03; 16]);
        put_u32(&mut bytes, 5);
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            HeadMap::decode(&mut reader),
            Err(FormatError::Malformed {
                reason: "head map number at or above the high-water"
            }),
        );
    }

    #[test]
    fn decode_rejects_an_over_bound_count_before_allocating() {
        let mut bytes = Vec::new();
        put_u32(&mut bytes, 0);
        put_u32(&mut bytes, MAX_HEAD_MAP_ENTRIES + 1);
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            HeadMap::decode(&mut reader),
            Err(FormatError::LengthOverflow {
                field: "head map entries"
            }),
        );
    }
}
