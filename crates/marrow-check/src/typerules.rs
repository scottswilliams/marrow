//! The type lattice's compatibility, conversion, and display rules, plus the
//! literal-range envelope.

use super::*;

/// The decimal envelope, mirroring `marrow_store::decimal`: at most 34
/// significant digits and 34 fractional places.
pub(crate) const DECIMAL_MAX_DIGITS: usize = 34;

/// Flag a numeric literal whose magnitude is provably out of range, so it is
/// caught at check time rather than only at run time (`run.overflow`). The lexer
/// emits a number literal as bare ASCII digits (the sign is a separate unary
/// operator), so an integer is in range exactly when it parses as `i64`, and a
/// decimal `digits.digits` is in range only within the 34-significant-digit /
/// 34-fractional-place envelope.
pub(crate) fn check_literal_range(
    kind: marrow_syntax::LiteralKind,
    text: &str,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::LiteralKind;
    let out_of_range = match kind {
        LiteralKind::Integer => text.parse::<i64>().is_err(),
        LiteralKind::Decimal => decimal_out_of_envelope(text),
        // A duration literal's magnitude is checked at run time, where it shares
        // the int/decimal overflow path, so nothing is flagged here.
        LiteralKind::Duration => false,
        LiteralKind::String | LiteralKind::Bytes | LiteralKind::Bool => false,
    };
    if out_of_range {
        let type_name = match kind {
            LiteralKind::Integer => "int",
            _ => "decimal",
        };
        diagnostics.push(CheckDiagnostic {
            code: CHECK_LITERAL_RANGE,
            severity: Severity::Error,
            file: file.to_path_buf(),
            message: format!("{type_name} literal `{text}` is out of range"),
            span,
        });
    }
}

/// Whether a decimal literal `digits.digits` (or `digits`) provably falls outside
/// the 34-digit envelope. Mirrors `marrow_store::decimal`, which normalizes before
/// the envelope check: leading integer zeros and trailing fraction zeros drop out,
/// so they are stripped before counting. A literal is rejected only when its
/// canonical significant digits or fractional places exceed 34 — never a value the
/// runtime would normalize back into range.
pub(crate) fn decimal_out_of_envelope(text: &str) -> bool {
    let (integer, fraction) = text.split_once('.').unwrap_or((text, ""));
    let integer = integer.trim_start_matches('0');
    let fraction = fraction.trim_end_matches('0');
    // Significant digits run from the first to the last nonzero digit. With the
    // integer part empty (all zeros), leading fraction zeros are not significant.
    let significant = if integer.is_empty() {
        fraction.trim_start_matches('0').len()
    } else {
        integer.len() + fraction.len()
    };
    significant > DECIMAL_MAX_DIGITS || fraction.len() > DECIMAL_MAX_DIGITS
}

/// The scalar a type denotes, or `None` for any non-scalar (resource, identity,
/// sequence, the checker-only `Error`, or unknown) type. `Error` is concrete, not
/// untyped: each scalar-requiring site (operator, condition, return, assignment,
/// argument) handles a `None` from `Error` as a real mismatch, distinct from the
/// untyped-value path taken for `Unknown`.
pub(crate) fn as_primitive(ty: &MarrowType) -> Option<ScalarType> {
    match ty {
        MarrowType::Primitive(scalar) => Some(*scalar),
        _ => None,
    }
}

/// Whether a type is a concrete non-scalar value type: an identity, a whole
/// record, a sequence, or an enum. These compare nominally, so an operator that
/// defaults or equates them resolves by [`type_compatible`] rather than by scalar
/// shape. The checker-only `Error` and the untyped `Unknown` are excluded: `Error`
/// has its own operator handling and `Unknown` defers.
pub(crate) fn is_concrete_nonscalar(ty: &MarrowType) -> bool {
    matches!(
        ty,
        MarrowType::Identity(_)
            | MarrowType::Resource(_)
            | MarrowType::Sequence(_)
            | MarrowType::Enum { .. }
    )
}

/// Whether a value of type `actual` may stand where `expected` is required.
/// `Some(true)`/`Some(false)` is a verdict; `None` defers — the value's type is
/// `unknown` (the untyped-value path owns that case) or `expected` itself places
/// no constraint. Identities and resources compare nominally: same resource name
/// or nothing, so a key-compatible foreign identity is still a mismatch. A
/// cross-module identity the checker could not place is `Unknown` and defers,
/// permissive until the type IR is unified across modules. Enums are nominal too:
/// a value satisfies an enum place only when it is the same enum, by owning module
/// and name, so two same-named enums in different modules never alias.
pub(crate) fn type_compatible(expected: &MarrowType, actual: &MarrowType) -> Option<bool> {
    if matches!(expected, MarrowType::Invalid)
        || matches!(actual, MarrowType::Unknown | MarrowType::Invalid)
    {
        return None;
    }
    match expected {
        MarrowType::Primitive(p) => Some(matches!(actual, MarrowType::Primitive(q) if q == p)),
        MarrowType::Identity(resource) => {
            Some(matches!(actual, MarrowType::Identity(other) if other == resource))
        }
        MarrowType::Resource(resource) => {
            Some(matches!(actual, MarrowType::Resource(other) if other == resource))
        }
        MarrowType::Enum { .. } => Some(actual == expected),
        MarrowType::Sequence(element) => match actual {
            MarrowType::Sequence(other) => type_compatible(element, other),
            _ => Some(false),
        },
        MarrowType::Error => Some(matches!(actual, MarrowType::Error)),
        MarrowType::Invalid => None,
        MarrowType::Unknown => None,
    }
}

