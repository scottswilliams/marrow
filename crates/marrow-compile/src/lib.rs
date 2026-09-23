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
pub use demand::{DemandPaths, DurableNaming};
pub use diag::{
    IdentityGap, NameFamily, RefusedDeclaration, SourceDiagnostic, Steer, TypeMismatch,
    TypeSpelling, Unresolved,
};
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
        marrow_test_support::project::project(&[
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
pub mod public_type_pins {
    //! Properties of the public types a consumer cannot circumvent: a diagnostic and a
    //! compiler invariant are constructed only by the compiler, and a retained fact's
    //! file coordinate and the fact it indexes are not nameable outside the snapshot
    //! that minted them.
    //!
    //! ```compile_fail
    //! fn build(file: marrow_compile::ProjectFile) -> marrow_compile::SourceDiagnostic {
    //!     marrow_compile::SourceDiagnostic::at(
    //!         marrow_codes::Code::CheckType,
    //!         &file,
    //!         marrow_syntax::SourceSpan::default(),
    //!         "forged".to_string(),
    //!     )
    //! }
    //! ```
    //!
    //! ```compile_fail
    //! use marrow_compile::CompileInvariant;
    //!
    //! let _ = CompileInvariant(loop {});
    //! ```
    //!
    //! ```compile_fail
    //! fn coordinate() -> marrow_compile::FileRef {
    //!     unimplemented!()
    //! }
    //! ```
    //!
    //! ```compile_fail
    //! fn fact() -> marrow_compile::HoverFact {
    //!     unimplemented!()
    //! }
    //! ```
}
