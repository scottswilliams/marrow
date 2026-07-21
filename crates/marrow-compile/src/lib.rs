//! The storeless Marrow compiler slice.
//!
//! This crate is the refounded analysis-fact owner for the compiled subset,
//! extracted from the prototype checker (design §A). It parses source through the
//! retained parser, checks the subset, owns the language scalar vocabulary
//! ([`ScalarType`]), and lowers to a validated [`marrow_image::ImageDraft`] that it
//! encodes to canonical bytes. It has no edge to the verifier, VM, kernel, or
//! store: the compiler emits bytes, opens no store, and mints no verified image.

// Production compiler code reports every source-level problem as a typed
// diagnostic and never aborts. The six explicit-abort families are denied in
// non-test builds; each legitimate invariant guard carries a narrow, reasoned
// `#[expect(...)]` at its site. `expect` self-enables its restriction lint at
// that span, so it is fulfilled in both the test and non-test compilations under
// the strict all-targets gate, and it additionally fails as an unfulfilled
// expectation if a later edit removes the guarded abort — turning a stale guard
// into a build error that a bare `allow` would silence. Test code keeps the
// ordinary abort vocabulary.
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]

mod analysis;
mod compile;
mod diag;
mod durable;
mod konst;
mod lower;
mod scalar;
mod types;

pub use analysis::{
    AnalysisFailure, AnalysisResourceLimit, AnalysisSnapshot, Definition, Fact, Hover,
    InputRevision, MAX_FORMAT_OUTPUT_BYTES, MAX_HOVER_DISPLAY_BYTES, MAX_SNAPSHOT_FACT_BYTES,
    MAX_SNAPSHOT_FACT_COUNT, QueryError, Unavailability, analyze,
};
pub use compile::{
    CompileFailure, CompileInvariant, CompileResourceLimit, Compiled, CompiledTests, ExportEntry,
    NonEmptySourceDiagnostics, ResourceLimitKind, TestEntry, compile, compile_with_tests,
};
pub use diag::{IdentityGap, SourceDiagnostic};
pub use marrow_image::ExportId;
pub use scalar::ScalarType;

/// The canonical [`FileIdentity`](marrow_project::FileIdentity) for a test source
/// path. Tests attribute diagnostics to a real captured file exactly as the
/// production capture path does, so they name the same identity type rather than a
/// bare string.
#[cfg(test)]
pub(crate) fn test_file_identity(path: &str) -> marrow_project::FileIdentity {
    marrow_project::FileIdentity::validate(path)
        .expect("test source path is a canonical identity")
        .0
}

/// A `'static` reference to the canonical `src/main.mw` identity, for test sites
/// that borrow a `&FileIdentity` (a `MintSite`, an identity resolver, a lowerer
/// file) or return one with `'static` lifetime.
#[cfg(test)]
pub(crate) fn test_main_file_identity() -> &'static marrow_project::FileIdentity {
    static ID: std::sync::OnceLock<marrow_project::FileIdentity> = std::sync::OnceLock::new();
    ID.get_or_init(|| test_file_identity("src/main.mw"))
}

#[cfg(doctest)]
pub mod compile_invariant_privacy_doctests {
    //! The compiler invariant is an opaque public outcome. External callers may
    //! distinguish the outer `CompileFailure::Invariant` arm, but cannot
    //! construct or classify its private cause.
    //!
    //! Tuple construction remains private:
    //!
    //! ```compile_fail
    //! use marrow_compile::CompileInvariant;
    //!
    //! let _ = CompileInvariant(());
    //! ```
    //!
    //! A cause-bearing tuple pattern remains private as well:
    //!
    //! ```compile_fail
    //! use marrow_compile::CompileInvariant;
    //!
    //! fn classify(invariant: CompileInvariant) {
    //!     match invariant {
    //!         CompileInvariant(_) => {}
    //!     }
    //! }
    //! ```
}
