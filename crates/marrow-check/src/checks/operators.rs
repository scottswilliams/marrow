//! Condition, throw, return, and assignment type checks, and the unary/binary/
//! equality/coalesce operator rules. Each fires only on a known wrong or untyped
//! type, deferring on `Unknown` so an uncertain operand never false-positives.

use std::collections::HashMap;
use std::path::Path;

use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::infer::infer_type_with_read_scope;
use crate::typerules::{
    as_primitive, binary_symbol, expects_conversion, is_concrete_nonscalar, is_numeric, is_ordered,
    is_steppable, marrow_type_name, mismatch_display, type_compatible, unary_symbol,
};
use crate::{
    CHECK_ASSIGNMENT_TYPE, CHECK_CONDITION_TYPE, CHECK_RETURN_TYPE, CHECK_THROW_TYPE,
    CHECK_UNTYPED_VALUE, CheckDiagnostic, CheckedProgram, DiagnosticPayload, MarrowType,
};

use super::diagnostics::operator_diagnostic;

/// Type-check an `if`/`while` condition (must be `bool`). Inferring it also
/// operator-checks it. An unknown type — an unresolved call, a saved-data read — is
/// left alone, so the check never fires on an uncertain condition.
pub(crate) fn check_condition(
    program: &CheckedProgram,
    file: &Path,
    condition: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    transform_old: Option<crate::presence::TransformOldReadScope<'_>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let condition_type = infer_type_with_read_scope(
        program,
        condition,
        scope,
        aliases,
        file,
        diagnostics,
        transform_old,
    );
    let span = condition.span();
    match as_primitive(&condition_type) {
        Some(primitive) if primitive != ScalarType::Bool => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_CONDITION_TYPE,
                file,
                span,
                format!("condition must be `bool`, found `{}`", primitive.name()),
            ))
        }
        // An unresolved condition is untyped rather than a wrong type, since strict
        // typing cannot show it to be `bool`.
        None if matches!(condition_type, MarrowType::Unknown) => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_UNTYPED_VALUE,
                file,
                span,
                "condition has no known type; it must be `bool`",
            ))
        }
        // `Error` and other concrete non-scalars are known types, not unknown ones,
        // so they are flagged like a wrong scalar rather than swallowed.
        None if matches!(condition_type, MarrowType::Error) => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_CONDITION_TYPE,
                file,
                span,
                "condition must be `bool`, found `Error`",
            ))
        }
        None if is_concrete_nonscalar(&condition_type) => diagnostics.push(CheckDiagnostic::error(
            CHECK_CONDITION_TYPE,
            file,
            span,
            format!(
                "condition must be `bool`, found `{}`",
                marrow_type_name(&condition_type)
            ),
        )),
        _ => {}
    }
}

/// Flag a `throw` whose operand is known to be something other than `Error`.
/// Unknown operands are left to the runtime backstop, as with other unresolved
/// values in this pass.
pub(crate) fn check_throw_type(
    file: &Path,
    span: SourceSpan,
    value_type: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match value_type {
        MarrowType::Error | MarrowType::Unknown => {}
        _ => diagnostics.push(CheckDiagnostic::error(
            CHECK_THROW_TYPE,
            file,
            span,
            format!(
                "`throw` requires an `Error` value, found `{}`",
                marrow_type_name(value_type)
            ),
        )),
    }
}

/// Flag a `return` value whose type does not match the declared return type.
/// Fires only when both are known, incompatible types, so a void function or an
/// unresolved returned value is left alone. Value presence is checked separately
/// by `check.return_value`.
pub(crate) fn check_return_type(
    file: &Path,
    span: SourceSpan,
    return_type: &MarrowType,
    value_type: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match type_compatible(return_type, value_type) {
        Some(true) => {}
        Some(false) => diagnostics.push(
            CheckDiagnostic::error(
                CHECK_RETURN_TYPE,
                file,
                span,
                format!(
                    "function returns `{}`, but this value is `{}`",
                    marrow_type_name(return_type),
                    marrow_type_name(value_type),
                ),
            )
            .with_payload(DiagnosticPayload::TypeMismatch {
                expected: return_type.clone(),
                found: value_type.clone(),
            }),
        ),
        // Strict typing: an untyped value returned where a convertible type is
        // declared must be converted first. A return type with no conversion boundary
        // (void, a whole resource, a sequence) places no such constraint.
        None if matches!(value_type, MarrowType::Unknown) && expects_conversion(return_type) => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_UNTYPED_VALUE,
                file,
                span,
                format!(
                    "this `return` value has no known type, but the function returns `{}`; convert it first",
                    marrow_type_name(return_type),
                ),
            ));
        }
        None => {}
    }
}

