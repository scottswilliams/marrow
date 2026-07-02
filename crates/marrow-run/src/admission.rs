//! Store admission: the second, program-aware stage above `marrow-store`'s sealed open.
//!
//! `marrow-store` mints a [`SealedStore`] whose engine open passed the store-integrity
//! ladder. Admission binds that sealed handle to a checked program's identity and the
//! committed `marrow.lock`, deciding whether the store may serve the program. Every
//! durable open in the runtime and CLI routes through this module, so `SealedStore` — and
//! `TreeStore`'s crate-private constructors behind it — appear nowhere else: a store handle
//! cannot reach a command without passing this boundary.
//!
//! The two stages are crate-aligned by necessity: `marrow-store` cannot see program
//! identity, and the program-aware ladder cannot reach into the store crate's private open.

use std::marker::PhantomData;
use std::path::Path;

use marrow_store::tree::TreeStore;
use marrow_store::{AccessMode, SealedStore, StoreError};

/// Read-only admission: the handle may serve reads but never commits.
pub enum Read {}

/// Write-capable admission: the handle may commit.
pub enum Write {}

/// A durable store handle that reached a command through admission.
///
/// The only public source of a runtime store handle. Its constructor is module-private, so
/// an `AdmittedStore` is proof the handle was opened through [`SealedStore`] rather than
/// around the store-integrity ladder. The access marker `A` records whether the open was
/// read-only or write-capable.
pub struct AdmittedStore<A> {
    store: TreeStore,
    access: PhantomData<A>,
}

impl<A> AdmittedStore<A> {
    fn new(store: TreeStore) -> Self {
        Self {
            store,
            access: PhantomData,
        }
    }

    /// Borrow the underlying store for reads, writes, and navigation.
    pub fn store(&self) -> &TreeStore {
        &self.store
    }

    /// Take ownership of the underlying store.
    pub fn into_store(self) -> TreeStore {
        self.store
    }
}

/// Open the durable store at `path` read-only.
pub fn open_read(path: &Path) -> Result<AdmittedStore<Read>, StoreError> {
    Ok(AdmittedStore::new(
        SealedStore::open(path, AccessMode::Read)?.into_store(),
    ))
}

/// Open an existing durable store write-capably; an absent body is an error.
pub fn open_write(path: &Path) -> Result<AdmittedStore<Write>, StoreError> {
    Ok(AdmittedStore::new(
        SealedStore::open(path, AccessMode::Write)?.into_store(),
    ))
}

/// Open the durable store at `path` write-capably, creating the body when it is absent.
pub fn open_create(path: &Path) -> Result<AdmittedStore<Write>, StoreError> {
    Ok(AdmittedStore::new(
        SealedStore::open(path, AccessMode::Create)?.into_store(),
    ))
}

/// Whether the store path holds no on-disk store. Only a `NotFound` stat is absent: a
/// present file, a symlink loop, a denied lookup, or any other stat error means the path is
/// occupied by something that must route to the store open and fail closed there, never be
/// treated as an absent body the write paths seed. `Path::exists` collapses every stat error
/// to absent, so this inspects the link itself rather than following it.
pub fn store_path_is_absent(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

/// Decide whether a store satisfies the roots its committed lock recorded.
///
/// The committed `marrow.lock` is the independent witness to durable identity: a PRESENT
/// store missing an active accepted root its own epoch covers has lost data to a rollback or
/// a torn baseline and is `store.corruption`. An ABSENT store body (`store` is `None`) is the
/// disposable-store case — a fresh checkout or a deleted store the write paths seed an empty
/// store from the committed identity for — and never fails closed. A first run records no
/// active root in the lock, so the witness never fires either way. Each committed root is
/// judged by its lock-recorded activation epoch, so a behind checkout legitimately lacks a
/// root activated after its own epoch (the store-behind fence's case) while a missing root
/// the store's epoch covers is a loss whatever the lock's high-water.
pub fn verify_present_store_lock_roots(
    store: Option<&TreeStore>,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<(), StoreError> {
    let Some(store) = store else {
        return Ok(());
    };
    marrow_check::tooling::verify_store_roots_against_lock(store, lock)
}
