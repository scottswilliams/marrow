//! The narrow ordered-byte seam the path kernel consumes.
//!
//! [`ByteEngine`] is the minimal public projection of the crate-private
//! [`Backend`](crate::backend::Backend): the operations the path kernel
//! (`marrow-kernel`) needs to lay out and traverse durable cells — point read,
//! write, prefix delete, forward `scan_after`, the transaction bracket, a pinned
//! read snapshot, and a write-access probe. The rich scan family and the value
//! prefix reader stay crate-private for the byte-engine lane (E00) to delete; the
//! kernel never sees them.
//!
//! The kernel keys physical cells itself and interprets no bytes here: this seam
//! orders opaque bytes and nothing more.

use crate::backend::{Backend, StoreError};
use crate::mem::MemStore;
#[cfg(feature = "native")]
use crate::redb::RedbStore;

/// The maximum entries a single [`ByteEngine::scan_after`] returns. The kernel
/// walks one cell at a time, so one is enough; the bound keeps a page from
/// materializing an unbounded subtree.
const SCAN_PAGE: usize = 64;

/// One ordered-byte cell: its key and value.
pub type Cell = (Vec<u8>, Vec<u8>);

/// The ordered-byte operations the path kernel consumes. Keys sort byte-wise, so
/// `scan_after` yields cells in ascending key order; the kernel's physical layout
/// relies on that order.
pub trait ByteEngine {
    /// The value stored at `key`, or `None`.
    fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError>;
    /// Store `value` at `key`, replacing any prior value.
    fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError>;
    /// Remove `key` and every key under it as a prefix.
    fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError>;
    /// The cells under `prefix` strictly after `cursor`, in ascending key order,
    /// up to a bounded page. An empty result means no such cell exists.
    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError>;
    /// Fail closed when the handle cannot accept writes.
    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError>;
    /// Open (or deepen) the write-transaction bracket.
    fn begin(&mut self) -> Result<(), StoreError>;
    /// Commit the outermost write-transaction bracket.
    fn commit(&mut self) -> Result<(), StoreError>;
    /// Abort the open write-transaction bracket, discarding its staged writes.
    fn rollback(&mut self) -> Result<(), StoreError>;
    /// Pin a consistent read view so a multi-call traversal observes one snapshot.
    fn begin_snapshot(&mut self) -> Result<(), StoreError>;
    /// Release the pinned read view.
    fn end_snapshot(&mut self);
}

/// Project the crate-private [`Backend`] onto the narrow public seam.
macro_rules! byte_engine_from_backend {
    ($ty:ty) => {
        impl ByteEngine for $ty {
            fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
                Backend::read(&self.0, key)
            }
            fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
                Backend::write(&mut self.0, key, value)
            }
            fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError> {
                Backend::delete(&mut self.0, prefix)
            }
            fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
                Ok(Backend::scan_after(&self.0, prefix, cursor, SCAN_PAGE)?.entries)
            }
            fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
                Backend::require_write_access(&self.0, op)
            }
            fn begin(&mut self) -> Result<(), StoreError> {
                Backend::begin(&mut self.0)
            }
            fn commit(&mut self) -> Result<(), StoreError> {
                Backend::commit(&mut self.0)
            }
            fn rollback(&mut self) -> Result<(), StoreError> {
                Backend::rollback(&mut self.0)
            }
            fn begin_snapshot(&mut self) -> Result<(), StoreError> {
                Backend::begin_snapshot(&mut self.0)
            }
            fn end_snapshot(&mut self) {
                Backend::end_snapshot(&mut self.0);
            }
        }
    };
}

/// An in-memory ordered-byte engine. The differential proving ground for the path
/// kernel; not durable across processes.
#[derive(Debug, Default)]
pub struct MemoryEngine(MemStore);

impl MemoryEngine {
    /// A fresh empty in-memory engine.
    pub fn new() -> Self {
        Self::default()
    }
}

byte_engine_from_backend!(MemoryEngine);

/// A redb-backed native ordered-byte engine, durable across processes.
#[cfg(feature = "native")]
pub struct NativeEngine(RedbStore);

#[cfg(feature = "native")]
impl NativeEngine {
    /// Open (creating if needed) a write-capable native store at `path`.
    pub fn open(path: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self(RedbStore::open(path)?))
    }

    /// Open an existing native store read-only, never creating the file.
    pub fn open_read_only(path: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self(RedbStore::open_read_only(path)?))
    }
}

#[cfg(feature = "native")]
byte_engine_from_backend!(NativeEngine);