/// Flag a value stored into a concrete place when its type is wrong (a
/// `check.assignment_type` mismatch) or untyped against a conversion boundary (a
/// `check.untyped_value`, under strict typing). An untyped place (a sequence,
/// `unknown`) is left alone. A whole group-entry assignment may take a value of the
/// owning resource type, since the runtime writes matching fields from that value
/// into the addressed entry.
pub(crate) fn check_assignment(
    file: &Path,
    span: SourceSpan,
    place: &MarrowType,
    value: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let compatible = match (place, value) {
        (MarrowType::GroupEntry { resource, .. }, MarrowType::Resource(value_resource)) => {
            Some(resource == value_resource)
        }
        _ => type_compatible(place, value),
    };
    match compatible {
        Some(true) => {}
        Some(false) => {
            let (expected, found) = mismatch_display(place, value);
            diagnostics.push(
                CheckDiagnostic::error(
                    CHECK_ASSIGNMENT_TYPE,
                    file,
                    span,
                    format!("expected `{expected}`, but the value is `{found}`"),
                )
                .with_payload(DiagnosticPayload::TypeMismatch {
                    expected: place.clone(),
                    found: value.clone(),
                }),
            );
        }
        // An untyped value stored into a place with a conversion boundary.
        None if matches!(value, MarrowType::Unknown) && expects_conversion(place) => {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_UNTYPED_VALUE,
                file,
                span,
                format!(
                    "the value stored into `{}` has no known type; convert it before typed use",
                    marrow_type_name(place),
                ),
            ));
        }
        None => {}
    }
}

/// Validate a unary operator against its operand type, returning the result type,
/// or [`MarrowType::Unknown`] when the operand is not a known primitive or the
/// operator is misused (which records a diagnostic).
pub(crate) fn check_unary(
    op: marrow_syntax::UnaryOp,
    operand: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::UnaryOp;
    if matches!(operand, MarrowType::Invalid) {
        return MarrowType::Invalid;
    }
    // A concrete non-scalar operand (an identity, record, sequence, or the
    // checker-only `Error`) has no unary operator. Flag it before the `as_primitive`
    // gate, which would otherwise drop every non-primitive to `Unknown`.
    if matches!(operand, MarrowType::Error) || is_concrete_nonscalar(operand) {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}`",
                unary_symbol(op),
                marrow_type_name(operand),
            ),
        ));
        return MarrowType::Invalid;
    }
    let Some(operand) = as_primitive(operand) else {
        return MarrowType::Unknown;
    };
    let valid = match op {
        UnaryOp::Neg => is_numeric(operand),
        UnaryOp::Not => operand == ScalarType::Bool,
    };
    if !valid {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}`",
                unary_symbol(op),
                operand.name(),
            ),
        ));
        return MarrowType::Unknown;
    }
    MarrowType::Primitive(operand)
}

