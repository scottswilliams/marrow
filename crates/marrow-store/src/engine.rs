//! The narrow ordered-byte engine contract the path kernel consumes.
//!
//! The seam is exactly three capabilities and nothing more:
//!
//! - [`ByteEngine::read_view`] hands out a lifetime-bound coherent [`ReadView`].
//!   Nothing can mutate the engine while a view borrows it, so the snapshot a
//!   view reads is stable for its whole life without an explicit pin/unpin pair.
//! - [`ByteEngine::begin`] consumes exclusive write access into a [`WriteTxn`].
//!   Because the transaction borrows the engine mutably, a second concurrent
//!   transaction cannot be named: nesting is unrepresentable, not merely rejected
//!   at runtime.
//! - [`ByteEngine::require_write_access`] reports whether the open handle admits
//!   writes at all, so a read-only handle mints a read-only ceiling.
//!
//! A [`WriteTxn`] is *consuming*: [`WriteTxn::commit`] takes `self` and reports a
//! [`CommitOutcome`], and dropping an uncommitted transaction aborts it. A
//! [`ReadView`] offers only reads, so mutation through a read capability is a
//! type error rather than a guarded call.
//!
//! Keys sort byte-wise; the single [`scan_after`](ReadView::scan_after) primitive
//! yields cells in ascending key order after a cursor, up to the batch limits in
//! [`limits`]. The engine orders opaque bytes and interprets none of them: the
//! logical key and value codecs that give bytes meaning are owned by the path
//! kernel (`marrow-kernel`).

use crate::error::StoreError;

/// Bounds every batch the engine returns or admits, so no operation allocates
/// unbounded work (campaign law 9). The values are engine policy; the kernel
/// keys and values it stores sit far below them.
pub(crate) mod limits {
    /// The largest key the engine will store. A key beyond this is a
    /// [`LimitExceeded`](crate::error::StoreError::LimitExceeded).
    pub(crate) const MAX_KEY_LEN: usize = 4096;
    /// The largest value the engine will store.
    pub(crate) const MAX_VALUE_LEN: usize = 1 << 20;
    /// The most cells one [`scan_after`](super::ReadView::scan_after) returns. The
    /// kernel walks one cell at a time, so a small page bounds a subtree walk.
    pub(crate) const SCAN_MAX_RECORDS: usize = 64;
    /// The most bytes one `scan_after` page accumulates before it stops early
    /// (always returning at least one cell so progress is guaranteed).
    pub(crate) const SCAN_MAX_AGGREGATE_BYTES: usize = 1 << 20;
}

/// One ordered-byte cell: its key and value.
pub type Cell = (Vec<u8>, Vec<u8>);

/// How a [`WriteTxn::commit`] resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOutcome {
    /// The transaction's writes are durably committed.
    Confirmed,
    /// The commit did not persist and the store is unchanged; retry is a fresh
    /// transaction, never a replay.
    Aborted,
    /// The commit's durability is unknown — it may or may not have persisted. The
    /// caller must close the store and reclassify on reopen rather than retry.
    Indeterminate,
}

/// A coherent read view. Bound to the engine borrow that produced it, so every
/// read it serves observes one consistent version.
pub trait ReadView {
    /// The value stored at `key`, or `None`.
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError>;
    /// The cells under `prefix` strictly after `cursor`, in ascending key order,
    /// up to [`limits::SCAN_MAX_RECORDS`] cells and
    /// [`limits::SCAN_MAX_AGGREGATE_BYTES`] bytes. An empty result means no cell
    /// under `prefix` sorts after `cursor`.
    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError>;
}

/// A write transaction. Reads observe its own staged writes ([`ReadView`]), and
/// it is consumed by [`commit`](WriteTxn::commit) or aborted by drop.
pub trait WriteTxn: ReadView {
    /// Stage `value` at `key`, replacing any prior value. A key or value beyond
    /// the [`limits`] is refused before it is staged.
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError>;
    /// Stage the removal of exactly `key`. Removing an absent key stages nothing
    /// and is not an error.
    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError>;
    /// Attempt to commit the staged writes, reporting whether they persisted.
    fn commit(self) -> CommitOutcome;
}

/// The ordered-byte engine. Its two implementors — an in-memory engine and a
/// redb-backed native engine — satisfy one conformance suite.
///
/// Two misuses the narrowed contract makes *unrepresentable* rather than
/// rejecting at runtime:
///
/// Nesting: a second transaction cannot be opened while one is live, because
/// [`begin`](ByteEngine::begin) borrows the engine mutably.
///
/// ```compile_fail
/// use marrow_store::{ByteEngine, MemoryEngine};
/// let mut engine = MemoryEngine::new();
/// let first = engine.begin().unwrap();
/// let second = engine.begin().unwrap(); // second mutable borrow of `engine`
/// drop((first, second));
/// ```
///
/// Mutation through a read capability: a [`ReadView`] exposes no `put` or
/// `remove`.
///
/// ```compile_fail
/// use marrow_store::{ByteEngine, MemoryEngine, WriteTxn};
/// let engine = MemoryEngine::new();
/// let view = engine.read_view().unwrap();
/// view.put(b"k", b"v".to_vec()).unwrap(); // no `put` on a read view
/// ```
pub trait ByteEngine {
    /// A coherent read view over this engine.
    type View<'a>: ReadView
    where
        Self: 'a;
    /// An exclusive write transaction over this engine.
    type Txn<'a>: WriteTxn
    where
        Self: 'a;

    /// Open a coherent read view. The view borrows the engine, so no write can
    /// interleave for its lifetime.
    fn read_view(&self) -> Result<Self::View<'_>, StoreError>;
    /// Begin the one write transaction. The returned transaction borrows the
    /// engine mutably, so a second cannot be opened while it is live.
    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError>;
    /// Fail closed with [`StoreError::ReadOnly`] when the handle cannot accept
    /// writes, so a caller can mint a read-only ceiling before opening a
    /// transaction.
    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError>;
    /// Run one full integrity audit: a complete structural walk that verifies
    /// every stored checksum. The fast open path does not re-verify page
    /// checksums, so an external bit-flip on live bytes reads back silently
    /// altered on a clean open; this walk catches it and reports
    /// [`StoreError::Corruption`]. It is one bounded pass over the store — the
    /// audit primitive the lifecycle audit mode consumes — and allocates no
    /// per-caller unbounded work. A backend with nothing durable beneath it
    /// (the in-memory engine) has nothing to walk and passes trivially.
    fn audit_integrity(&mut self) -> Result<(), StoreError>;
}

/// Reject a key or value that exceeds its batch limit before it is staged.
pub(crate) fn check_cell_limits(key: &[u8], value: &[u8]) -> Result<(), StoreError> {
    if key.len() > limits::MAX_KEY_LEN {
        return Err(StoreError::LimitExceeded {
            limit: "key length",
        });
    }
    if value.len() > limits::MAX_VALUE_LEN {
        return Err(StoreError::LimitExceeded {
            limit: "value length",
        });
    }
    Ok(())
}
