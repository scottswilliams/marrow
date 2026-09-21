//! The Marrow language server.
//!
//! `marrow-lsp` consumes the compiler's published editor-analysis facts —
//! diagnostics, checked formatting, hover, definition, completion, signature help,
//! and document symbols over one exact
//! [`marrow_compile::AnalysisSnapshot`] — and serves them over the Language Server
//! Protocol. The server reconstructs no language semantics: types, paths, facts,
//! diagnostics, and formatting come only from the compiler fact surface and the
//! shared physical project adapter.
//!
//! The server owns a private, closed JSON-RPC 2.0 envelope (`protocol`) and a bounded
//! standard-library transport (`transport`); it depends on no LSP-server framework,
//! async runtime, or channel crate. Every retained resource is charged against the
//! frozen `capacities` before admission.
//!
//! # Topology
//!
//! A bounded reader frames stdin; the process-main coordinator owns lifecycle,
//! admission, document versions, overlay construction, edit coalescing, and outbound
//! ordering; one analysis worker owns all parse/format/compile/snapshot work; one
//! writer accepts immutable framed bytes. The threads communicate over bounded
//! standard-library channels; the coordinator hands the writer at most a fixed number
//! of frames ahead of their delivery receipts.
//!
//! The single public entry point is [`serve`]; every module below is a private
//! implementation owner.

mod analysis;
mod capacities;
mod document;
mod facts;
mod lifecycle;
mod outbound;
mod position;
mod protocol;
mod query;
mod server;
mod transport;
mod uri;

pub use server::serve;
