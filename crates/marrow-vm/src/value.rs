//! The runtime value model (design §D).
//!
//! The vacant state of an optional is the typed `Optional(None)`; there is no
//! dedicated absent value variant. Records and optionals arrive with their
//! slices; the current subset uses the scalar values.

use std::rc::Rc;

use marrow_kernel::codec::key::KeyScalar;

/// A runtime value.
///
/// A record carries its type index and one slot per field in declared order; a
/// required field's slot is always present, a sparse field's slot may be vacant.
/// An optional's vacant state is `Optional(None)` — there is no separate absent
/// variant.
///
/// An enum value carries its enum-type index, its selected variant index, and one
/// slot per dense payload leaf in declaration order (empty for a payloadless
/// member). Equality is exact — same variant and equal payload — so it derives
/// structurally.
///
/// A `List` carries its COLLTYPES index and its elements in insertion order. A `Map`
/// carries its COLLTYPES index and its entries kept sorted by `CollectionKeyOrder`
/// (the kernel's `KeyScalar` order), so positional access yields key order and a
/// lookup is a binary search. Both are ordinary copied values; the shared `Rc`
/// backing gives copy-on-write growth (`Rc::make_mut`) rather than a copy per read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Text(Rc<str>),
    Bytes(Rc<[u8]>),
    Record(u16, Box<[Option<Value>]>),
    Optional(Option<Box<Value>>),
    Enum(u16, u16, Box<[Value]>),
    List(u16, Rc<Vec<Value>>),
    Map(u16, Rc<Vec<(KeyScalar, Value)>>),
}