/// Validate a binary operator against its operand types, returning the result
/// type, or [`MarrowType::Unknown`] when either operand is not a known primitive
/// or the operator is misused (which records a diagnostic).
pub(crate) fn check_binary(
    op: marrow_syntax::BinaryOp,
    left: &MarrowType,
    right: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::BinaryOp;
    if matches!(left, MarrowType::Invalid) || matches!(right, MarrowType::Invalid) {
        return MarrowType::Invalid;
    }
    // `Error` is a concrete type with no binary operator. Flag it before the
    // `as_primitive` gate, which would otherwise drop it to `Unknown`.
    if matches!(left, MarrowType::Error) || matches!(right, MarrowType::Error) {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `Error`",
                binary_symbol(op)
            ),
        ));
        return MarrowType::Invalid;
    }
    // Equality over concrete non-scalars is decided before the `as_primitive` gate;
    // an `Unknown` operand defers to the scalar path.
    if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual)
        && let Some(result) = check_equality(op, left, right, span, file, diagnostics)
    {
        return result;
    }
    // No non-equality operator applies to a concrete non-scalar operand. Flag it
    // before the scalar gate; an `Unknown` operand still defers there.
    if is_concrete_nonscalar(left) || is_concrete_nonscalar(right) {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}` and `{}`",
                binary_symbol(op),
                marrow_type_name(left),
                marrow_type_name(right),
            ),
        ));
        return MarrowType::Invalid;
    }
    let (Some(left), Some(right)) = (as_primitive(left), as_primitive(right)) else {
        return MarrowType::Unknown;
    };
    // Each arm is (operator accepts these operands, result type when it does).
    let (valid, result) = match op {
        BinaryOp::Add => match (left, right) {
            (ScalarType::Str, ScalarType::Str) => (true, MarrowType::Primitive(ScalarType::Str)),
            (ScalarType::Instant, ScalarType::Duration) => {
                (true, MarrowType::Primitive(ScalarType::Instant))
            }
            (ScalarType::Duration, ScalarType::Duration) => {
                (true, MarrowType::Primitive(ScalarType::Duration))
            }
            _ => (
                is_numeric(left) && left == right,
                MarrowType::Primitive(left),
            ),
        },
        BinaryOp::Subtract => match (left, right) {
            (ScalarType::Instant, ScalarType::Instant) => {
                (true, MarrowType::Primitive(ScalarType::Duration))
            }
            (ScalarType::Instant, ScalarType::Duration) => {
                (true, MarrowType::Primitive(ScalarType::Instant))
            }
            (ScalarType::Duration, ScalarType::Duration) => {
                (true, MarrowType::Primitive(ScalarType::Duration))
            }
            _ => (
                is_numeric(left) && left == right,
                MarrowType::Primitive(left),
            ),
        },
        BinaryOp::Multiply => (
            is_numeric(left) && left == right,
            MarrowType::Primitive(left),
        ),
        BinaryOp::Divide => (
            is_numeric(left) && left == right,
            MarrowType::Primitive(ScalarType::Decimal),
        ),
        BinaryOp::Remainder => (
            left == ScalarType::Int && right == ScalarType::Int,
            MarrowType::Primitive(ScalarType::Int),
        ),
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => (
            is_ordered(left) && left == right,
            MarrowType::Primitive(ScalarType::Bool),
        ),
        BinaryOp::Equal | BinaryOp::NotEqual => {
            (left == right, MarrowType::Primitive(ScalarType::Bool))
        }
        BinaryOp::And | BinaryOp::Or => (
            left == ScalarType::Bool && right == ScalarType::Bool,
            MarrowType::Primitive(ScalarType::Bool),
        ),
        // A range is not a value an operator consumes; accept two endpoints of the
        // same steppable type. The endpoint typing, step, and direction rules are a
        // separate range-for check, so this only rejects a non-steppable or
        // mismatched endpoint pairing.
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => (
            is_steppable(left) && left == right,
            MarrowType::Primitive(left),
        ),
        BinaryOp::Coalesce | BinaryOp::Is => {
            unreachable!(
                "`??` and `is` are typed in check_coalesce/check_is before reaching check_binary"
            )
        }
    };
    if !valid {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot be applied to `{}` and `{}`",
                binary_symbol(op),
                left.name(),
                right.name(),
            ),
        ));
        return MarrowType::Unknown;
    }
    result
}

/// Decide `==`/`!=` over concrete non-scalar operands, returning `Some(result)`
/// once a verdict is reached and `None` to defer to the scalar path. A rejected
/// pairing still yields `bool`, the natural result type of a comparison.
fn check_equality(
    op: marrow_syntax::BinaryOp,
    left: &MarrowType,
    right: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<MarrowType> {
    let reject = |diagnostics: &mut Vec<CheckDiagnostic>| {
        let (left_name, right_name) = mismatch_display(left, right);
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot compare `{left_name}` and `{right_name}`",
                binary_symbol(op),
            ),
        ));
        Some(MarrowType::Primitive(ScalarType::Bool))
    };
    match (left, right) {
        (MarrowType::Invalid, _) | (_, MarrowType::Invalid) => None,
        // An untyped operand defers: the scalar path handles untyped values.
        (MarrowType::Unknown, _) | (_, MarrowType::Unknown) => None,
        // Whole records and sequences have no equality at all.
        (
            MarrowType::Resource(_)
            | MarrowType::GroupEntry { .. }
            | MarrowType::Sequence(_)
            | MarrowType::LocalTree { .. },
            _,
        )
        | (
            _,
            MarrowType::Resource(_)
            | MarrowType::GroupEntry { .. }
            | MarrowType::Sequence(_)
            | MarrowType::LocalTree { .. },
        ) => reject(diagnostics),
        // Identities compare nominally: equatable only against the same resource.
        (MarrowType::Identity(a), MarrowType::Identity(b)) => {
            if a == b {
                Some(MarrowType::Primitive(ScalarType::Bool))
            } else {
                reject(diagnostics)
            }
        }
        // An identity against a scalar, enum, or `Error` is a category error.
        (MarrowType::Identity(_), _) | (_, MarrowType::Identity(_)) => reject(diagnostics),
        // Enums compare nominally: equatable only against the same enum, by owning
        // module and name, so two same-named enums in different modules are not.
        (MarrowType::Enum { .. }, MarrowType::Enum { .. }) => {
            if left == right {
                Some(MarrowType::Primitive(ScalarType::Bool))
            } else {
                reject(diagnostics)
            }
        }
        // An enum against a scalar or `Error` is a category error.
        (MarrowType::Enum { .. }, _) | (_, MarrowType::Enum { .. }) => reject(diagnostics),
        // Two scalars (or `Error`, which the caller already rejected) defer to the
        // ordinary scalar-equality path.
        (
            MarrowType::Primitive(_) | MarrowType::Error,
            MarrowType::Primitive(_) | MarrowType::Error,
        ) => None,
    }
}

/// Type-check `path ?? default`. The result is the leaf type of the path read on
/// the left (a populated read yields that value; an absent one yields the
/// default), so the default must be the same scalar type. A non-path left operand
/// is rejected: only a read that can be absent has anything to default.
pub(crate) struct CoalesceCheck<'a> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) left: &'a marrow_syntax::Expression,
    pub(crate) left_type: &'a MarrowType,
    pub(crate) right_type: &'a MarrowType,
    pub(crate) span: SourceSpan,
    pub(crate) file: &'a Path,
    pub(crate) scope: &'a [HashMap<String, MarrowType>],
    pub(crate) transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

pub(crate) fn check_coalesce(check: CoalesceCheck<'_>) -> MarrowType {
    let CoalesceCheck {
        program,
        left,
        left_type,
        right_type,
        span,
        file,
        scope,
        transform_old,
        diagnostics,
    } = check;
    if matches!(left_type, MarrowType::Invalid) || matches!(right_type, MarrowType::Invalid) {
        return MarrowType::Invalid;
    }
    let Some(left) = crate::executable::lower_expr_for_file(program, file, left, scope) else {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            "operator `??` applies only to a path read or `?.` chain".to_string(),
        ));
        return MarrowType::Unknown;
    };
    if !crate::presence::read_resolves_in_type_scope(program, &left, scope, transform_old) {
        diagnostics.push(operator_diagnostic(
            file,
            span,
            "operator `??` applies only to a path read or `?.` chain".to_string(),
        ));
        return MarrowType::Unknown;
    }
    // A concrete non-scalar leaf defaults only with a value of the same nominal
    // type. The scalar path below would drop it to `Unknown` and silently accept a
    // mismatch, so resolve any non-scalar pairing here; an `Unknown` operand still
    // defers there.
    if is_concrete_nonscalar(left_type) || is_concrete_nonscalar(right_type) {
        return match type_compatible(left_type, right_type) {
            Some(true) => left_type.clone(),
            Some(false) => {
                diagnostics.push(operator_diagnostic(
                    file,
                    span,
                    format!(
                        "operator `??` cannot default `{}` with `{}`",
                        marrow_type_name(left_type),
                        marrow_type_name(right_type),
                    ),
                ));
                MarrowType::Unknown
            }
            None => left_type.clone(),
        };
    }
    // Both sides must be the same scalar, like the other value operators. When
    // either is still untyped, defer rather than guess, yielding the known side
    // (or `Unknown`) so a surrounding operator never fires on an uncertain operand.
    match (as_primitive(left_type), as_primitive(right_type)) {
        (Some(leaf), Some(default)) if leaf == default => MarrowType::Primitive(leaf),
        (Some(leaf), Some(default)) => {
            diagnostics.push(operator_diagnostic(
                file,
                span,
                format!(
                    "operator `??` cannot default `{}` with `{}`",
                    leaf.name(),
                    default.name(),
                ),
            ));
            MarrowType::Unknown
        }
        // An untyped leaf falls back to the default's type; an untyped default
        // leaves the result the leaf type. Either way an unknown stays unknown.
        (None, _) => right_type.clone(),
        (Some(leaf), None) => MarrowType::Primitive(leaf),
    }
}
