//! The stored positional-value fixtures: one ledger and the declarations whose leaves a
//! stored `struct` value or enum payload writes by position.
//!
//! Shared for the same reason as [`crate::ledger_ids`]: the compiler's contract tests and
//! the lifecycle's attach and apply tests describe one durable graph, so they spell its
//! identities once.

/// One ledger for every positional program: a `markers` root whose `Marker` resource
/// stores `at` (or the top-level pair `x`/`y`), plus the sum and member anchors each
/// stored enum needs.
pub const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Marker 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id root markers 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key markers.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id field Marker.at 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Marker.x 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a\n\
     id field Marker.y 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id sum Shape 50505050505050505050505050505050\n\
     id member Shape.rect 51515151515151515151515151515151\n\
     id sum Place 52525252525252525252525252525252\n\
     id member Place.at 53535353535353535353535353535353\n\
     id sum Option[Pos] 60606060606060606060606060606060\n\
     id member Option[Pos].none 61616161616161616161616161616161\n\
     id member Option[Pos].some 62626262626262626262626262626262\n\
     id sum Result[Pos,int] 70707070707070707070707070707070\n\
     id member Result[Pos,int].ok 71717171717171717171717171717171\n\
     id member Result[Pos,int].err 72727272727272727272727272727272\n\
     id sum Pair[int] 80808080808080808080808080808080\n\
     id member Pair[int].two 81818181818181818181818181818181\n\
     high-water 0\n\
     end\n";

/// A stored struct with leaves `x`, `y`.
pub const POS: &str = "struct Pos {\n    x: int\n    y: int\n}\n";
/// [`POS`] with its leaves reordered.
pub const POS_SWAPPED: &str = "struct Pos {\n    y: int\n    x: int\n}\n";
/// An enum whose one member carries the payload `width`, `height`.
pub const SHAPE: &str = "enum Shape {\n    rect(width: int, height: int)\n}\n";
/// [`SHAPE`] with its payload leaves reordered.
pub const SHAPE_SWAPPED: &str = "enum Shape {\n    rect(height: int, width: int)\n}\n";
/// A generic enum whose one member carries the payload `a`, `b`.
pub const PAIR: &str = "enum Pair<T> {\n    two(a: T, b: T)\n}\n";
/// [`PAIR`] with its payload leaves reordered.
pub const PAIR_SWAPPED: &str = "enum Pair<T> {\n    two(b: T, a: T)\n}\n";
