//! Marrow's saved-tree storage layer.
//!
//! This crate defines Marrow's ordered-byte backend contract, the tree-cell
//! physical key profile above that contract, and an in-memory store. It sits
//! below language facts: it does not parse `.mw`, resolve schemas, or decide
//! source-name identity.
//!
//! Tree-cell keys ([`cell`]) derive from stable catalog IDs and typed key values.
//! Saved-path encoding ([`path`]) is the backend traversal and raw archive
//! surface. [`mem`] and the native backend serve opaque ordered bytes through the
//! [`Backend`](backend::Backend) contract.

pub mod archive;
pub mod backend;
pub mod cell;
pub mod decimal;
pub mod key;
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
