use std::collections::HashSet;
use std::io::{self, Read};

use marrow_catalog::{CatalogEntry, LockLedgerTombstone};

use crate::hex::push_lower_hex;

/// Hands out random opaque 128-bit catalog ids (`cat_<32 lowercase hex>`), re-rolling
/// against the ids it must never reuse. Ids are random rather than a monotonic counter so
/// branch-parallel work, which has no single coordinator, cannot collide when two branches
/// allocate identity for different entities and merge. A clash from a hand-edited or badly
/// merged catalog fails closed at `CatalogMetadata::validate()`, which the proposal runs at
/// check time.
///
/// The never-reuse set is seeded from the lock's complete append-only id ledger — its
/// reserved and retired tombstones — which is the durable authority for ids that must never
/// be reissued, even across store loss when no surviving entry carries them. Live proposal
/// entries contribute too, but only the ledger guarantees a retired id stays dead.
pub(super) struct StableIdAllocator<E = OsCatalogIdEntropy> {
    used: HashSet<String>,
    entropy: E,
}

impl StableIdAllocator<OsCatalogIdEntropy> {
    /// Seeds the never-reuse set from the lock id ledger unioned with the in-memory proposal
    /// entries, so an id retired into a tombstone is never handed back out even when no
    /// surviving entry still carries it.
    pub(super) fn over(ledger: &[LockLedgerTombstone], entries: &[CatalogEntry]) -> Self {
        Self {
            used: seed_never_reuse(ledger, entries),
            entropy: OsCatalogIdEntropy,
        }
    }
}

/// The union of every id the lock ledger records (its reserved and retired tombstones) with
/// the ids of the supplied proposal entries — the complete set of ids `allocate()` must never
/// reissue.
fn seed_never_reuse(ledger: &[LockLedgerTombstone], entries: &[CatalogEntry]) -> HashSet<String> {
    ledger
        .iter()
        .map(|tombstone| tombstone.id.clone())
        .chain(entries.iter().map(|entry| entry.stable_id.clone()))
        .collect()
}

#[cfg(test)]
impl<E> StableIdAllocator<E> {
    pub(super) fn with_entropy(used: HashSet<String>, entropy: E) -> Self {
        Self { used, entropy }
    }
}

impl<E: CatalogIdEntropy> StableIdAllocator<E> {
    pub(super) fn allocate(&mut self) -> io::Result<String> {
        loop {
            let id = catalog_id_from_bytes(self.entropy.next_id_bytes()?);
            if self.used.insert(id.clone()) {
                return Ok(id);
            }
        }
    }
}

pub(super) trait CatalogIdEntropy {
    fn next_id_bytes(&mut self) -> io::Result<[u8; 16]>;
}

pub(super) struct OsCatalogIdEntropy;

impl CatalogIdEntropy for OsCatalogIdEntropy {
    fn next_id_bytes(&mut self) -> io::Result<[u8; 16]> {
        let mut bytes = [0; 16];
        fill_os_entropy(&mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(unix)]
fn fill_os_entropy(bytes: &mut [u8; 16]) -> io::Result<()> {
    std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(bytes))
}

#[cfg(not(unix))]
fn fill_os_entropy(_bytes: &mut [u8; 16]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "catalog id allocation requires an approved OS entropy source on this platform",
    ))
}

fn catalog_id_from_bytes(bytes: [u8; 16]) -> String {
    let mut id = String::with_capacity("cat_".len() + 32);
    id.push_str("cat_");
    push_lower_hex(&mut id, &bytes);
    id
}

#[cfg(test)]
mod tests {
    use std::collections::{HashSet, VecDeque};
    use std::io;

    use marrow_catalog::{CatalogLifecycle, LockLedgerTombstone};

    use super::{CatalogIdEntropy, StableIdAllocator, catalog_id_from_bytes, seed_never_reuse};

    struct ScriptedEntropy {
        ids: VecDeque<[u8; 16]>,
    }

    impl ScriptedEntropy {
        fn new(ids: impl IntoIterator<Item = [u8; 16]>) -> Self {
            Self {
                ids: ids.into_iter().collect(),
            }
        }
    }

    impl CatalogIdEntropy for ScriptedEntropy {
        fn next_id_bytes(&mut self) -> io::Result<[u8; 16]> {
            self.ids
                .pop_front()
                .ok_or_else(|| io::Error::other("scripted entropy exhausted"))
        }
    }

    struct FailingEntropy;

    impl CatalogIdEntropy for FailingEntropy {
        fn next_id_bytes(&mut self) -> io::Result<[u8; 16]> {
            Err(io::Error::other("entropy unavailable"))
        }
    }

    #[test]
    fn catalog_id_from_bytes_uses_canonical_lowercase_text() {
        let id = catalog_id_from_bytes([
            0x00, 0x01, 0x02, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0xff, 0xa0, 0xb0,
            0xc0, 0xd0,
        ]);

        assert_eq!(id, "cat_000102090a0b0c0d0e0f10ffa0b0c0d0");
    }

    #[test]
    fn stable_id_allocator_retries_forced_entropy_collisions() {
        let collision = [0x11; 16];
        let unique = [0x22; 16];
        let mut used = HashSet::new();
        used.insert(catalog_id_from_bytes(collision));
        let mut allocator =
            StableIdAllocator::with_entropy(used, ScriptedEntropy::new([collision, unique]));
        let expected = catalog_id_from_bytes(unique);
        let allocated = allocator.allocate();

        assert!(matches!(allocated.as_deref(), Ok(id) if id == expected));
    }

    #[test]
    fn stable_id_allocator_returns_entropy_errors() {
        let mut allocator = StableIdAllocator::with_entropy(HashSet::new(), FailingEntropy);
        let error = allocator.allocate();

        assert_eq!(
            error.as_ref().map_err(|error| error.kind()),
            Err(io::ErrorKind::Other)
        );
    }

    fn tombstone(bytes: [u8; 16], high_water: u64) -> LockLedgerTombstone {
        LockLedgerTombstone {
            kind: marrow_catalog::CatalogEntryKind::ResourceMember,
            path: "books::Book::retired".to_string(),
            id: catalog_id_from_bytes(bytes),
            lifecycle: CatalogLifecycle::Reserved,
            high_water,
        }
    }

    #[test]
    fn stable_id_allocator_never_reissues_a_tombstoned_ledger_id() {
        let tombstoned = [0x33; 16];
        let unique = [0x44; 16];
        let ledger = [tombstone(tombstoned, 7)];

        // The retired id survives only as a ledger tombstone: no surviving entry carries it.
        let seed = seed_never_reuse(&ledger, &[]);
        assert!(
            seed.contains(&catalog_id_from_bytes(tombstoned)),
            "the never-reuse seed must include a tombstone-only id"
        );

        let mut allocator =
            StableIdAllocator::with_entropy(seed, ScriptedEntropy::new([tombstoned, unique]));
        let allocated = allocator.allocate();

        assert!(
            matches!(allocated.as_deref(), Ok(id) if id == catalog_id_from_bytes(unique)),
            "allocate() must re-roll past the tombstoned id and return the unique one"
        );
    }
}
