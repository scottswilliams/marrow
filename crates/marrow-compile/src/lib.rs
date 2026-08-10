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
mod demand;
mod diag;
mod durable;
mod konst;
mod lower;
mod scalar;
mod types;

pub use analysis::{
    ActiveCall, ActiveCallOutcome, AnalysisFailure, AnalysisResourceLimit, AnalysisSnapshot,
    Candidate, CandidateKind, CompletionOutcome, Completions, DeclKind, DeclSymbol, Definition,
    Fact, FormatOutcome, Hover, InputRevision, MAX_ACTIVE_CALL_RENDER_BYTES,
    MAX_COMPLETION_CANDIDATES, MAX_COMPLETION_RENDER_BYTES, MAX_DOCUMENT_SYMBOLS_PER_FILE,
    MAX_FORMAT_OUTPUT_BYTES, MAX_SNAPSHOT_FACT_BYTES, MAX_SNAPSHOT_FACT_COUNT, MAX_SYMBOL_DEPTH,
    ParamPiece, PositionClass, QueryError, Unavailability, analyze,
};
pub use compile::{
    CompileFailure, CompileInvariant, CompileResourceLimit, Compiled, CompiledTests, ExportEntry,
    MAX_PARSED_FILE_BYTES, MAX_QUERY_PARSE_TRANSIENT_BYTES, NonEmptySourceDiagnostics,
    ResourceLimitKind, TestEntry, compile, compile_with_tests,
};
pub use demand::{DemandSummary, DurableNaming, RootDemand};
pub use diag::{IdentityGap, SourceDiagnostic};
pub use marrow_image::ExportId;
pub use marrow_syntax::FormatRefusal;
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
pub mod source_diagnostic_privacy_doctests {
    //! `SourceDiagnostic` is opaque: consumers read the frozen accessor set and
    //! can neither reach a payload field nor construct a diagnostic.
    //!
    //! Access to either declared field does not compile. The field names are
    //! pinned by the absence gate `source_diagnostic_fields_stay_private`, so a
    //! rename must update these doctests instead of voiding them silently.
    //!
    //! ```compile_fail
    //! fn read(diagnostic: &marrow_compile::SourceDiagnostic) {
    //!     let _ = &diagnostic.file;
    //! }
    //! ```
    //!
    //! ```compile_fail
    //! fn read(diagnostic: &marrow_compile::SourceDiagnostic) {
    //!     let _ = &diagnostic.payload;
    //! }
    //! ```
    //!
    //! External construction does not compile:
    //!
    //! ```compile_fail
    //! fn build(file: marrow_project::FileIdentity) -> marrow_compile::SourceDiagnostic {
    //!     marrow_compile::SourceDiagnostic::at(
    //!         "check.type",
    //!         &file,
    //!         marrow_syntax::SourceSpan::default(),
    //!         "forged".to_string(),
    //!     )
    //! }
    //! ```
}

#[cfg(doctest)]
pub mod fact_coordinate_privacy_doctests {
    //! A retained fact's file coordinate is private to the compiler. It indexes one
    //! snapshot's own module order, so it is meaningless outside the snapshot that
    //! minted it and is never handed to a consumer. A consumer names a file by
    //! `marrow_project::FileIdentity` and reads a definition through
    //! [`Definition`](crate::Definition), both of which the snapshot resolves.
    //!
    //! The coordinate type is not nameable outside the crate:
    //!
    //! ```compile_fail
    //! fn coordinate() -> marrow_compile::FileRef {
    //!     unimplemented!()
    //! }
    //! ```
    //!
    //! Neither is the retained fact they index, so a fact cannot be forged and handed
    //! to a snapshot that did not produce it:
    //!
    //! ```compile_fail
    //! fn fact() -> marrow_compile::HoverFact {
    //!     unimplemented!()
    //! }
    //! ```
    //!
    //! The public definition fact carries a resolved file identity and exposes no
    //! coordinate; its fields stay private:
    //!
    //! ```compile_fail
    //! fn read(definition: &marrow_compile::Definition) {
    //!     let _ = &definition.file;
    //! }
    //! ```
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
