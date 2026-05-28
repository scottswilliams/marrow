//! Marrow's saved-tree storage layer.
//!
//! This crate defines how Marrow saved paths encode to ordered bytes and, in
//! later slices, the backend contract every store implements and an in-memory
//! store. It sits below language facts: it does not parse `.mw`, resolve
//! schemas, or maintain indexes. Those belong to the checker and runtime above
//! it.
//!
//! The saved-path encoding ([`path`]) has byte order that is Marrow's own and
//! independent of any backend's collation; the in-memory store ([`mem`]) serves
//! values over those ordered paths.

pub mod mem;
pub mod path;
pub mod value;
