//! Range-for header rules and range-as-value rejection: endpoint typing, the `by`
//! step type and direction, temporal/date step constraints, and the dead-loop
//! detector. Endpoint typing is owned by the type pass; a non-steppable or
//! mismatched endpoint pair is left to the operator check.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;

use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::infer::infer_only;
use crate::typerules::{as_primitive, is_steppable};
use crate::walk::for_each_child_expr;
use crate::{CHECK_RANGE_VALUE, CheckDiagnostic, CheckedProgram, MarrowType};

use super::diagnostics::range_diagnostic;

/// The endpoint scalar type of a range iterable when both endpoints are the same
/// steppable type, or `None` for any other iterable or a mismatched/non-steppable
/// pair. A range binds its loop variable to this type.
pub(super) fn range_endpoint_type(
    program: &CheckedProgram,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<MarrowType> {
    let (left, right) = range_endpoints(iterable)?;
    let left = infer_only(program, left, scope, aliases, file);
    let right = infer_only(program, right, scope, aliases, file);
    match (as_primitive(&left), as_primitive(&right)) {
        (Some(lo), Some(hi)) if lo == hi && is_steppable(lo) => Some(MarrowType::Primitive(lo)),
        _ => None,
    }
}

/// The two endpoint expressions of a range, or `None` for a non-range iterable.
fn range_endpoints(
    iterable: &marrow_syntax::Expression,
) -> Option<(&marrow_syntax::Expression, &marrow_syntax::Expression)> {
    let range = marrow_syntax::range_expr(iterable)?;
    Some((range.start?, range.end?))
}

/// Reject ranges outside `for` iterables. A range is a loop shape, not a value
/// that can be stored, returned, thrown, passed, or evaluated for its own sake.
pub(crate) fn check_range_value(
    file: &Path,
    expr: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    check_range_value_guarded(file, expr, &|_| false, diagnostics);
}

/// The single owner of the [`CHECK_RANGE_VALUE`] emit and message: reject ranges
/// outside `for` iterables, skipping any subexpression `allowed` exempts (and its
/// children) so a context that legitimately accepts a range — a saved key-range
/// argument to `exists`/`count` — is not flagged.
pub(crate) fn check_range_value_guarded(
    file: &Path,
    expr: &marrow_syntax::Expression,
    allowed: &impl Fn(&marrow_syntax::Expression) -> bool,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if allowed(expr) {
        return;
    }
    if let Some(range) = marrow_syntax::range_expr(expr) {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_RANGE_VALUE,
            file,
            range.span,
            "a range can only be used as a `for` iterable",
        ));
    }
    for_each_child_expr(expr, |child| {
        check_range_value_guarded(file, child, allowed, diagnostics)
    });
}

pub(super) fn check_range_iterable_value_parts(
    file: &Path,
    iterable: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if range_endpoints(iterable).is_some() {
        return;
    }
    // A for-iterable that is itself a range but is missing an endpoint is an
    // ill-formed range header, not a range used outside a `for`. Report the
    // missing endpoint here so the range-value rule does not claim a range is
    // forbidden where it is exactly what is expected.
    if let Some(range) = marrow_syntax::range_expr(iterable) {
        diagnostics.push(range_diagnostic(
            file,
            range.span,
            "a `for` range needs both endpoints (lo..hi or lo..=hi)".to_string(),
        ));
        return;
    }
    check_range_value(file, iterable, diagnostics);
}

/// Validate a range-for header's step and direction rules: the `by` step must
/// match the endpoint type (`int` endpoints step by `int`, temporal endpoints
/// step by `duration`), instant requires an explicit step, and a step that
/// statically cannot run (wrong-direction or zero) is a dead loop. A step on a
/// non-range iterable is rejected. Endpoint typing is owned by the type pass, so a
/// non-steppable or mismatched endpoint pair is left to it.
pub(crate) fn check_range_header(
    program: &CheckedProgram,
    file: &Path,
    iterable: &marrow_syntax::Expression,
    step: Option<&marrow_syntax::Expression>,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some((left, right)) = range_endpoints(iterable) else {
        // A step is only meaningful on a range; reject `by` on any other iterable.
        if let Some(step) = step {
            diagnostics.push(range_diagnostic(
                file,
                step.span(),
                "a `by` step applies only to a range".to_string(),
            ));
        }
        return;
    };
    let endpoint = match (
        as_primitive(&infer_only(program, left, scope, aliases, file)),
        as_primitive(&infer_only(program, right, scope, aliases, file)),
    ) {
        // A same-typed steppable endpoint pair is the only shape with step rules;
        // a non-steppable or mismatched pair is reported by the operator check.
        (Some(lo), Some(hi)) if lo == hi && is_steppable(lo) => lo,
        _ => return,
    };
    let step_type = step.map(|step| as_primitive(&infer_only(program, step, scope, aliases, file)));
    check_step_type(
        file,
        iterable.span(),
        endpoint,
        step,
        step_type,
        diagnostics,
    );
    check_temporal_step_sign(file, endpoint, step, diagnostics);
    check_date_step_whole_days(file, endpoint, step, diagnostics);
    check_dead_loop(file, iterable, left, right, step, diagnostics);
}

