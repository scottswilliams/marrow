//! The type lattice's compatibility, conversion, and display rules, plus the
//! literal-range envelope.

use std::path::Path;

use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::model::decls::DeclIds;
use crate::{CHECK_LITERAL_RANGE, CHECK_UNRESOLVED_OPTIONAL, CheckDiagnostic, MarrowType};

/// The decimal envelope, mirroring `marrow_store::decimal`: at most 34
/// significant digits and 34 fractional places.
pub(crate) const DECIMAL_MAX_DIGITS: usize = 34;

/// Whether an integer literal carries a leading unary minus. The lexer tokenizes a
/// bare magnitude with the sign as a separate unary operator, but the negation
/// shifts the representable bound: `9223372036854775808` is `i64::MAX + 1` and out
/// of range on its own, yet negated it is exactly `i64::MIN`, a valid value.
#[derive(Clone, Copy)]
pub(crate) enum LiteralSign {
    Bare,
    Negated,
}

/// The text and span of the integer literal under a `-` unary operator, or `None`
/// for any other shape. A negated integer literal is the one place a magnitude of
/// `i64::MAX + 1` is in range, so range-checking and constant-folding both pivot on
/// recognizing it.
pub(crate) fn negated_integer_literal(
    op: marrow_syntax::UnaryOp,
    operand: &marrow_syntax::Expression,
) -> Option<(&str, SourceSpan)> {
    match (op, operand) {
        (
            marrow_syntax::UnaryOp::Neg,
            marrow_syntax::Expression::Literal {
                kind: marrow_syntax::LiteralKind::Integer,
                text,
                span,
            },
        ) => Some((text, *span)),
        _ => None,
    }
}

