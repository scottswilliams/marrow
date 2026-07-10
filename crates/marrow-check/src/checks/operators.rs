//! Condition, throw, return, and assignment type checks, and the unary/binary/
//! equality/coalesce operator rules. Each boundary distinguishes explicit dynamic
//! and no-value operands from unresolved or poisoned recovery states.

use std::collections::HashMap;
use std::path::Path;

use marrow_codes::Code;
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::infer::infer_type_with_read_scope;
use crate::model::decls::DeclIds;
use crate::typerules::{
    Admission, StrictValueFault, admit_strict_value, as_primitive, binary_symbol,
    expects_conversion, is_concrete_nonscalar, is_numeric, is_optional_value, is_ordered,
    marrow_type_name, mismatch_display, type_compatible, unary_symbol, unresolved_optional,
    unresolved_optional_diagnostic,
};
use crate::{
    CHECK_THROW_TYPE, CHECK_UNTYPED_VALUE, CheckDiagnostic, CheckedProgram, ConditionTypeFault,
    DiagnosticAnchor, DiagnosticPayload, MarrowType,
};

use super::diagnostics::operator_diagnostic;

/// Type-check an `if`/`while` condition (must be `bool`). Inferring it also
/// operator-checks it. Dynamic, no-value, and unresolved conditions take the
/// untyped-value path rather than being misreported as a wrong concrete scalar.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_condition(
    program: &CheckedProgram,
    file: &Path,
    condition: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    const_ints: &[HashMap<String, Option<i64>>],
    aliases: &HashMap<String, Vec<String>>,
    read_scope: crate::presence::ReadScope<'_>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    // A condition the parser could not structure carries its own parse error. Skip
    // the placeholder so it is not typed and does not stack a second diagnostic. The
    // per-file body gate already spares a function body this, but an evolve transform
    // body is type-checked ungated, so the poison-skip is load-bearing here.
    if condition.is_error() {
        return;
    }
    let condition_type = infer_type_with_read_scope(
        program,
        condition,
        scope,
        aliases,
        file,
        diagnostics,
        const_ints,
        read_scope,
    );
    let span = condition.span();
    if condition_type.contains_invalid() {
        return;
    }
    // A maybe-present value is not a `bool` until it is resolved; the one rule owns
    // it before the scalar gate so the message names the four resolution forms.
    if is_optional_value(&condition_type) {
        diagnostics.push(unresolved_optional_diagnostic(file, span));
        return;
    }
    let not_bool = |found: MarrowType| {
        CheckDiagnostic::new(
            Code::CheckConditionType,
            DiagnosticAnchor::at(file, span),
            DiagnosticPayload::ConditionType(ConditionTypeFault::NotBool { found }),
            &program.decl_ids(),
        )
    };
    match as_primitive(&condition_type) {
        Some(primitive) if primitive != ScalarType::Bool => {
            diagnostics.push(not_bool(condition_type))
        }
        // A dynamic, missing, or unresolved condition is untyped rather than a
        // wrong concrete type.
        None if matches!(
            condition_type,
            MarrowType::Dynamic | MarrowType::NoValue | MarrowType::Unknown
        ) =>
        {
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
            diagnostics.push(not_bool(condition_type))
        }
        None if is_concrete_nonscalar(&condition_type) => {
            diagnostics.push(not_bool(condition_type))
        }
        _ => {}
    }
}

/// Flag a `throw` whose operand is not an `Error`. Diagnosed poison defers to its
/// originating diagnostic; every other non-`Error` state is rejected here.
pub(crate) fn check_throw_type(
    names: &DeclIds<'_>,
    file: &Path,
    span: SourceSpan,
    value_type: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    match admit_strict_value(&MarrowType::Error, value_type) {
        Admission::Accepted | Admission::Poisoned => {}
        Admission::Rejected(StrictValueFault::Optional) => {
            diagnostics.push(unresolved_optional_diagnostic(file, span));
        }
        Admission::Rejected(
            StrictValueFault::Recovery
            | StrictValueFault::ExplicitDynamic
            | StrictValueFault::NoValue
            | StrictValueFault::Mismatch,
        ) => diagnostics.push(CheckDiagnostic::error(
            CHECK_THROW_TYPE,
            file,
            span,
            format!(
                "`throw` requires an `Error` value, found `{}`",
                marrow_type_name(names, value_type)
            ),
        )),
    }
}

