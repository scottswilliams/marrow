//! Marrow's ordered-byte storage engine.
//!
//! This crate defines the narrow byte-oriented engine contract ([`ByteEngine`])
//! and its two implementors — an in-memory engine ([`MemoryEngine`]) and a
//! redb-backed native engine ([`NativeEngine`]) — under one conformance suite. It
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

mod engine;
mod error;
mod mem;
#[cfg(feature = "native")]
mod redb;
mod traversal;

// The engine is exercised by the in-crate conformance suite and by the path
// kernel; the conformance laws keep the memory and native engines aligned.
#[cfg(test)]
mod conformance;

pub use engine::{ByteEngine, Cell, CommitOutcome, ReadView, WriteTxn};
pub use error::StoreError;
pub use mem::MemoryEngine;
#[cfg(feature = "native")]
pub use redb::NativeEngine;

/// Freezes the crate's public surface against removal and rename: every `pub`
/// export named in `lib.rs` appears below, so deleting or renaming one fails to
/// compile here. It does NOT detect additions — a new `pub` item compiles clean
/// past this audit, so review is the gate against surface growth (an additive
/// freeze would need external tooling, deliberately out of the dependency
/// budget). This is a compile-time audit, not a runtime test.
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
        // Concrete types and constructors.
        let _cell: Cell = (Vec::new(), Vec::new());
        let _outcomes = [
            CommitOutcome::Confirmed,
            CommitOutcome::Aborted,
            CommitOutcome::Indeterminate,
        ];
        let _code: fn(&StoreError) -> &'static str = StoreError::code;
        let _new: fn() -> MemoryEngine = MemoryEngine::new;

        #[cfg(feature = "native")]
        {
            let _open: fn(&std::path::Path) -> Result<NativeEngine, StoreError> =
                NativeEngine::open;
            let _open_ro: fn(&std::path::Path) -> Result<NativeEngine, StoreError> =
                NativeEngine::open_read_only;
        }
    }
}
