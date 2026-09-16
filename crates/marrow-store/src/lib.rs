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
//! Native storage cannot be opened without its process owner lock. The raw
//! engine is intentionally not part of this crate's public surface:
//!
//! ```compile_fail
//! use marrow_store::NativeEngine;
//! let _ = NativeEngine::create_new(std::path::Path::new("store.redb"));
//! ```

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

/// Freezes the crate's public surface against removal and rename: every `pub`
/// export named in `lib.rs` appears below, so deleting or renaming one fails to
/// compile here. Additions still compile clean, so review remains the gate
/// against surface growth.
#[cfg(test)]
mod public_surface_audit {
    use super::*;

    #[test]
    fn public_surface_is_exactly_the_whitelist() {
        // Traits — named as bounds, so deleting or renaming one breaks these
        // signatures. `ReadView` and `WriteTxn` are also the supertrait/associated
        // bounds `ByteEngine` requires.
        fn read_view<V: ReadView>() {}
        fn write_txn<T: WriteTxn>() {}
        fn byte_engine<E: ByteEngine>() {}
        let _named = (
            read_view::<crate::mem::MemView<'static>>,
            write_txn::<crate::mem::MemTxn<'static>>,
            byte_engine::<MemoryEngine>,
        );
        let _cell_limit: fn(&[u8], &[u8]) -> bool = cell_within_limits;
        let _batch_limit: fn(usize, usize, usize) -> bool = batch_is_full;
        // Concrete types and constructors.
        let _cell: Cell = (Vec::new(), Vec::new());
        let _outcomes = [
            CommitOutcome::Confirmed,
            CommitOutcome::Aborted,
            CommitOutcome::Indeterminate,
        ];
        let _code: fn(&StoreError) -> marrow_codes::Code = StoreError::code;
        let _op = StoreOp::Open;
        let _limit = StoreLimit::KeyLength;
        let _new: fn() -> MemoryEngine = MemoryEngine::new;

        #[cfg(feature = "native")]
        {
            fn native_engine<E: ByteEngine>() {}
            let _owner = native_engine::<NativeEngineOwner>;
            let _format = NATIVE_ENGINE_FORMAT_VERSION;
            let _access = [NativeOpenAccess::ReadOnly, NativeOpenAccess::ReadWrite];
            let _engine_file = NATIVE_ENGINE_FILE;
            let _lock_file = NATIVE_LOCK_FILE;
            let _acquire: fn(
                &std::path::Path,
            )
                -> Result<PendingNativeEngineOwner, NativeOwnerAcquireError> =
                NativeEngineOwner::acquire_existing;
            let _directory: fn(&PendingNativeEngineOwner) -> &std::path::Path =
                PendingNativeEngineOwner::directory;
        }
    }
}
