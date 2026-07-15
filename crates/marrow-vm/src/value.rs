//! The runtime value model (design §D).
//!
//! The vacant state of an optional is the typed `Optional(None)`; there is no
//! dedicated absent value variant. Records and optionals arrive with their
//! slices; the current subset uses the scalar values.

use std::rc::Rc;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Text(Rc<str>),
    Bytes(Rc<[u8]>),
    Record(u16, Box<[Option<Value>]>),
    Optional(Option<Box<Value>>),
    Enum(u16, u16, Box<[Value]>),
}
