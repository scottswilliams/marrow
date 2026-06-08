use std::collections::HashSet;
use std::io::Read;

use marrow_project::CatalogEntry;

/// Hands out catalog ids in the `cat_<32 lowercase hex>` shape as random opaque
/// 128-bit values, re-rolling against the ids already in use. Allocation is
/// independent of the entity's source path, so an id never changes when a path
/// changes, and it is random rather than a monotonic counter so two project
/// branches that each allocate identity for different entities cannot collide on
/// one id when they merge — a monotonic sequence is only safe with a single
/// coordinator, which branch-parallel work has none of. An id is frozen the moment
/// the catalog is committed and never recomputed afterward. The vanishingly rare
/// random clash (or a hand-edited or badly merged catalog) is not silently
/// tolerated: `CatalogMetadata::validate()` rejects two entries sharing a stable id,
/// and the proposal is validated at check, so a duplicate fails closed there.
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

    /// Seed the in-use set from every recorded entry regardless of lifecycle, so a
    /// retired or deprecated id is never handed back out to a new entity.
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
    #[cfg(test)]
    fn with_entropy(used: HashSet<String>, entropy: E) -> Self {
        Self { used, entropy }
    }

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

    #[test]
    fn stable_id_allocator_retries_forced_entropy_collisions() {
        let collision = [0x11; 16];
        let unique = [0x22; 16];
        let mut used = HashSet::new();
        used.insert(catalog_id_from_bytes(collision));
        let mut allocator =
            StableIdAllocator::with_entropy(used, ScriptedEntropy::new([collision, unique]));

        assert_eq!(catalog_id_from_bytes(unique), allocator.allocate());
    }
}
