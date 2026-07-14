//! Marrow's ordered-byte storage engine.
//!
//! This crate defines the private byte-oriented engine contract and the two
//! implementors that back it — an in-memory store and a redb-backed native store
//! — under one conformance suite. It orders opaque bytes: it does not parse
//! `.mw`, resolve schemas, assign language identity, or interpret key or value
//! bytes. The logical key/value codecs that give those bytes meaning are owned
//! by the path kernel (`marrow-kernel`).

// The engine's sole prototype consumer (the logical tree facade) was deleted at
// B00, so in a non-test compile the crate-private engine has no caller: it is
// exercised by the in-crate conformance suite and tests only. The refounded
// consumer arrives with the path kernel; the E00 contract narrowing deletes this
// allowance together with anything still unused.
#![allow(dead_code)]

mod backend;
mod engine;
mod mem;
#[cfg(feature = "native")]
mod redb;
mod traversal;

// Private substrate tests keep memory and native engines aligned without making
// engine keys part of the production API.
#[cfg(test)]
mod conformance;

/// The shared store error for typed byte-cell operations.
pub use backend::StoreError;
#[cfg(feature = "native")]
pub use engine::NativeEngine;
/// The narrow ordered-byte seam the path kernel consumes.
pub use engine::{ByteEngine, Cell, MemoryEngine};
