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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Text(Rc<str>),
    Record(u16, Box<[Option<Value>]>),
    Optional(Option<Box<Value>>),
}
