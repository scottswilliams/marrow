//! The saved-tree backend contract.
//!
//! Every backend serves the same ordered-tree operations over encoded saved
//! paths (docs/implementation.md) — read, write, delete, presence, child keys,
//! bounded scan, and roots — plus transaction control modelled as a savepoint
//! stack (`begin`/`commit`/`rollback`, with nested `begin`s as savepoints).
//! [`MemStore`](crate::mem::MemStore) is the in-memory implementor; a persistent
//! backend implements the same contract.
//!
//! Reads return owned bytes so a persistent backend can serve them from a
//! transaction guard, and every operation is fallible: a persistent store can
//! meet I/O and corruption errors the in-memory store never does.

use crate::mem::{Presence, ScanPage, StoreError};
use crate::path::ChildSegment;

/// The operations every Marrow saved-tree store provides over encoded paths.
pub trait Backend {
    /// The exact value at `path`, or `None` when no value is stored there.
    fn read(&self, path: &[u8]) -> Result<Option<Vec<u8>>, StoreError>;
    /// Write `value` at `path`, replacing any value already there.
    fn write(&mut self, path: &[u8], value: Vec<u8>) -> Result<(), StoreError>;
    /// Remove the value at `path` and every value below it.
    fn delete(&mut self, path: &[u8]) -> Result<(), StoreError>;
    /// Whether `path` holds a value, children, both, or neither.
    fn presence(&self, path: &[u8]) -> Result<Presence, StoreError>;
    /// The distinct immediate children directly below `path`, in Marrow order.
    fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError>;
    /// Up to `limit` (path, value) pairs in the subtree at `path`, in Marrow
    /// order, including the value at `path` itself when present.
    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError>;
    /// The distinct saved root names, in Marrow order.
    fn roots(&self) -> Result<Vec<String>, StoreError>;

    /// Open a savepoint. Nested `begin`s stack; writes after it stay visible to
    /// reads (read-your-writes) until the matching `commit` or `rollback`.
    fn begin(&mut self) -> Result<(), StoreError>;
    /// Discard the innermost savepoint, keeping its writes (a normal exit). With
    /// no open savepoint this is a no-op.
    fn commit(&mut self) -> Result<(), StoreError>;
    /// Roll back to the innermost savepoint, discarding its writes. With no open
    /// savepoint this is a no-op.
    fn rollback(&mut self) -> Result<(), StoreError>;
}
