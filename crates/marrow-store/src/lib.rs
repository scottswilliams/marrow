//! Marrow's saved-tree storage layer.
//!
//! This crate defines how Marrow saved paths encode to ordered bytes, the
//! [`Backend`](backend::Backend) contract every store implements, and an
//! in-memory store. It sits below language facts: it does not parse `.mw`,
//! resolve schemas, or maintain indexes. Those belong to the checker and runtime
//! above it.
//!
//! The saved-path encoding ([`path`]) has byte order that is Marrow's own and
//! independent of any backend's collation; the in-memory store ([`mem`]) serves
//! values over those ordered paths and implements the [`Backend`](backend::Backend)
//! contract.

pub mod archive;
pub mod backend;
pub mod decimal;
pub mod mem;
pub mod path;
#[cfg(feature = "native")]
pub mod redb;
mod traversal;
pub mod value;

// The reusable backend conformance suite is test-only: it holds every backend to
// one contract and is not part of the published store surface.
#[cfg(test)]
mod conformance;

/// The shared backend error, re-exported at the crate root: it is part of the
/// [`Backend`](backend::Backend) contract.
pub use backend::StoreError;

/// Exact base-10 decimal arithmetic, re-exported at the crate root.
pub use decimal::Decimal;
