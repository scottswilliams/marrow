//! The single race-aware owner of the lock-root cross-check.
//!
//! The committed `marrow.lock` is the independent witness to durable identity: a store that
//! presents fewer of the lock's active accepted roots than the lock recorded has lost data to a
//! rollback or deletion. [`verify_store_roots_against_lock`](super::integrity::verify_store_roots_against_lock)
//! is the bare, race-blind comparison. A writer re-creating a removed store transiently presents
//! exactly that shortfall, so every driver that compares a store against the lock — the read-only
//! inspections, `doctor`, `backup`, `data recover`, a write-capable `run`, and `evolve apply` —
//! routes through this module instead of the bare witness, so a live re-creation race is
//! tolerated while a settled loss still fails closed.

use std::path::Path;

use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use super::integrity::verify_store_roots_against_lock;

/// Decide whether a store satisfies the roots its committed lock recorded, tolerating the live
/// store-recreation race the bare witness cannot distinguish from a settled loss.
///
/// The bare witness condemns a store presenting fewer committed roots than the persisted lock as a
/// rollback. The discriminator between a live recreation and a settled loss is the redb
/// cross-process write lock the writer holds across the whole creation, never the store's on-disk
/// shape:
///
/// - While the store file is briefly gone, after the old store was deleted and before the new one
///   is created, the caller resolves the store absent (`store` is `None`) and the wait below
///   watches for the file to reappear, evidence a writer is mid-re-creation.
/// - Once the file is present the writer holds the redb write flock from minting the store uid
///   through committing the first root in one continuous open, so a racing open is excluded as
///   `store.locked` and never reaches this witness. A present store the caller *did* open is
///   therefore not a live race: a uid-only store with no committed baseline is a settled crash
///   between the two creation transactions — a genuine loss that fails closed with no
///   shape-based tolerance.
///
/// A first run records no active root in the lock, so the witness never fires either way.
pub fn verify_lock_roots_tolerating_recreation(
    store: Option<&TreeStore>,
    store_path: Option<&Path>,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<(), StoreError> {
    let Err(error) = verify_store_roots_against_lock(store, lock) else {
        return Ok(());
    };
    // A present store the caller opened is past the flock the writer holds across the whole
    // creation, so its shortfall is a settled loss, never a live race. An absent store is a live
    // race only while its file reappears within the wait budget; a settled deletion never does.
    if store.is_none() && store_path.is_some_and(|path| wait_for_store_recreation(path, lock)) {
        return Ok(());
    }
    Err(error)
}

/// Wait briefly for an absent store to reappear because a writer is re-creating it. Only a
/// committed lock recording active roots can be contradicted by a shortfall, and only an actively
/// re-created store reappears, so when both hold this watches for the file to materialize,
/// matching the store layer's creation-race tolerance. A settled deletion has no writer, so the
/// file stays absent across the budget and the caller treats the absence as final.
pub fn wait_for_store_recreation(path: &Path, lock: Option<&marrow_catalog::CatalogLock>) -> bool {
    if !lock.is_some_and(marrow_catalog::CatalogLock::records_active_roots) {
        return false;
    }
    const CREATION_RACE_BACKOFF: [u64; 4] = [1, 2, 4, 8];
    for wait in CREATION_RACE_BACKOFF {
        std::thread::sleep(std::time::Duration::from_millis(wait));
        if !store_path_is_absent(path) {
            return true;
        }
    }
    false
}

/// Whether the store path holds no on-disk store. Only a `NotFound` stat is absent: a present
/// file, a symlink loop, a denied lookup, or any other stat error means the path is occupied by
/// something that must route to the store open and fail closed there, never be treated as a clean
/// recreation that reappeared. `Path::exists` collapses every stat error to absent, so this
/// inspects the link itself rather than following it.
pub fn store_path_is_absent(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}
