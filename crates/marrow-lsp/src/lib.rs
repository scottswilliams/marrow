//! The Marrow language server.
//!
//! `marrow-lsp` is a language server dispatched as `marrow lsp`. It consumes the
//! compiler's published editor-analysis facts — diagnostics, checked formatting, hover,
//! and definition over one exact [`marrow_compile::analysis::AnalysisSnapshot`] — and
//! serves them over the Language Server Protocol. The published fact owner is
//! [`marrow_compile::AnalysisSnapshot`]. The server reconstructs no language semantics:
//! types, paths, facts, diagnostics, and formatting come only from the compiler fact
//! surface (H00f/H00f2) and the shared physical project adapter (CAP01).
//!
//! The server owns a private, closed JSON-RPC 2.0 envelope ([`protocol`]) and a bounded
//! standard-library transport ([`transport`]); it depends on no LSP-server framework,
//! async runtime, or channel crate. Every retained resource is charged against the
//! frozen [`capacities`] before admission.
//!
//! # Topology
//!
//! A bounded reader frames stdin; the process-main coordinator owns lifecycle,
//! admission, document versions, overlay construction, edit coalescing, and outbound
//! ordering; one analysis worker owns all parse/format/compile/snapshot work; one
//! writer accepts immutable framed bytes. The threads communicate over bounded
//! standard-library channels and move-only affine credits ([`credit`]).

#![warn(missing_docs)]

pub mod analysis;
pub mod capacities;
pub mod credit;
pub mod document;
pub mod facts;
pub mod position;
pub mod protocol;
pub mod transport;
pub mod uri;
