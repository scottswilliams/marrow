//! The one descriptor for the builtin `Error` shape.
//!
//! `Error` is the language's structural error value: the type `throw`/`catch`
//! and `std::log::error` work in, and the value `Error(...)` constructs. Its
//! field set, each field's type, and the required subset live here so the
//! checker (which types a field read off an `Error`) and the runtime (which
//! validates and builds a constructed `Error`) consume one definition rather
//! than each spelling the vocabulary out.

use crate::Type;

/// The `code` field name, the required dotted error identifier every `Error`
/// carries. The single owner of this spelling; the runtime builds and reads the
/// field through it so the descriptor below and the value layout cannot drift.
pub const CODE: &str = "code";

/// The `message` field name, the required human-readable text every `Error`
/// carries. Owned here alongside [`CODE`] for the same reason.
pub const MESSAGE: &str = "message";

/// One field of the builtin `Error` shape: its name, its declared type, and
/// whether a constructed `Error` must supply it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorField {
    pub name: &'static str,
    pub ty: Type,
    pub required: bool,
}

/// The fields of `Error`, in declaration order. `code` and `message` are the
/// required subset every `Error` value carries; `help` is optional prose and
/// `data` is an open `unknown` payload the checker does not constrain.
const FIELDS: &[ErrorField] = &[
    ErrorField {
        name: CODE,
        ty: Type::Scalar(crate::ScalarType::Str),
        required: true,
    },
    ErrorField {
        name: MESSAGE,
        ty: Type::Scalar(crate::ScalarType::Str),
        required: true,
    },
    ErrorField {
        name: "help",
        ty: Type::Scalar(crate::ScalarType::Str),
        required: false,
    },
    ErrorField {
        name: "data",
        ty: Type::Unknown,
        required: false,
    },
];

/// Every `Error` field in declaration order. The runtime walks these to reject
/// an unknown field name and to enforce that each required field is supplied.
pub fn fields() -> &'static [ErrorField] {
    FIELDS
}

/// The descriptor for the `Error` field named `name`, or `None` when `Error`
/// has no such field. The checker types `error_value.field` through this.
pub fn field(name: &str) -> Option<&'static ErrorField> {
    FIELDS.iter().find(|field| field.name == name)
}