/// Whether an expected type has a value-conversion boundary, so an `unknown` value
/// against it is a `check.untyped_value` ("convert it first") rather than a
/// deferral. Scalars and the checker-only `Error` are reached by the conversion
/// builtins (`int(...)`, `ErrorCode(...)`, the `Error(...)` constructor), an enum is
/// a concrete typed place a dynamic value must be made into, and an identity is
/// reached by its `Resource::Id(...)` constructor. A whole resource is the same
/// hazard at the record level: an `unknown` record's fields would land raw scalars
/// or foreign identities in the resource's typed (identity) fields — encodings
/// `data integrity` cannot later distinguish from a genuine reference, since a
/// stored identity carries only its keys, not its source resource. So every
/// concrete typed place rejects an `unknown` value, which must be converted into the
/// resource (a constructor, a read of the same resource) first. The only place an
/// `unknown` flows without conversion is into another `unknown`; a sequence has no
/// conversion either and is left to the runtime.
pub(crate) fn expects_conversion(ty: &MarrowType) -> bool {
    matches!(
        ty,
        MarrowType::Primitive(_)
            | MarrowType::Error
            | MarrowType::Enum { .. }
            | MarrowType::Identity(_)
            | MarrowType::Resource(_)
    )
}

pub(crate) fn is_numeric(scalar: ScalarType) -> bool {
    matches!(scalar, ScalarType::Int | ScalarType::Decimal)
}

/// Whether a scalar can be the endpoint of a range-for loop. A range walks evenly
/// spaced values, so its endpoint type must support a step: a number for int and
/// decimal, a duration for the temporal date and instant. String, bool, bytes, and
/// duration itself are not steppable endpoints.
pub(crate) fn is_steppable(scalar: ScalarType) -> bool {
    matches!(
        scalar,
        ScalarType::Int | ScalarType::Decimal | ScalarType::Date | ScalarType::Instant
    )
}

pub(crate) fn is_ordered(scalar: ScalarType) -> bool {
    matches!(
        scalar,
        ScalarType::Int
            | ScalarType::Decimal
            | ScalarType::Str
            | ScalarType::Bytes
            | ScalarType::Date
            | ScalarType::Instant
            | ScalarType::Duration
    )
}

pub(crate) fn unary_symbol(op: marrow_syntax::UnaryOp) -> &'static str {
    use marrow_syntax::UnaryOp;
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "not",
    }
}

pub(crate) fn binary_symbol(op: marrow_syntax::BinaryOp) -> &'static str {
    use marrow_syntax::BinaryOp;
    match op {
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Concat => "_",
        BinaryOp::RangeExclusive => "..",
        BinaryOp::RangeInclusive => "..=",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Coalesce => "??",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        BinaryOp::Is => "is",
    }
}

/// The source spelling of a type for a diagnostic message: a scalar by name, an
/// identity as `Resource::Id`, a resource by its name, an enum by its name, a
/// sequence as `sequence[element]`, the checker-only `Error`, or `value` for a type
/// with no surface spelling.
pub(crate) fn marrow_type_name(ty: &MarrowType) -> String {
    match ty {
        MarrowType::Primitive(scalar) => scalar.name().to_string(),
        MarrowType::Error => "Error".to_string(),
        MarrowType::Identity(resource) => format!("{resource}::Id"),
        MarrowType::Resource(resource) => resource.clone(),
        MarrowType::Enum { name, .. } => name.clone(),
        MarrowType::Sequence(element) => format!("sequence[{}]", marrow_type_name(element)),
        MarrowType::Invalid => "value".to_string(),
        MarrowType::Unknown => "value".to_string(),
    }
}

/// Display names for two mismatched types, qualifying each enum with its owning
/// module (`module::Name`) only when both sides are enums that share the same
/// short name but come from different modules — otherwise the bare names ("expects
/// `Status`, but found `Status`") name no distinguishing detail. Any other pairing
/// uses each type's plain [`marrow_type_name`].
pub(crate) fn mismatch_display(left: &MarrowType, right: &MarrowType) -> (String, String) {
    if let (
        MarrowType::Enum {
            module: left_module,
            name: left_name,
        },
        MarrowType::Enum {
            module: right_module,
            name: right_name,
        },
    ) = (left, right)
        && left_name == right_name
        && left_module != right_module
    {
        return (
            format!("{left_module}::{left_name}"),
            format!("{right_module}::{right_name}"),
        );
    }
    (marrow_type_name(left), marrow_type_name(right))
}
