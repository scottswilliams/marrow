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

/// The top-level value state a boundary must decide before consulting structural
/// compatibility. Diagnosed poison is recursive; the remaining dispositions are
/// top-level so containers retain their own shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeDisposition {
    Poisoned,
    Recovery,
    ExplicitDynamic,
    NoValue,
    Concrete,
}

/// A typed boundary verdict. Poison is an already-diagnosed dependency, while a
/// rejection carries the boundary-specific reason that owns any new diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Admission<F> {
    Accepted,
    Poisoned,
    Rejected(F),
}

/// Why a concrete strict value slot rejected its actual type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StrictValueFault {
    Recovery,
    ExplicitDynamic,
    NoValue,
    Optional,
    Mismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectionFault {
    NoValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InferredBindingFault {
    NoValue,
}

pub(crate) fn admit_inferred_binding(ty: &MarrowType) -> Admission<InferredBindingFault> {
    match disposition(ty) {
        TypeDisposition::Poisoned => Admission::Poisoned,
        TypeDisposition::NoValue => Admission::Rejected(InferredBindingFault::NoValue),
        TypeDisposition::Recovery
        | TypeDisposition::ExplicitDynamic
        | TypeDisposition::Concrete => Admission::Accepted,
    }
}

pub(crate) fn admit_collection_operand(ty: &MarrowType) -> Admission<CollectionFault> {
    match disposition(ty) {
        TypeDisposition::Poisoned => Admission::Poisoned,
        TypeDisposition::NoValue => Admission::Rejected(CollectionFault::NoValue),
        TypeDisposition::Recovery
        | TypeDisposition::ExplicitDynamic
        | TypeDisposition::Concrete => Admission::Accepted,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyPolicy {
    Saved,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyFault {
    Recovery,
    ExplicitDynamic,
    NoValue,
    Optional,
    Mismatch,
    Arity,
    Named,
    Range,
}

pub(crate) type KeyAdmission = Admission<KeyFault>;

pub(crate) fn merge_key_admission(left: KeyAdmission, right: KeyAdmission) -> KeyAdmission {
    match (left, right) {
        (Admission::Poisoned, _) | (_, Admission::Poisoned) => Admission::Poisoned,
        (Admission::Rejected(fault), _) | (_, Admission::Rejected(fault)) => {
            Admission::Rejected(fault)
        }
        (Admission::Accepted, Admission::Accepted) => Admission::Accepted,
    }
}

/// The independently inferred parts of a saved range key. Keeping endpoints and
/// `by` separate prevents a concrete sibling from laundering poison or another
/// non-value state before the declared key policy sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RangeTypeAggregate {
    start: Option<MarrowType>,
    end: Option<MarrowType>,
    step: Option<MarrowType>,
}

impl RangeTypeAggregate {
    pub(crate) fn new(
        start: Option<MarrowType>,
        end: Option<MarrowType>,
        step: Option<MarrowType>,
    ) -> Self {
        Self { start, end, step }
    }

    pub(crate) fn admit_saved(&self, expected: &MarrowType) -> KeyAdmission {
        self.components()
            .fold(Admission::Accepted, |admission, actual| {
                merge_key_admission(admission, admit_key(KeyPolicy::Saved, expected, actual))
            })
    }

    pub(crate) fn representative_type(&self) -> MarrowType {
        if self.components().any(MarrowType::contains_invalid) {
            return MarrowType::Invalid;
        }
        for disposition in [
            TypeDisposition::NoValue,
            TypeDisposition::Recovery,
            TypeDisposition::ExplicitDynamic,
        ] {
            if let Some(component) = self
                .components()
                .find(|component| crate::typerules::disposition(component) == disposition)
            {
                return component.clone();
            }
        }
        if let Some(optional) = self
            .components()
            .find(|component| is_optional_value(component))
        {
            return optional.clone();
        }
        self.start
            .as_ref()
            .or(self.end.as_ref())
            .cloned()
            .unwrap_or(MarrowType::Unknown)
    }

    fn components(&self) -> impl Iterator<Item = &MarrowType> {
        [self.start.as_ref(), self.end.as_ref(), self.step.as_ref()]
            .into_iter()
            .flatten()
    }
}

pub(crate) fn disposition(ty: &MarrowType) -> TypeDisposition {
    if ty.contains_invalid() {
        return TypeDisposition::Poisoned;
    }
    match ty {
        MarrowType::Unknown => TypeDisposition::Recovery,
        MarrowType::Dynamic => TypeDisposition::ExplicitDynamic,
        MarrowType::NoValue => TypeDisposition::NoValue,
        MarrowType::Primitive(_)
        | MarrowType::Error
        | MarrowType::Resource(_)
        | MarrowType::GroupEntry { .. }
        | MarrowType::Identity(_)
        | MarrowType::Enum(_)
        | MarrowType::Sequence(_)
        | MarrowType::LocalTree { .. }
        | MarrowType::Optional(_)
        | MarrowType::Absent => TypeDisposition::Concrete,
        MarrowType::Invalid => TypeDisposition::Poisoned,
    }
}

/// Admit `actual` to a strict typed value slot. State policy is decided before
/// the unchanged compatibility lattice is consulted.
pub(crate) fn admit_strict_value(
    expected: &MarrowType,
    actual: &MarrowType,
) -> Admission<StrictValueFault> {
    if disposition(expected) == TypeDisposition::Poisoned
        || disposition(actual) == TypeDisposition::Poisoned
    {
        return Admission::Poisoned;
    }
    if disposition(expected) == TypeDisposition::ExplicitDynamic {
        return match disposition(actual) {
            TypeDisposition::Poisoned => Admission::Poisoned,
            TypeDisposition::NoValue => Admission::Rejected(StrictValueFault::NoValue),
            TypeDisposition::Recovery
            | TypeDisposition::ExplicitDynamic
            | TypeDisposition::Concrete => Admission::Accepted,
        };
    }
    match disposition(actual) {
        TypeDisposition::Poisoned => Admission::Poisoned,
        TypeDisposition::Recovery => Admission::Rejected(StrictValueFault::Recovery),
        TypeDisposition::ExplicitDynamic => Admission::Rejected(StrictValueFault::ExplicitDynamic),
        TypeDisposition::NoValue => Admission::Rejected(StrictValueFault::NoValue),
        TypeDisposition::Concrete
            if is_optional_value(actual) && !matches!(expected, MarrowType::Optional(_)) =>
        {
            Admission::Rejected(StrictValueFault::Optional)
        }
        TypeDisposition::Concrete if expected == actual => Admission::Accepted,
        TypeDisposition::Concrete => match type_compatible(expected, actual) {
            Some(true) => Admission::Accepted,
            Some(false) => Admission::Rejected(StrictValueFault::Mismatch),
            None => Admission::Accepted,
        },
    }
}

pub(crate) fn admit_key(
    policy: KeyPolicy,
    expected: &MarrowType,
    actual: &MarrowType,
) -> KeyAdmission {
    if disposition(expected) == TypeDisposition::Poisoned
        || disposition(actual) == TypeDisposition::Poisoned
    {
        return Admission::Poisoned;
    }
    match disposition(actual) {
        TypeDisposition::Poisoned => Admission::Poisoned,
        TypeDisposition::Recovery => Admission::Rejected(KeyFault::Recovery),
        TypeDisposition::ExplicitDynamic if policy == KeyPolicy::Local => Admission::Accepted,
        TypeDisposition::ExplicitDynamic => Admission::Rejected(KeyFault::ExplicitDynamic),
        TypeDisposition::NoValue => Admission::Rejected(KeyFault::NoValue),
        TypeDisposition::Concrete if is_optional_value(actual) => {
            Admission::Rejected(KeyFault::Optional)
        }
        TypeDisposition::Concrete => match type_compatible(expected, actual) {
            Some(true) => Admission::Accepted,
            Some(false) | None => Admission::Rejected(KeyFault::Mismatch),
        },
    }
}

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
/// from dynamic and recovery states.
pub(crate) fn as_primitive(ty: &MarrowType) -> Option<ScalarType> {
    match ty {
        MarrowType::Primitive(scalar) => Some(*scalar),
        _ => None,
    }
}

/// Whether a type is a concrete non-scalar value type. These compare nominally,
/// so an operator that defaults or equates them resolves by [`type_compatible`]
/// rather than by scalar shape. `Error` has its own operator handling; dynamic,
/// no-value, and recovery states defer, so all are excluded.
pub(crate) fn is_concrete_nonscalar(ty: &MarrowType) -> bool {
    match ty {
        MarrowType::Identity(_)
        | MarrowType::Resource(_)
        | MarrowType::GroupEntry { .. }
        | MarrowType::Sequence(_)
        | MarrowType::LocalTree { .. }
        // An optional is a concrete one-rule value, not a recovery deferral: it
        // must be resolved before any `T` operation, so the operator and render
        // gates treat it as concrete rather than silently admitting it.
        | MarrowType::Optional(_)
        | MarrowType::Absent
        | MarrowType::Enum(_) => true,
        MarrowType::Primitive(_)
        | MarrowType::Error
        | MarrowType::Dynamic
        | MarrowType::NoValue
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
/// still a mismatch. Explicit dynamic values retain the former `unknown` flow;
/// no-value, unresolved, and invalid states defer without becoming value types.
pub(crate) fn type_compatible(expected: &MarrowType, actual: &MarrowType) -> Option<bool> {
    // A reported fault poisons its expected type; defer so it does not cascade.
    if matches!(expected, MarrowType::Invalid) {
        return None;
    }
    // The optional axis is matched before any dynamic/recovery deferral so a
    // degraded expected type can never shadow the one rule and silently admit an
    // optional. Present widens into an optional place; an
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
    if matches!(
        actual,
        MarrowType::Dynamic | MarrowType::Invalid | MarrowType::NoValue | MarrowType::Unknown
    ) {
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
        MarrowType::Enum(_) => Some(actual == expected),
        MarrowType::Sequence(element) => match actual {
            MarrowType::Sequence(other) => type_compatible(element, other),
            _ => Some(false),
        },
        MarrowType::LocalTree { keys, value } => match actual {
            MarrowType::LocalTree {
                keys: other_keys,
                value: other_value,
            } if keys.len() == other_keys.len() => {
                let (has_unknown, has_mismatch) = keys.iter().zip(other_keys).fold(
                    (false, false),
                    |(has_unknown, has_mismatch), (expected, actual)| match type_compatible(
                        expected, actual,
                    ) {
                        Some(true) => (has_unknown, has_mismatch),
                        Some(false) => (has_unknown, true),
                        None => (true, has_mismatch),
                    },
                );
                if has_unknown {
                    None
                } else if has_mismatch {
                    Some(false)
                } else {
                    type_compatible(value, other_value)
                }
            }
            MarrowType::LocalTree { .. } => Some(false),
            _ => Some(false),
        },
        MarrowType::Error => Some(matches!(actual, MarrowType::Error)),
        // The optional axis is fully decided above; reaching here means `actual` is
        // a concrete non-optional that no optional/absent place accepts.
        MarrowType::Optional(_) | MarrowType::Absent => Some(false),
        MarrowType::Dynamic | MarrowType::Invalid | MarrowType::NoValue | MarrowType::Unknown => {
            None
        }
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
        | MarrowType::Enum(_)
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
        | MarrowType::Dynamic
        | MarrowType::NoValue
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
/// at check. `None` defers dynamic and recovery states to their runtime/backstop
/// owners.
pub(crate) fn type_renderable_at_runtime(ty: &MarrowType) -> Option<bool> {
    match disposition(ty) {
        TypeDisposition::Poisoned
        | TypeDisposition::Recovery
        | TypeDisposition::ExplicitDynamic => return None,
        TypeDisposition::NoValue => return Some(false),
        TypeDisposition::Concrete => {}
    }
    match ty {
        MarrowType::Primitive(_) | MarrowType::Identity(_) | MarrowType::Enum(_) => Some(true),
        MarrowType::Sequence(element) => type_renderable_at_runtime(element),
        // An optional must be resolved before it renders (the one rule), so it is a
        // concrete non-renderable value here rather than a deferral.
        MarrowType::Optional(_)
        | MarrowType::Absent
        | MarrowType::Error
        | MarrowType::Resource(_)
        | MarrowType::GroupEntry { .. }
        | MarrowType::LocalTree { .. } => Some(false),
        MarrowType::Dynamic => unreachable!("dynamic values are handled by disposition"),
        MarrowType::Invalid => unreachable!("poisoned values are handled by disposition"),
        MarrowType::NoValue => unreachable!("no-value results are handled by disposition"),
        MarrowType::Unknown => unreachable!("recovery values are handled by disposition"),
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
pub(crate) fn marrow_type_name(names: &DeclIds<'_>, ty: &MarrowType) -> String {
    match ty {
        MarrowType::Primitive(scalar) => scalar.name().to_string(),
        MarrowType::Error => "Error".to_string(),
        MarrowType::Identity(root) => {
            format!("Id(^{})", names.root_spelling(*root).unwrap_or("?"))
        }
        MarrowType::Resource(resource) => names.resource_display(*resource),
        MarrowType::GroupEntry { resource, .. } => names.resource_display(*resource),
        MarrowType::Enum(id) => names
            .enum_owner_and_name(*id)
            .map_or_else(|| "unknown".to_string(), |(_, name)| name.to_string()),
        MarrowType::Sequence(element) => format!("sequence[{}]", marrow_type_name(names, element)),
        MarrowType::LocalTree { value, .. } => format!("tree[{}]", marrow_type_name(names, value)),
        MarrowType::Optional(inner) => format!("{}?", marrow_type_name(names, inner)),
        MarrowType::Absent => "absent".to_string(),
        MarrowType::Invalid => "value".to_string(),
        MarrowType::Dynamic | MarrowType::NoValue | MarrowType::Unknown => "unknown".to_string(),
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
    let poisoned = expected.contains_invalid() || actual.contains_invalid();
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
    if let (MarrowType::Enum(left_id), MarrowType::Enum(right_id)) = (left, right)
        && let Some((left_module, left_name)) = names.enum_owner_and_name(*left_id)
        && let Some((right_module, right_name)) = names.enum_owner_and_name(*right_id)
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

#[cfg(test)]
mod admission_tests {
    use std::path::Path;

    use marrow_store::value::ScalarType;
    use marrow_syntax::SourceSpan;

    use super::{
        Admission, CollectionFault, InferredBindingFault, KeyFault, KeyPolicy, RangeTypeAggregate,
        StrictValueFault, TypeDisposition, admit_collection_operand, admit_inferred_binding,
        admit_key, admit_strict_value, disposition, merge_key_admission, unresolved_optional,
    };
    use crate::MarrowType;

    fn int_type() -> MarrowType {
        MarrowType::Primitive(ScalarType::Int)
    }

    #[test]
    fn disposition_distinguishes_top_level_states_after_recursive_poison() {
        assert_eq!(
            disposition(&MarrowType::Sequence(Box::new(MarrowType::Invalid))),
            TypeDisposition::Poisoned,
        );
        assert_eq!(disposition(&MarrowType::Unknown), TypeDisposition::Recovery,);
        assert_eq!(
            disposition(&MarrowType::Dynamic),
            TypeDisposition::ExplicitDynamic,
        );
        assert_eq!(disposition(&MarrowType::NoValue), TypeDisposition::NoValue,);
        assert_eq!(disposition(&int_type()), TypeDisposition::Concrete);
    }

    #[test]
    fn disposition_keeps_non_poison_structural_states_top_level_concrete() {
        for ty in [
            MarrowType::Sequence(Box::new(MarrowType::Dynamic)),
            MarrowType::Optional(Box::new(MarrowType::Unknown)),
            MarrowType::LocalTree {
                keys: vec![MarrowType::NoValue],
                value: Box::new(int_type()),
            },
        ] {
            assert_eq!(disposition(&ty), TypeDisposition::Concrete, "{ty:?}");
        }
    }

    #[test]
    fn strict_value_admission_preserves_every_state_and_compatibility_fault() {
        let expected = int_type();
        let cases = [
            (MarrowType::Invalid, Admission::Poisoned),
            (
                MarrowType::Sequence(Box::new(MarrowType::Invalid)),
                Admission::Poisoned,
            ),
            (
                MarrowType::Unknown,
                Admission::Rejected(StrictValueFault::Recovery),
            ),
            (
                MarrowType::Dynamic,
                Admission::Rejected(StrictValueFault::ExplicitDynamic),
            ),
            (
                MarrowType::NoValue,
                Admission::Rejected(StrictValueFault::NoValue),
            ),
            (
                MarrowType::Optional(Box::new(int_type())),
                Admission::Rejected(StrictValueFault::Optional),
            ),
            (
                MarrowType::Primitive(ScalarType::Str),
                Admission::Rejected(StrictValueFault::Mismatch),
            ),
            (int_type(), Admission::Accepted),
        ];

        for (actual, expected_admission) in cases {
            assert_eq!(
                admit_strict_value(&expected, &actual),
                expected_admission,
                "{actual:?}",
            );
        }
    }

    #[test]
    fn strict_value_admission_defers_a_recursively_poisoned_expected_type() {
        let expected = MarrowType::LocalTree {
            keys: vec![MarrowType::Invalid],
            value: Box::new(int_type()),
        };

        assert_eq!(
            admit_strict_value(&expected, &int_type()),
            Admission::Poisoned,
        );
        assert_eq!(
            admit_strict_value(&MarrowType::Dynamic, &int_type()),
            Admission::Accepted,
        );
    }

    #[test]
    fn strict_value_admission_accepts_identical_structural_dynamic_types() {
        let ty = MarrowType::Sequence(Box::new(MarrowType::Dynamic));

        assert_eq!(admit_strict_value(&ty, &ty), Admission::Accepted);
    }

    #[test]
    fn strict_value_admission_defers_indeterminate_expected_shapes() {
        assert_eq!(
            admit_strict_value(&MarrowType::Unknown, &int_type()),
            Admission::Accepted,
        );
        assert_eq!(
            admit_strict_value(
                &MarrowType::Sequence(Box::new(MarrowType::Unknown)),
                &MarrowType::Sequence(Box::new(int_type())),
            ),
            Admission::Accepted,
        );
    }

    #[test]
    fn inferred_binding_admission_rejects_only_no_value_and_propagates_poison() {
        assert_eq!(
            admit_inferred_binding(&MarrowType::NoValue),
            Admission::Rejected(InferredBindingFault::NoValue),
        );
        assert_eq!(
            admit_inferred_binding(&MarrowType::Invalid),
            Admission::Poisoned,
        );
        for ty in [MarrowType::Unknown, MarrowType::Dynamic, int_type()] {
            assert_eq!(admit_inferred_binding(&ty), Admission::Accepted, "{ty:?}");
        }
    }

    #[test]
    fn key_admission_keeps_saved_and_local_dynamic_policies_distinct() {
        assert_eq!(
            admit_key(KeyPolicy::Saved, &int_type(), &MarrowType::Dynamic),
            Admission::Rejected(KeyFault::ExplicitDynamic),
        );
        assert_eq!(
            admit_key(KeyPolicy::Local, &int_type(), &MarrowType::Dynamic),
            Admission::Accepted,
        );
        for policy in [KeyPolicy::Saved, KeyPolicy::Local] {
            assert_eq!(
                admit_key(policy, &int_type(), &MarrowType::Unknown),
                Admission::Rejected(KeyFault::Recovery),
            );
            assert_eq!(
                admit_key(policy, &int_type(), &MarrowType::NoValue),
                Admission::Rejected(KeyFault::NoValue),
            );
        }
    }

    #[test]
    fn key_admission_propagates_recursive_poison_and_types_other_rejections() {
        let poisoned_tree = MarrowType::LocalTree {
            keys: vec![MarrowType::Invalid],
            value: Box::new(int_type()),
        };
        assert_eq!(
            admit_key(KeyPolicy::Saved, &int_type(), &poisoned_tree),
            Admission::Poisoned,
        );
        assert_eq!(
            admit_key(KeyPolicy::Local, &poisoned_tree, &int_type()),
            Admission::Poisoned,
        );
        assert_eq!(
            admit_key(
                KeyPolicy::Saved,
                &int_type(),
                &MarrowType::Optional(Box::new(int_type())),
            ),
            Admission::Rejected(KeyFault::Optional),
        );
        assert_eq!(
            admit_key(
                KeyPolicy::Saved,
                &int_type(),
                &MarrowType::Primitive(ScalarType::Str),
            ),
            Admission::Rejected(KeyFault::Mismatch),
        );
    }

    #[test]
    fn key_admission_merge_preserves_poison_then_rejection() {
        assert_eq!(
            merge_key_admission(Admission::Rejected(KeyFault::Mismatch), Admission::Poisoned,),
            Admission::Poisoned,
        );
        assert_eq!(
            merge_key_admission(Admission::Accepted, Admission::Rejected(KeyFault::Arity),),
            Admission::Rejected(KeyFault::Arity),
        );
    }

    #[test]
    fn range_type_aggregate_preserves_endpoint_order_and_step_state() {
        let expected = int_type();
        for aggregate in [
            RangeTypeAggregate::new(Some(expected.clone()), Some(MarrowType::Invalid), None),
            RangeTypeAggregate::new(Some(MarrowType::Invalid), Some(expected.clone()), None),
            RangeTypeAggregate::new(
                Some(expected.clone()),
                Some(expected.clone()),
                Some(MarrowType::Invalid),
            ),
        ] {
            assert_eq!(aggregate.admit_saved(&expected), Admission::Poisoned);
            assert_eq!(aggregate.representative_type(), MarrowType::Invalid);
        }
    }

    #[test]
    fn range_type_aggregate_keeps_non_poison_rejections_distinct() {
        let expected = int_type();
        for (component, fault) in [
            (MarrowType::Unknown, KeyFault::Recovery),
            (MarrowType::Dynamic, KeyFault::ExplicitDynamic),
            (MarrowType::NoValue, KeyFault::NoValue),
        ] {
            let aggregate = RangeTypeAggregate::new(Some(expected.clone()), Some(component), None);
            assert_eq!(aggregate.admit_saved(&expected), Admission::Rejected(fault),);
        }

        let compatible =
            RangeTypeAggregate::new(Some(expected.clone()), Some(expected.clone()), None);
        assert_eq!(compatible.admit_saved(&expected), Admission::Accepted);
        assert_eq!(compatible.representative_type(), expected);
    }

    #[test]
    fn collection_operand_admission_rejects_only_no_value_and_propagates_poison() {
        assert_eq!(
            admit_collection_operand(&MarrowType::NoValue),
            Admission::Rejected(CollectionFault::NoValue),
        );
        assert_eq!(
            admit_collection_operand(&MarrowType::Sequence(Box::new(MarrowType::Invalid))),
            Admission::Poisoned,
        );
        for accepted in [
            MarrowType::Unknown,
            MarrowType::Dynamic,
            MarrowType::Sequence(Box::new(int_type())),
        ] {
            assert_eq!(
                admit_collection_operand(&accepted),
                Admission::Accepted,
                "{accepted:?}",
            );
        }
    }

    #[test]
    fn optional_boundary_defers_recursive_poison() {
        let poisoned_optional = MarrowType::Optional(Box::new(MarrowType::Invalid));
        assert!(
            unresolved_optional(
                &int_type(),
                &poisoned_optional,
                SourceSpan::default(),
                Path::new("test.mw"),
            )
            .is_none(),
        );
    }
}
