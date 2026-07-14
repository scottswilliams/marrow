//! Marrow's ordered-byte storage engine.
//!
//! This crate defines the private byte-oriented engine contract and the two
//! implementors that back it — an in-memory store and a redb-backed native store
//! — under one conformance suite. It temporarily also hosts the logical
//! key/value/civil-date codecs until a later lane moves them to their runtime
//! owner. It sits below language facts: it does not parse `.mw`, resolve schemas,
//! or assign language identity.
//!
//! Tree-cell keys ([`cell`]) derive from stable catalog IDs and typed key values.

// The engine's sole prototype consumer (the logical tree facade) was deleted at
// B00, so in a non-test compile the crate-private engine and codecs have no
// caller: they are exercised by the in-crate conformance suite and tests only.
// The refounded consumer arrives with the T01 tracer and the E00 contract
// narrowing, which deletes this allowance together with anything still unused.
#![allow(dead_code)]

mod backend;
pub mod cell;
mod codec;
pub mod key;
mod mem;
#[cfg(feature = "native")]
mod redb;
mod traversal;
pub mod value;

// Private substrate tests keep memory and native engines aligned without making
// engine keys part of the production API.
#[cfg(test)]
mod conformance;

/// The shared store error for typed tree-cell operations.
pub use backend::StoreError;
