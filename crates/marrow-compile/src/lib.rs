//! The storeless Marrow compiler slice.
//!
//! This crate is the analysis-fact owner for the compiled subset. It parses source
//! through the retained parser, checks the subset, owns the language scalar
//! vocabulary ([`ScalarType`]), and lowers to a validated [`marrow_image::ImageDraft`]
//! that it encodes to canonical bytes. It has no edge to the verifier, VM, kernel, or
//! store: the compiler emits bytes, opens no store, and mints no verified image.

// Production compiler code reports every source-level problem as a typed diagnostic
// and never aborts, so the six explicit-abort families are denied outside tests. Each
// legitimate invariant guard carries a narrow, reasoned `#[expect(...)]` at its site
// rather than an `allow`: an `expect` also fails once the guarded abort is removed,
// so a stale guard becomes a build error. Test code keeps the ordinary vocabulary.
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
mod bounded;
mod call_graph;
#[cfg(test)]
mod call_graph_tests;
mod compile;
mod decl;
mod demand;
mod diag;
mod durable;
mod issuance;
mod konst;
mod lower;
mod scalar;
mod source;
mod types;

#[cfg(test)]
#[path = "../tests/common/ledger.rs"]
mod test_ledger;
#[cfg(test)]
#[path = "../tests/common/project.rs"]
mod test_project;

pub use analysis::{
    ActiveCall, ActiveCallOutcome, AnalysisFailure, AnalysisResourceLimit, AnalysisSnapshot,
    Candidate, CandidateKind, CompletionOutcome, Completions, DeclKind, DeclSymbol, Definition,
    Fact, FormatOutcome, Hover, InputRevision, MAX_ACTIVE_CALL_RENDER_BYTES,
    MAX_COMPLETION_CANDIDATES, MAX_COMPLETION_RENDER_BYTES, MAX_DOCUMENT_SYMBOLS_PER_FILE,
    MAX_SNAPSHOT_FACT_BYTES, MAX_SNAPSHOT_FACT_COUNT, MAX_SYMBOL_DEPTH, ParamPiece, PositionClass,
    QueryError, Unavailability, analyze,
};
pub use compile::{
    CompileFailure, CompileInvariant, CompileResourceLimit, Compiled, CompiledTests, ExportEntry,
    MAX_PARSED_FILE_BYTES, MAX_QUERY_PARSE_TRANSIENT_BYTES, NonEmptySourceDiagnostics,
    ResourceLimitKind, TestEntry, check, compile, compile_with_tests,
};
pub use decl::{DeclarationNamespace, RefusalReport, SourceStage};
pub use demand::{DemandSummary, DurableNaming, RootDemand};
pub use diag::{IdentityGap, NameFamily, RefusedDeclaration, SourceDiagnostic, Steer, Unresolved};
pub use marrow_image::ExportId;
pub use marrow_syntax::FormatRefusal;
pub use scalar::ScalarType;
pub use source::ProjectFile;

/// The in-crate tests' project, taken once through the production capture path.
///
/// Every file address, origin set and identity ledger a test needs is read out of a
/// capture rather than assembled beside one, so no fixture can hold a value
/// `capture_origins` would not have produced. A test that needs another path adds it
/// to this listing.
#[cfg(test)]
pub(crate) fn test_input() -> &'static marrow_project::ProjectInput {
    static INPUT: std::sync::OnceLock<marrow_project::ProjectInput> = std::sync::OnceLock::new();
    INPUT.get_or_init(|| {
        test_project::project(&[
            ("src/a.mw", ""),
            ("src/abcdefgh.mw", ""),
            ("src/first.mw", ""),
            ("src/later.mw", ""),
            ("src/main.mw", ""),
        ])
    })
}

/// The address [`test_input`]'s capture gave `path`.
#[cfg(test)]
#[track_caller]
pub(crate) fn test_file(path: &str) -> &'static ProjectFile {
    static FILES: std::sync::OnceLock<Vec<ProjectFile>> = std::sync::OnceLock::new();
    FILES
        .get_or_init(|| {
            test_input()
                .modules()
                .iter()
                .map(ProjectFile::from)
                .collect()
        })
        .iter()
        .find(|file| file.identity().as_str() == path)
        .expect("the captured test fixture holds this path")
}

#[cfg(doctest)]
pub mod source_diagnostic_privacy_doctests {
    //! `SourceDiagnostic` is opaque: consumers read the frozen accessor set and can
    //! neither reach a payload field nor construct a diagnostic. The field names below
    //! are pinned by the absence gate `source_diagnostic_fields_stay_private`, so a
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
    //! A retained fact's file coordinate is private to the compiler: it indexes one
    //! snapshot's own module order, so it is meaningless outside the snapshot that
    //! minted it. A consumer names a file by `marrow_project::FileIdentity` and reads a
    //! definition through [`Definition`](crate::Definition), both snapshot-resolved.
    //!
    //! The coordinate type is not nameable outside the crate:
    //!
    //! ```compile_fail
    //! fn coordinate() -> marrow_compile::FileRef {
    //!     unimplemented!()
    //! }
    //! ```
    //!
    //! Neither is the retained fact they index, so a fact cannot be forged and handed to
    //! a snapshot that did not produce it:
    //!
    //! ```compile_fail
    //! fn fact() -> marrow_compile::HoverFact {
    //!     unimplemented!()
    //! }
    //! ```
    //!
    //! The public definition fact carries a resolved identity, never a coordinate, and
    //! its fields stay private:
    //!
    //! ```compile_fail
    //! fn read(definition: &marrow_compile::Definition) {
    //!     let _ = &definition.file;
    //! }
    //! ```
}

#[cfg(doctest)]
pub mod compile_invariant_privacy_doctests {
    //! The compiler invariant is an opaque public outcome: external callers may
    //! distinguish the outer `CompileFailure::Invariant` arm, but can neither construct
    //! nor classify its private cause.
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
