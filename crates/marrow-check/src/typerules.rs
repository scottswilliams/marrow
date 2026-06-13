//! The type lattice's compatibility, conversion, and display rules, plus the
//! literal-range envelope.

use std::path::Path;

use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::{CHECK_LITERAL_RANGE, CheckDiagnostic, MarrowType};

/// The decimal envelope, mirroring `marrow_store::decimal`: at most 34
/// significant digits and 34 fractional places.
pub(crate) const DECIMAL_MAX_DIGITS: usize = 34;

/// Reject a numeric literal provably out of range at check time. The lexer emits
/// bare digits with the sign as a separate unary operator, so an integer is in
/// range exactly when it parses as `i64`; a decimal stays within the 34-digit
/// envelope.
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
        diagnostics.push(CheckDiagnostic::error(
            CHECK_LITERAL_RANGE,
            file,
            span,
            format!("{type_name} literal `{}` is out of range", elide(text)),
        ));
    }
}

/// A literal bounded for display: an out-of-range numeric literal can be
/// arbitrarily long, and echoing the whole of it into a diagnostic floods the
/// output without adding meaning past the leading digits.
fn elide(text: &str) -> std::borrow::Cow<'_, str> {
    const MAX: usize = 48;
    if text.len() > MAX {
        let head: String = text.chars().take(MAX).collect();
        std::borrow::Cow::Owned(format!("{head}…"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// Whether a decimal literal provably exceeds the 34-digit envelope. Mirrors
/// `marrow_store::decimal`, which normalizes first: leading integer zeros and
/// trailing fraction zeros drop out, so they are stripped before counting and no
/// literal the runtime would normalize back into range is rejected.
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

/// The scalar a type denotes, or `None` for any non-scalar. A `None` from the
/// checker-only `Error` is a real mismatch at scalar-requiring sites, distinct
/// from the untyped-value path taken for `Unknown`.
pub(crate) fn as_primitive(ty: &MarrowType) -> Option<ScalarType> {
    match ty {
        MarrowType::Primitive(scalar) => Some(*scalar),
        _ => None,
    }
}

/// Whether a type is a concrete non-scalar value type. These compare nominally,
/// so an operator that defaults or equates them resolves by [`type_compatible`]
/// rather than by scalar shape. `Error` has its own operator handling and
/// `Unknown` defers, so both are excluded.
pub(crate) fn is_concrete_nonscalar(ty: &MarrowType) -> bool {
    matches!(
        ty,
        MarrowType::Identity(_)
            | MarrowType::Resource(_)
            | MarrowType::GroupEntry { .. }
            | MarrowType::Sequence(_)
            | MarrowType::LocalTree { .. }
            | MarrowType::Enum { .. }
    )
}

/// Whether a value of type `actual` may stand where `expected` is required.
/// `Some` is a verdict; `None` defers (the value is `unknown`, owned by the
/// untyped-value path, or `expected` places no constraint). Identities, resources,
/// and enums compare nominally: an identity matches by store root, a resource by
/// module-qualified resource name, and an enum by owning module and name. A
/// key-compatible foreign identity or same-named enum from another module is
/// still a mismatch. `Unknown` and `Invalid` defer for recovery and explicit
/// `unknown` flows.
pub(crate) fn type_compatible(expected: &MarrowType, actual: &MarrowType) -> Option<bool> {
    if matches!(expected, MarrowType::Invalid)
        || matches!(actual, MarrowType::Unknown | MarrowType::Invalid)
    {
        return None;
    }
    match expected {
        MarrowType::Primitive(p) => Some(matches!(actual, MarrowType::Primitive(q) if q == p)),
        MarrowType::Identity(store_root) => {
            Some(matches!(actual, MarrowType::Identity(other) if other == store_root))
        }
        MarrowType::Resource(resource) => {
            Some(matches!(actual, MarrowType::Resource(other) if other == resource))
        }
        MarrowType::GroupEntry { resource, layers } => Some(matches!(
            actual,
            MarrowType::GroupEntry {
                resource: other,
                layers: other_layers,
            } if other == resource && other_layers == layers
        )),
        MarrowType::Enum { .. } => Some(actual == expected),
        MarrowType::Sequence(element) => match actual {
            MarrowType::Sequence(other) => type_compatible(element, other),
            _ => Some(false),
        },
        MarrowType::LocalTree { keys, value } => match actual {
            MarrowType::LocalTree {
                keys: other_keys,
                value: other_value,
            } if keys.len() == other_keys.len() => {
                let keys_match = keys
                    .iter()
                    .zip(other_keys)
                    .all(|(expected, actual)| type_compatible(expected, actual) == Some(true));
                if keys_match {
                    type_compatible(value, other_value)
                } else {
                    Some(false)
                }
            }
            MarrowType::LocalTree { .. } => Some(false),
            _ => Some(false),
        },
        MarrowType::Error => Some(matches!(actual, MarrowType::Error)),
        MarrowType::Invalid => None,
        MarrowType::Unknown => None,
    }
}

/// Whether an expected type has a value-conversion boundary, so an `unknown` value
/// against it is a `check.untyped_value` ("convert it first") rather than a
/// deferral. Every concrete typed place rejects an `unknown`: a stored identity
/// carries only its keys, not its source store, so an unchecked record's fields
/// could land foreign identities that data integrity cannot later distinguish from
/// genuine references. The value must be converted through a constructor or a
/// checked read first. An `unknown` flows freely only into another `unknown`, and a
/// sequence has no conversion and is left to the runtime.
pub(crate) fn expects_conversion(ty: &MarrowType) -> bool {
    matches!(
        ty,
        MarrowType::Primitive(_)
            | MarrowType::Error
            | MarrowType::Enum { .. }
            | MarrowType::Identity(_)
            | MarrowType::Resource(_)
            | MarrowType::GroupEntry { .. }
    )
}

pub(crate) fn is_numeric(scalar: ScalarType) -> bool {
    matches!(scalar, ScalarType::Int | ScalarType::Decimal)
}

/// Whether a scalar can be the endpoint of a range-for loop. A range walks
/// evenly spaced values, so its endpoint must support a bounded step contract.
pub(crate) fn is_steppable(scalar: ScalarType) -> bool {
    matches!(
        scalar,
        ScalarType::Int | ScalarType::Date | ScalarType::Instant
    )
}

pub(crate) fn type_renderable_at_runtime(ty: &MarrowType) -> Option<bool> {
    match ty {
        MarrowType::Primitive(
            ScalarType::Bool | ScalarType::Int | ScalarType::Str | ScalarType::Decimal,
        )
        | MarrowType::Identity(_) => Some(true),
        MarrowType::Unknown | MarrowType::Invalid => None,
        MarrowType::Primitive(
            ScalarType::Bytes | ScalarType::Date | ScalarType::Instant | ScalarType::Duration,
        )
        | MarrowType::Error
        | MarrowType::Resource(_)
        | MarrowType::GroupEntry { .. }
        | MarrowType::Enum { .. }
        | MarrowType::Sequence(_)
        | MarrowType::LocalTree { .. } => Some(false),
    }
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

/// The source spelling of a type for a diagnostic message, or `value` for a type
/// with no surface spelling.
pub(crate) fn marrow_type_name(ty: &MarrowType) -> String {
    match ty {
        MarrowType::Primitive(scalar) => scalar.name().to_string(),
        MarrowType::Error => "Error".to_string(),
        MarrowType::Identity(root) => format!("Id(^{root})"),
        MarrowType::Resource(resource) => resource.clone(),
        MarrowType::GroupEntry { resource, .. } => resource.clone(),
        MarrowType::Enum { name, .. } => name.clone(),
        MarrowType::Sequence(element) => format!("sequence[{}]", marrow_type_name(element)),
        MarrowType::LocalTree { value, .. } => format!("tree[{}]", marrow_type_name(value)),
        MarrowType::Invalid => "value".to_string(),
        MarrowType::Unknown => "value".to_string(),
    }
}

/// Display names for two mismatched types, qualifying each enum as `module::Name`
/// only when both sides are same-named enums from different modules — otherwise the
/// bare names would read "expects `Status`, but found `Status`".
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
