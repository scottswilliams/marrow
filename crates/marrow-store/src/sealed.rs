//! The sole source of a durable [`TreeStore`] handle.
//!
//! `TreeStore`'s on-disk constructors are private to this crate, so every durable
//! open runs through [`SealedStore::open`]. A caller names its intent with an
//! [`AccessMode`]; the engine open behind each mode is the store-integrity ladder
//! (regular-file guard, page-graph guard, format-version and shape checks, and — on a
//! read-only open — the committed-recoverable replay) that fails a damaged store closed
//! before a handle escapes. Structural verification of the live cells stays an explicit
//! post-open step the caller runs.

use std::ops::Deref;
use std::path::Path;

use crate::backend::StoreError;
use crate::tree::TreeStore;

/// How a caller intends to open a durable store on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Create the store when the body is absent, otherwise open it read-write.
    Create,
    /// Open an existing store read-write; an absent body is an error.
    Write,
    /// Open an existing store read-only.
    Read,
}

/// A durable store handle whose engine open has passed the store-integrity ladder.
///
/// This is the only way to obtain a native [`TreeStore`]: the path constructors are
/// crate-private, so a handle cannot be minted around the ladder. The handle derefs to
/// [`TreeStore`]; an owner that must hand the engine handle onward takes it out with
/// [`into_store`](Self::into_store).
pub struct SealedStore {
    store: TreeStore,
}

impl SealedStore {
    /// Open the durable store at `path` under `access`, running the engine's
    /// store-integrity ladder before the handle is returned.
    pub fn open(path: &Path, access: AccessMode) -> Result<Self, StoreError> {
        let store = match access {
            AccessMode::Create => TreeStore::open(path)?,
            AccessMode::Write => TreeStore::open_existing(path)?,
            AccessMode::Read => TreeStore::open_read_only(path)?,
        };
        Ok(Self { store })
    }

    /// Take ownership of the underlying store.
    pub fn into_store(self) -> TreeStore {
        self.store
    }
}

impl Deref for SealedStore {
    type Target = TreeStore;

    fn deref(&self) -> &TreeStore {
        &self.store
    }
}
