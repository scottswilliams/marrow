use std::collections::HashSet;
use std::io::Read;

use marrow_catalog::CatalogEntry;

/// Hands out random opaque 128-bit catalog ids (`cat_<32 lowercase hex>`), re-rolling
/// against the ids already in use. Ids are random rather than a monotonic counter so
/// branch-parallel work, which has no single coordinator, cannot collide when two
/// branches allocate identity for different entities and merge. A clash from a hand-edited
/// or badly merged catalog fails closed at `CatalogMetadata::validate()`, which the
/// proposal runs at check time.
pub(super) struct StableIdAllocator<E = OsCatalogIdEntropy> {
    used: HashSet<String>,
    entropy: E,
}

impl StableIdAllocator<OsCatalogIdEntropy> {
    pub(super) fn empty() -> Self {
        Self {
            used: HashSet::new(),
            entropy: OsCatalogIdEntropy,
        }
    }

    /// Seeds the in-use set from every recorded entry regardless of lifecycle, so a
    /// retired id is never handed back out to a new entity.
    pub(super) fn over(entries: &[CatalogEntry]) -> Self {
        Self {
            used: entries
                .iter()
                .map(|entry| entry.stable_id.clone())
                .collect(),
            entropy: OsCatalogIdEntropy,
        }
    }
}

impl<E: CatalogIdEntropy> StableIdAllocator<E> {
    pub(super) fn allocate(&mut self) -> String {
        loop {
            let id = catalog_id_from_bytes(self.entropy.next_id_bytes());
            if self.used.insert(id.clone()) {
                return id;
            }
        }
    }
}

pub(super) trait CatalogIdEntropy {
    fn next_id_bytes(&mut self) -> [u8; 16];
}

pub(super) struct OsCatalogIdEntropy;

impl CatalogIdEntropy for OsCatalogIdEntropy {
    fn next_id_bytes(&mut self) -> [u8; 16] {
        let mut bytes = [0; 16];
        fill_os_entropy(&mut bytes);
        bytes
    }
}

#[cfg(unix)]
fn fill_os_entropy(bytes: &mut [u8; 16]) {
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(bytes))
        .expect("catalog id allocation requires OS entropy");
}

#[cfg(not(unix))]
fn fill_os_entropy(_bytes: &mut [u8; 16]) {
    panic!("catalog id allocation requires an approved OS entropy source on this platform");
}

fn catalog_id_from_bytes(bytes: [u8; 16]) -> String {
    let mut id = String::with_capacity("cat_".len() + 32);
    id.push_str("cat_");
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut id, "{byte:02x}").expect("writing to a string cannot fail");
    }
    id
}

#[cfg(test)]
mod tests {
    use std::collections::{HashSet, VecDeque};

    use super::{CatalogIdEntropy, StableIdAllocator, catalog_id_from_bytes};

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
        fn next_id_bytes(&mut self) -> [u8; 16] {
            self.ids.pop_front().expect("scripted entropy exhausted")
        }
    }

    fn with_entropy<E: CatalogIdEntropy>(
        used: HashSet<String>,
        entropy: E,
    ) -> StableIdAllocator<E> {
        StableIdAllocator { used, entropy }
    }

    #[test]
    fn stable_id_allocator_retries_forced_entropy_collisions() {
        let collision = [0x11; 16];
        let unique = [0x22; 16];
        let mut used = HashSet::new();
        used.insert(catalog_id_from_bytes(collision));
        let mut allocator = with_entropy(used, ScriptedEntropy::new([collision, unique]));

        assert_eq!(catalog_id_from_bytes(unique), allocator.allocate());
    }
}
