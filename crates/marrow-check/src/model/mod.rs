//! The recorded semantic model: the crate's id vocabulary and, in later sub-lanes,
//! the def arena, side tables, and query surface. Ids are minted once and read
//! everywhere; identity is typed and compared by value.

pub mod decls;
pub mod ids;
