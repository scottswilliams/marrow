//! Marrow's ordered-byte storage engine.
//!
//! This crate defines the narrow byte-oriented engine contract ([`ByteEngine`])
//! and its two implementors — an in-memory engine ([`MemoryEngine`]) and an
//! opaque redb-backed owner ([`NativeEngineOwner`]) — under one conformance suite. It
//! orders opaque bytes: it does not parse `.mw`, resolve schemas, assign language
//! identity, or interpret key or value bytes. The logical key and value codecs
//! that give those bytes meaning are owned by the path kernel (`marrow-kernel`).
//!
//! The contract is exactly a coherent lifetime-bound [`ReadView`], a consuming
//! [`WriteTxn`] with a [`CommitOutcome`], point get/put/remove, one bounded
//! forward `scan_after`, a bounded integrity audit, and create-new/open-existing
//! construction. There is no rich scan family, prefix delete, transaction
//! nesting, or snapshot pin/unpin pair, and no raw public store handle or backend
//! registry.
//!
//! Native storage cannot be opened without its process owner lock; the raw engine is
//! private to this crate.

mod engine;
mod error;
mod mem;
#[cfg(feature = "native")]
mod native_owner;
#[cfg(feature = "native")]
mod redb;
mod traversal;

// The engine is exercised by the in-crate conformance suite and by the path
// kernel; the conformance laws keep the memory and native engines aligned.
#[cfg(test)]
mod conformance;

pub use engine::limits::{MAX_KEY_LEN, MAX_VALUE_LEN, SCAN_MAX_AGGREGATE_BYTES, SCAN_MAX_RECORDS};
pub use engine::{
    ByteEngine, Cell, CommitOutcome, ReadView, WriteTxn, batch_is_full, cell_within_limits,
};
pub use error::{StoreError, StoreLimit, StoreOp};
pub use mem::MemoryEngine;
#[cfg(feature = "native")]
pub use native_owner::{
    NATIVE_ENGINE_FILE, NATIVE_ENGINE_FORMAT_VERSION, NATIVE_LOCK_FILE, NativeEngineOwner,
    NativeLockError, NativeLockOwner, NativeOpenAccess, NativeOwnerAcquireError,
    NativeOwnerOpenError, NativeOwnerTxn, NativeOwnerView, NativePromotionRefusal,
    PendingNativeEngineOwner,
};