/// Reject a negated duration step on a `date`/`instant` range. A duration is
/// always non-negative, so a descending temporal range can only fault at runtime;
/// the check reports that guaranteed fault now. Int ranges still descend by a
/// negative step.
fn check_temporal_step_sign(
    file: &Path,
    endpoint: ScalarType,
    step: Option<&marrow_syntax::Expression>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if !matches!(endpoint, ScalarType::Date | ScalarType::Instant) {
        return;
    }
    let Some(step) = step else { return };
    if matches!(literal_int_sign(step), Some(sign) if sign < 0) {
        diagnostics.push(range_diagnostic(
            file,
            step.span(),
            format!(
                "{} range step must be a positive duration; descending temporal ranges are not yet supported",
                article_for(endpoint)
            ),
        ));
    }
}

/// Reject a literal duration step on a `date` range that is not a whole number of
/// days. A date has no time of day, so a sub-day step faults at runtime; the
/// checker reports that guaranteed fault now. `instant` carries a time component
/// and steps by any positive duration, so this rule is `date`-only. A non-literal
/// step is left to the runtime, which still faults on a fractional-day multiple.
fn check_date_step_whole_days(
    file: &Path,
    endpoint: ScalarType,
    step: Option<&marrow_syntax::Expression>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if endpoint != ScalarType::Date {
        return;
    }
    let Some(step) = step else { return };
    let Some(total_seconds) = literal_duration_seconds(step) else {
        return;
    };
    const SECONDS_PER_DAY: i64 = 86_400;
    if total_seconds % SECONDS_PER_DAY != 0 {
        diagnostics.push(range_diagnostic(
            file,
            step.span(),
            "a date range step must be a whole number of days".to_string(),
        ));
    }
}

/// The total seconds of a literal duration step (`1.hour` => 3600), or `None` for a
/// non-literal or non-duration step. A negation is read through to its magnitude;
/// the sign is handled separately.
fn literal_duration_seconds(expr: &marrow_syntax::Expression) -> Option<i64> {
    use marrow_syntax::{Expression, LiteralKind, UnaryOp, duration_unit_seconds};
    match expr {
        Expression::Literal {
            kind: LiteralKind::Duration,
            text,
            ..
        } => {
            let (magnitude, unit) = text.split_once('.')?;
            let magnitude: i64 = magnitude.parse().ok()?;
            magnitude.checked_mul(duration_unit_seconds(unit)?)
        }
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => literal_duration_seconds(operand),
        _ => None,
    }
}

/// The step-type rule: `int` endpoints step by `int`; date/instant endpoints
/// step by a duration. Instant has no safe default step, so omitting `by` there
/// is an error; int defaults to 1 and date to one calendar day. An untyped
/// (`unknown`) step defers.
fn check_step_type(
    file: &Path,
    range_span: SourceSpan,
    endpoint: ScalarType,
    step: Option<&marrow_syntax::Expression>,
    step_type: Option<Option<ScalarType>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let expected = match endpoint {
        ScalarType::Int => endpoint,
        // date and instant step by a duration span.
        _ => ScalarType::Duration,
    };
    match (step, step_type) {
        (Some(step), Some(Some(actual))) if actual != expected => {
            diagnostics.push(range_diagnostic(
                file,
                step.span(),
                format!(
                    "{} range steps by `{}`, not `{}`",
                    article_for(endpoint),
                    expected.name(),
                    actual.name(),
                ),
            ));
        }
        (Some(_), _) => {}
        // No `by`: instant requires one; int and date have a default.
        (None, _) => {
            if endpoint == ScalarType::Instant {
                diagnostics.push(range_diagnostic(
                    file,
                    range_span,
                    format!(
                        "{} range needs an explicit `by` step",
                        article_for(endpoint)
                    ),
                ));
            }
        }
    }
}

