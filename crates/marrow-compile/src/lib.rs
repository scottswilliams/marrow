//! The storeless Marrow compiler slice.
//!
//! This crate is the refounded analysis-fact owner for the compiled subset,
//! extracted from the prototype checker (design §A). It parses source through the
//! retained parser, checks the subset, owns the language scalar vocabulary
//! ([`ScalarType`]), and lowers to a validated [`marrow_image::ImageDraft`] that it
//! encodes to canonical bytes. It has no edge to the verifier, VM, kernel, or
//! store: the compiler emits bytes, opens no store, and mints no verified image.

mod compile;
mod diag;
mod durable;
mod konst;
mod lower;
mod scalar;
mod types;

pub use compile::{Compiled, CompiledTests, ExportEntry, TestEntry, compile, compile_with_tests};
pub use diag::{IdentityGap, SourceDiagnostic};
pub use marrow_image::ExportId;
pub use scalar::ScalarType;

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
