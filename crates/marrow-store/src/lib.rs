//! Marrow's typed tree-cell storage layer.
//!
//! This crate defines Marrow's typed tree-cell storage contract and the private
//! ordered-byte engines that back it. It sits below language facts: it does not
//! parse `.mw`, resolve schemas, or assign language identity.
//!
//! Tree-cell keys ([`cell`]) derive from stable catalog IDs and typed key values.

mod backend;
mod backup;
pub mod cell;
pub mod decimal;
pub mod key;
mod mem;
mod metadata;
#[cfg(feature = "native")]
mod redb;
mod traversal;
pub mod tree;
pub mod value;

// Private substrate tests keep memory and native engines aligned without making
// engine keys part of the production API.
#[cfg(test)]
mod conformance;

/// The shared store error for typed tree-cell operations.
pub use backend::StoreError;

/// Exact base-10 decimal arithmetic, re-exported at the crate root.
pub use decimal::Decimal;