/// A scalar named with its indefinite article and backtick spelling (`` an `int` ``)
/// so a range diagnostic reads naturally. `int` and `instant` are the vowel-initial
/// steppable spellings.
fn article_for(scalar: ScalarType) -> String {
    let article = if matches!(scalar, ScalarType::Int | ScalarType::Instant) {
        "an"
    } else {
        "a"
    };
    format!("{article} `{}`", scalar.name())
}

/// Reject a step that statically can never run. A zero step never progresses; a
/// literal wrong-direction step over literal endpoints (`1..10 by -1`) is a dead
/// loop. A variable endpoint or step is left to the runtime, where a wrong
/// direction is simply an empty loop and a zero step faults.
fn check_dead_loop(
    file: &Path,
    iterable: &marrow_syntax::Expression,
    left: &marrow_syntax::Expression,
    right: &marrow_syntax::Expression,
    step: Option<&marrow_syntax::Expression>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(step_sign) = literal_step_sign(step) else {
        return;
    };
    if step_sign == Ordering::Equal {
        diagnostics.push(range_diagnostic(
            file,
            iterable.span(),
            "a range step cannot be zero".to_string(),
        ));
        return;
    }
    // The endpoints' relative order. A mismatched or non-literal pair yields
    // `None` and is left to the runtime.
    let endpoints = literal_int_value(left)
        .zip(literal_int_value(right))
        .map(|(lo, hi)| lo.cmp(&hi));
    let Some(endpoints) = endpoints else {
        return;
    };
    // An ascending step needs lo <= hi to run; a descending step needs lo >= hi.
    // Equal endpoints with `..` are also empty, but that is a legitimate empty loop,
    // not a wrong-direction bug, so only a provably wrong direction is flagged.
    let wrong_direction = (step_sign == Ordering::Greater && endpoints == Ordering::Greater)
        || (step_sign == Ordering::Less && endpoints == Ordering::Less);
    if wrong_direction {
        diagnostics.push(range_diagnostic(
            file,
            iterable.span(),
            "this range steps away from its end and never runs".to_string(),
        ));
    }
}

/// The direction of a literal step against zero — `Greater` ascending, `Less`
/// descending, `Equal` for a zero step — or `None` for a non-literal step (or an
/// omitted one, which defaults to the ascending unit step).
fn literal_step_sign(step: Option<&marrow_syntax::Expression>) -> Option<Ordering> {
    let Some(step) = step else {
        return Some(Ordering::Greater);
    };
    literal_int_sign(step).map(|sign| sign.cmp(&0))
}

/// The signed value of a literal integer expression (`5`, `-1`), or `None` for a
/// non-literal or non-integer literal.
fn literal_int_value(expr: &marrow_syntax::Expression) -> Option<i64> {
    use marrow_syntax::{Expression, LiteralKind, UnaryOp};
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => text.parse().ok(),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => literal_int_value(operand).and_then(i64::checked_neg),
        _ => None,
    }
}

/// The sign (-1, 0, +1) of a literal step — an integer literal or a duration
/// literal, optionally negated — or `None` for a non-literal step. A duration
/// literal's magnitude carries the sign through the unary negation.
fn literal_int_sign(expr: &marrow_syntax::Expression) -> Option<i64> {
    use marrow_syntax::{Expression, LiteralKind, UnaryOp};
    match expr {
        Expression::Literal {
            kind: LiteralKind::Integer | LiteralKind::Duration,
            text,
            ..
        } => duration_or_int_magnitude(text).map(i64::signum),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => literal_int_sign(operand).map(|sign| -sign),
        _ => None,
    }
}

/// The leading magnitude of an int or duration literal as a signed `i64` for a
/// sign test: an int literal's value, or a duration literal's count before its
/// unit (`1.day` => 1). Saturates so a huge magnitude still reports its sign.
fn duration_or_int_magnitude(text: &str) -> Option<i64> {
    let digits = text.split('.').next().unwrap_or(text);
    digits
        .parse::<i64>()
        .ok()
        .or(Some(if digits.is_empty() { 0 } else { i64::MAX }))
}