/// Reject a numeric literal provably out of range at check time. An integer is in
/// range when its magnitude parses as `i64`, except that a negated magnitude may
/// also be exactly `i64::MAX + 1`, which negates to `i64::MIN`. A decimal stays
/// within the 34-digit envelope.
pub(crate) fn check_literal_range(
    kind: marrow_syntax::LiteralKind,
    text: &str,
    sign: LiteralSign,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::LiteralKind;
    let out_of_range = match kind {
        LiteralKind::Integer => integer_out_of_range(text, sign),
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

/// The `i64` an integer literal magnitude denotes, or `None` when it is out of
/// range. A bare magnitude must itself fit `i64`; a negated one may reach
/// `i64::MAX + 1`, whose negation is `i64::MIN`. The negated case folds the sign
/// before parsing rather than parsing the bare magnitude, which would reject
/// `i64::MIN`. This is the single place the `i64` boundary lives, so the range
/// check and the constant fold agree on it.
pub(crate) fn parse_integer_literal(text: &str, sign: LiteralSign) -> Option<i64> {
    match sign {
        LiteralSign::Bare => text.parse().ok(),
        LiteralSign::Negated => format!("-{text}").parse().ok(),
    }
}

/// Whether an integer literal's magnitude is outside `i64`.
fn integer_out_of_range(text: &str, sign: LiteralSign) -> bool {
    parse_integer_literal(text, sign).is_none()
}

/// Whether a decimal literal provably exceeds the 34-digit envelope. Mirrors
/// `marrow_store::decimal`, which normalizes first: leading integer zeros and
/// trailing fraction zeros drop out, so they are stripped before counting and no
/// literal the runtime would normalize back into range is rejected.
fn decimal_out_of_envelope(text: &str) -> bool {
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
    match ty {
        MarrowType::Identity(_)
        | MarrowType::Resource(_)
        | MarrowType::GroupEntry { .. }
        | MarrowType::Sequence(_)
        | MarrowType::LocalTree { .. }
        // An optional is a concrete one-rule value, not an `Unknown` deferral: it
        // must be resolved before any `T` operation, so the operator and render
        // gates treat it as concrete rather than silently admitting it.
        | MarrowType::Optional(_)
        | MarrowType::Absent
        | MarrowType::Enum { .. } => true,
        MarrowType::Primitive(_)
        | MarrowType::Error
        | MarrowType::Unknown
        | MarrowType::Invalid => false,
    }
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
    // A reported fault poisons its expected type; defer so it does not cascade.
    if matches!(expected, MarrowType::Invalid) {
        return None;
    }
    // The optional axis is matched before any `Unknown`/`Invalid` deferral so a
    // degraded-to-`Unknown` expected type can never shadow the one rule and
    // silently admit an optional. Present widens into an optional place; an
    // optional or `absent` never satisfies a non-optional place until resolved.
    match (expected, actual) {
        (MarrowType::Optional(_), MarrowType::Absent) => return Some(true),
        (MarrowType::Optional(inner), MarrowType::Optional(a)) => {
            return type_compatible(inner, a);
        }
        (MarrowType::Optional(inner), other) => return type_compatible(inner, other),
        (_, MarrowType::Optional(_) | MarrowType::Absent) => return Some(false),
        _ => {}
    }
    if matches!(actual, MarrowType::Unknown | MarrowType::Invalid) {
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
        // The optional axis is fully decided above; reaching here means `actual` is
        // a concrete non-optional that no optional/absent place accepts.
        MarrowType::Optional(_) | MarrowType::Absent => Some(false),
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
    match ty {
        MarrowType::Primitive(_)
        | MarrowType::Error
        | MarrowType::Enum { .. }
        | MarrowType::Identity(_)
        | MarrowType::Resource(_)
        | MarrowType::GroupEntry { .. } => true,
        // An optional place carries the conversion boundary of its inner value, so
        // an untyped value stored into it must be converted first; the empty
        // optional is never an expected place type.
        MarrowType::Optional(_) => true,
        MarrowType::Absent
        | MarrowType::Sequence(_)
        | MarrowType::LocalTree { .. }
        | MarrowType::Unknown
        | MarrowType::Invalid => false,
    }
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

/// Whether a value of type `ty` renders to text directly through `print` and
/// interpolation. Every scalar, every enum, a saved identity, and a sequence
/// whose element type renders can render; local trees and resources are rejected
/// at check. `None` defers the unknown and recovery types to the runtime value.
pub(crate) fn type_renderable_at_runtime(ty: &MarrowType) -> Option<bool> {
    match ty {
        MarrowType::Primitive(_) | MarrowType::Identity(_) | MarrowType::Enum { .. } => Some(true),
        MarrowType::Sequence(element) => type_renderable_at_runtime(element),
        MarrowType::Unknown | MarrowType::Invalid => None,
        // An optional must be resolved before it renders (the one rule), so it is a
        // concrete non-renderable value here rather than a deferral.
        MarrowType::Optional(_)
        | MarrowType::Absent
        | MarrowType::Error
        | MarrowType::Resource(_)
        | MarrowType::GroupEntry { .. }
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

/// The source spelling of a type for a diagnostic message. The poison `Invalid`
/// type has no surface spelling and renders as `value`; it normally suppresses the
/// cascades that would surface it.
// The recovery view reaches every leaf arm but is read only once a nominal leaf
// carries an interned id instead of its spelling; until each leaf is migrated it
// flows through the recursive arms alone.
#[allow(clippy::only_used_in_recursion)]
pub(crate) fn marrow_type_name(names: &DeclIds<'_>, ty: &MarrowType) -> String {
    match ty {
        MarrowType::Primitive(scalar) => scalar.name().to_string(),
        MarrowType::Error => "Error".to_string(),
        MarrowType::Identity(root) => format!("Id(^{root})"),
        MarrowType::Resource(resource) => resource.clone(),
        MarrowType::GroupEntry { resource, .. } => resource.clone(),
        MarrowType::Enum { name, .. } => name.clone(),
        MarrowType::Sequence(element) => format!("sequence[{}]", marrow_type_name(names, element)),
        MarrowType::LocalTree { value, .. } => format!("tree[{}]", marrow_type_name(names, value)),
        MarrowType::Optional(inner) => format!("{}?", marrow_type_name(names, inner)),
        MarrowType::Absent => "absent".to_string(),
        MarrowType::Invalid => "value".to_string(),
        MarrowType::Unknown => "unknown".to_string(),
    }
}

/// Whether a value's type carries possible absence — an `Optional(T)` or the empty
/// `absent`. The one rule keys off this: such a value cannot stand where a definite
/// `T` is required until a resolution form discharges it.
pub(crate) fn is_optional_value(ty: &MarrowType) -> bool {
    matches!(ty, MarrowType::Optional(_) | MarrowType::Absent)
}

/// The one-rule diagnostic for a `T?` value used where a `T` is required, naming the
/// four resolution forms. The single owner of the `check.unresolved_optional`
/// message; every slot site routes through it.
pub(crate) fn unresolved_optional_diagnostic(file: &Path, span: SourceSpan) -> CheckDiagnostic {
    CheckDiagnostic::error(
        CHECK_UNRESOLVED_OPTIONAL,
        file,
        span,
        "a `T?` value is used where a `T` is required; resolve it with `?? default`, `if const`, `exists`, or `?.`",
    )
}

/// The one rule at a typed slot: fires when `actual` is optional (or the empty
/// optional) and `expected` is a non-optional, concrete place. Returns `None`
/// otherwise so a boundary site can fall through to its generic mismatch.
pub(crate) fn unresolved_optional(
    expected: &MarrowType,
    actual: &MarrowType,
    span: SourceSpan,
    file: &Path,
) -> Option<CheckDiagnostic> {
    let optional_expected = matches!(expected, MarrowType::Optional(_));
    let poisoned = matches!(expected, MarrowType::Invalid) || matches!(actual, MarrowType::Invalid);
    (is_optional_value(actual) && !optional_expected && !poisoned)
        .then(|| unresolved_optional_diagnostic(file, span))
}

/// Display names for two mismatched types, qualifying each enum as `module::Name`
/// only when both sides are same-named enums from different modules — otherwise the
/// bare names would read "expects `Status`, but found `Status`".
pub(crate) fn mismatch_display(
    names: &DeclIds<'_>,
    left: &MarrowType,
    right: &MarrowType,
) -> (String, String) {
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
    (
        marrow_type_name(names, left),
        marrow_type_name(names, right),
    )
}