/// Flag a `return` value whose type does not match the declared return type.
/// Fires only when both are known, incompatible types, so a void function or an
/// unresolved returned value is left alone. Value presence is checked separately
/// by `check.return_value`.
pub(crate) fn check_return_type(
    names: &DeclIds<'_>,
    file: &Path,
    span: SourceSpan,
    return_type: &MarrowType,
    value_type: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if return_type.contains_invalid() || value_type.contains_invalid() {
        return;
    }
    if let Some(diagnostic) = unresolved_optional(return_type, value_type, span, file) {
        diagnostics.push(diagnostic);
        return;
    }
    match type_compatible(return_type, value_type) {
        Some(true) => {}
        Some(false) => {
            diagnostics.push(CheckDiagnostic::new(
                Code::CheckReturnType,
                DiagnosticAnchor::at(file, span),
                DiagnosticPayload::TypeMismatch {
                    expected: return_type.clone(),
                    found: value_type.clone(),
                },
                names,
            ));
        }
        // Strict typing: an untyped value returned where a convertible type is
        // declared must be converted first. A return type with no conversion boundary
        // (void, a whole resource, a sequence) places no such constraint.
        None if matches!(
            value_type,
            MarrowType::Dynamic | MarrowType::NoValue | MarrowType::Unknown
        ) && expects_conversion(return_type) =>
        {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_UNTYPED_VALUE,
                file,
                span,
                format!(
                    "this `return` value has no known type, but the function returns `{}`; convert it first",
                    marrow_type_name(names, return_type),
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
    names: &DeclIds<'_>,
    file: &Path,
    span: SourceSpan,
    place: &MarrowType,
    value: &MarrowType,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if place.contains_invalid() || value.contains_invalid() {
        return;
    }
    if let Some(diagnostic) = unresolved_optional(place, value, span, file) {
        diagnostics.push(diagnostic);
        return;
    }
    let compatible = match (place, value) {
        (MarrowType::GroupEntry { resource, .. }, MarrowType::Resource(value_resource)) => {
            Some(resource == value_resource)
        }
        _ => type_compatible(place, value),
    };
    match compatible {
        Some(true) => {}
        Some(false) => {
            diagnostics.push(CheckDiagnostic::new(
                Code::CheckAssignmentType,
                DiagnosticAnchor::at(file, span),
                DiagnosticPayload::TypeMismatch {
                    expected: place.clone(),
                    found: value.clone(),
                },
                names,
            ));
        }
        // An untyped value stored into a place with a conversion boundary.
        None if matches!(
            value,
            MarrowType::Dynamic | MarrowType::NoValue | MarrowType::Unknown
        ) && expects_conversion(place) =>
        {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_UNTYPED_VALUE,
                file,
                span,
                format!(
                    "the value stored into `{}` has no known type; convert it before typed use",
                    marrow_type_name(names, place),
                ),
            ));
        }
        None => {}
    }
}

/// Validate a unary operator against its operand type, returning the result type,
/// [`MarrowType::Unknown`] when the operand is not a known primitive, or
/// [`MarrowType::Invalid`] when the operator is misused (which records a diagnostic),
/// so a reported fault poisons the result rather than cascading an untyped-value error.
pub(crate) fn check_unary(
    names: &DeclIds<'_>,
    op: marrow_syntax::UnaryOp,
    operand: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::UnaryOp;
    if operand.contains_invalid() {
        return MarrowType::Invalid;
    }
    // A maybe-present operand must be resolved before any operator; the one rule
    // owns it before the generic mismatch so the message names the four resolution
    // forms rather than reading as a bare operator-type error.
    if is_optional_value(operand) {
        diagnostics.push(unresolved_optional_diagnostic(file, span));
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
                marrow_type_name(names, operand),
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
        return MarrowType::Invalid;
    }
    MarrowType::Primitive(operand)
}

/// Validate a binary operator against its operand types, returning the result
/// type, [`MarrowType::Unknown`] when either operand is not a known primitive, or
/// [`MarrowType::Invalid`] when the operator is misused (which records a diagnostic),
/// so a reported fault poisons the result rather than cascading an untyped-value error.
pub(crate) fn check_binary(
    names: &DeclIds<'_>,
    op: marrow_syntax::BinaryOp,
    left: &MarrowType,
    right: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> MarrowType {
    use marrow_syntax::BinaryOp;
    if left.contains_invalid() || right.contains_invalid() {
        return MarrowType::Invalid;
    }
    // A maybe-present operand must be resolved before any operator — equality,
    // comparison, or arithmetic alike. The one rule owns it before the range and
    // generic-mismatch paths so an optional range endpoint is resolved and the message
    // names the four resolution forms.
    if is_optional_value(left) || is_optional_value(right) {
        diagnostics.push(unresolved_optional_diagnostic(file, span));
        return MarrowType::Invalid;
    }
    // `..`/`..=` are loop shapes, not value operators. The range-for header check
    // owns every endpoint diagnostic — a non-steppable, non-scalar, or mismatched
    // endpoint pair is `check.range` — so this path never reports an operator-type
    // error for a range, handling it before the `Error` and non-scalar gates below.
    // Type the range from a scalar left endpoint; any other endpoint leaves it
    // untyped, and the range-value rule catches a range misused as a value.
    if matches!(op, BinaryOp::RangeExclusive | BinaryOp::RangeInclusive) {
        return as_primitive(left)
            .map(MarrowType::Primitive)
            .unwrap_or(MarrowType::Unknown);
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
        && let Some(result) = check_equality(names, op, left, right, span, file, diagnostics)
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
                marrow_type_name(names, left),
                marrow_type_name(names, right),
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
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => {
            unreachable!("ranges are typed and endpoint-checked before reaching this match")
        }
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
        return MarrowType::Invalid;
    }
    result
}

/// Decide `==`/`!=` over concrete non-scalar operands, returning `Some(result)`
/// once a verdict is reached and `None` to defer to the scalar path. A rejected
/// pairing yields poison so an enclosing boundary does not diagnose the same fault.
fn check_equality(
    names: &DeclIds<'_>,
    op: marrow_syntax::BinaryOp,
    left: &MarrowType,
    right: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<MarrowType> {
    let reject = |diagnostics: &mut Vec<CheckDiagnostic>| {
        let (left_name, right_name) = mismatch_display(names, left, right);
        diagnostics.push(operator_diagnostic(
            file,
            span,
            format!(
                "operator `{}` cannot compare `{left_name}` and `{right_name}`",
                binary_symbol(op),
            ),
        ));
        Some(MarrowType::Invalid)
    };
    match (left, right) {
        (MarrowType::Invalid, _) | (_, MarrowType::Invalid) => None,
        // A maybe-present operand is the one rule, resolved in `check_binary` before
        // equality dispatch, so it never reaches here.
        (MarrowType::Optional(_) | MarrowType::Absent, _)
        | (_, MarrowType::Optional(_) | MarrowType::Absent) => {
            unreachable!("an optional operand is resolved by the one rule before check_equality")
        }
        // Dynamic values and non-value/recovery states defer to their owning gates.
        (MarrowType::Dynamic | MarrowType::NoValue | MarrowType::Unknown, _)
        | (_, MarrowType::Dynamic | MarrowType::NoValue | MarrowType::Unknown) => None,
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
        (MarrowType::Enum(_), MarrowType::Enum(_)) => {
            if left == right {
                Some(MarrowType::Primitive(ScalarType::Bool))
            } else {
                reject(diagnostics)
            }
        }
        // An enum against a scalar or `Error` is a category error.
        (MarrowType::Enum(_), _) | (_, MarrowType::Enum(_)) => reject(diagnostics),
        // Two scalars (or `Error`, which the caller already rejected) defer to the
        // ordinary scalar-equality path.
        (
            MarrowType::Primitive(_) | MarrowType::Error,
            MarrowType::Primitive(_) | MarrowType::Error,
        ) => None,
    }
}

/// Type-check `place ?? default`. The left must be an optional value (`Optional(T)`
/// or the empty `absent`); a present, non-optional left has nothing to default and
/// is rejected. The result follows the **right** operand's presence, so chains type:
/// `T? ?? T = T`, `T? ?? T? = T?`. The right's base must be compatible with `T`.
pub(crate) struct CoalesceCheck<'a> {
    pub(crate) names: &'a DeclIds<'a>,
    pub(crate) left_type: &'a MarrowType,
    pub(crate) right_type: &'a MarrowType,
    pub(crate) span: SourceSpan,
    pub(crate) file: &'a Path,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

pub(crate) fn check_coalesce(check: CoalesceCheck<'_>) -> MarrowType {
    let CoalesceCheck {
        names,
        left_type,
        right_type,
        span,
        file,
        diagnostics,
    } = check;
    if left_type.contains_invalid() || right_type.contains_invalid() {
        return MarrowType::Invalid;
    }
    // The left's present-arm type, or `None` for the empty `absent` (which carries no
    // element type, so it takes its result type entirely from the default).
    let inner = match left_type {
        MarrowType::Optional(inner) => Some(inner.as_ref()),
        MarrowType::Absent => None,
        // An untyped left may be an unresolved maybe-present read; defer to the
        // default's type rather than assert it is always present, so a cross-module
        // unknown read is not turned into operator noise.
        MarrowType::Dynamic | MarrowType::NoValue | MarrowType::Unknown => {
            return right_type.clone();
        }
        _ => {
            diagnostics.push(operator_diagnostic(
                file,
                span,
                "operator `??` applies only to an optional value; this value is always present"
                    .to_string(),
            ));
            // The left is already present, so recover to its type: a consumer slot then
            // reads the value it would have, and no second `check.untyped_value` stacks
            // on the one always-present error.
            return left_type.clone();
        }
    };
    // The default may itself be optional (`a ?? b ?? c`); compare against its present
    // arm and carry its presence into the result.
    let right_optional = matches!(right_type, MarrowType::Optional(_));
    let default_base = match right_type {
        MarrowType::Optional(base) => base.as_ref(),
        other => other,
    };
    let resolved = match inner {
        // `absent ?? default` has no left element type to satisfy; the result is the
        // default's present arm.
        None => default_base.clone(),
        Some(inner) => match coalesce_base(names, inner, default_base, span, file, diagnostics) {
            Some(ty) => ty,
            None => return MarrowType::Invalid,
        },
    };
    if right_optional {
        MarrowType::optional(resolved)
    } else {
        resolved
    }
}

/// Resolve the present-arm result of `inner ?? default_base`, both non-optional. A
/// concrete non-scalar defaults only with the same nominal type; two scalars must
/// match; an untyped side defers. `None` signals a reported mismatch.
fn coalesce_base(
    names: &DeclIds<'_>,
    inner: &MarrowType,
    default_base: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> Option<MarrowType> {
    if is_concrete_nonscalar(inner) || is_concrete_nonscalar(default_base) {
        return match type_compatible(inner, default_base) {
            Some(true) | None => Some(inner.clone()),
            Some(false) => {
                diagnostics.push(operator_diagnostic(
                    file,
                    span,
                    format!(
                        "operator `??` cannot default `{}` with `{}`",
                        marrow_type_name(names, inner),
                        marrow_type_name(names, default_base),
                    ),
                ));
                None
            }
        };
    }
    match (as_primitive(inner), as_primitive(default_base)) {
        (Some(leaf), Some(default)) if leaf == default => Some(MarrowType::Primitive(leaf)),
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
            None
        }
        // An untyped leaf falls back to the default's type; an untyped default leaves
        // the result the leaf type. Either way an unknown stays unknown.
        (None, _) => Some(default_base.clone()),
        (Some(leaf), None) => Some(MarrowType::Primitive(leaf)),
    }
}
