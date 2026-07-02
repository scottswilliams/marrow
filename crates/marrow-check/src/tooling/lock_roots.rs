//! The single owner of the lock-root cross-check.
//!
//! The committed `marrow.lock` is the independent witness to durable identity: a PRESENT store
//! missing an active accepted root its own epoch covers has lost data to
//! a rollback or a torn baseline. [`verify_store_roots_against_lock`](super::integrity::verify_store_roots_against_lock)
//! is the store-vs-lock saved-root comparison, judging each committed root by its lock-recorded
//! activation epoch: a behind checkout legitimately lacks a root activated after its own epoch
//! (the store-behind fence's case, resolved by the advance paths), while a missing root the
//! store's epoch covers is a loss whatever the lock's high-water. An ABSENT store body is the
//! disposable-store case, not a loss: a fresh checkout or a deleted store seeds an empty store from
//! the committed identity. The
//! discriminator is present-and-damaged versus absent, so every driver that compares a store
//! against the lock — the read-only inspections, `doctor`, `backup`, `data recover`, a
//! write-capable `run`, `serve --write`, and `evolve apply` — routes through this module, which
//! condemns only a present shortfall.

use std::path::Path;

use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use super::integrity::verify_store_roots_against_lock;

/// Decide whether a store satisfies the roots its committed lock recorded.
///
/// Only a PRESENT store can lose committed roots and fail closed. An absent store body (`store`
/// is `None`) is the disposable-store case: a fresh checkout or a deleted store, which the write
/// paths seed an empty store from the committed identity for rather than failing closed. A first
/// run records no active root in the lock, so the witness never fires either way.
pub fn verify_present_store_lock_roots(
    store: Option<&TreeStore>,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<(), StoreError> {
    let Some(store) = store else {
        return Ok(());
    };
    verify_store_roots_against_lock(store, lock)
}

/// Whether the store path holds no on-disk store. Only a `NotFound` stat is absent: a present
/// file, a symlink loop, a denied lookup, or any other stat error means the path is occupied by
/// something that must route to the store open and fail closed there, never be treated as an
/// absent body the write paths seed. `Path::exists` collapses every stat error to absent, so this
/// inspects the link itself rather than following it.
pub fn store_path_is_absent(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}
