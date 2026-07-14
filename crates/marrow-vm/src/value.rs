//! The runtime value model (design §D).
//!
//! The vacant state of an optional is the typed `Optional(None)`; there is no
//! `Value::Absent`. Records and optionals arrive with their slices; the current
//! subset uses the scalar values.

use std::rc::Rc;

/// A runtime value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Text(Rc<str>),
}
