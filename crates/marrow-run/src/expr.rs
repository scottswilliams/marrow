//! Pure-expression evaluation: literals, operators, and coercions.

use std::cmp::Ordering;

use marrow_check::{
    CheckedBinaryOp as BinaryOp, CheckedEnumMemberRef, CheckedExpr as ExecExpr,
    CheckedInterpolationPart as InterpolationPart, CheckedLiteralKind as LiteralKind,
    CheckedUnaryOp as UnaryOp, MarrowType,
};
use marrow_store::Decimal;
use marrow_store::value::supported_instant_nanos;
use marrow_syntax::{
    BytesLiteralError, SourceSpan, StringLiteralError, decode_bytes_literal, decode_string_escapes,
    decode_string_literal, duration_unit_seconds,
};

use crate::call::eval_call;
use crate::durable_read::{
    eval_optional_field, eval_saved_field, read_resource, read_saved_value_if_present,
};
use crate::env::Env;
use crate::error::{
    RUN_DECIMAL_OVERFLOW, RUN_NO_VALUE, RUN_OVERFLOW, RUN_TYPE, RUN_UNBOUND_NAME, RuntimeError,
    decimal_overflow, divide_by_zero, overflow, raise_fault, temporal_overflow, type_error,
    unsupported,
};
use crate::path::direct_root_place;
use crate::read::{eval_local_field_get, local_field_value};
use crate::stdlib::int_remainder;
use crate::value::{Value, enum_id_from_ref, enum_value_from_member, render};

/// Evaluate an expression the checker typed as optional (`T?`) to its
/// `Option<Value>`: `None` is the empty optional, `Some` its present value. The one
/// place the runtime crosses from the value world — where the empty optional is
/// [`Value::Absent`] — into the `Option<Value>` that the return, resolution, and
/// present-or-clear boundaries consume. A present definite value widens into an
/// optional through the final arm.
pub(crate) fn eval_optional(
    expr: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match expr {
        ExecExpr::Absent { .. } => Ok(None),
        ExecExpr::Binary {
            op: BinaryOp::Coalesce,
            left,
            right,
            ..
        } => match eval_optional(left, env)? {
            Some(value) => Ok(Some(value)),
            None => eval_optional(right, env),
        },
        // A saved read resolves its own presence before any fixed-address read, so a
        // missing sparse field, keyed leaf, or unique-index key yields `None`
        // directly rather than a fault.
        _ if expr.saved_place().is_some() => read_saved_value_if_present(expr, expr.span(), env),
        ExecExpr::Call {
            args, target, span, ..
        } => eval_optional_call(expr, args, target, *span, env),
        ExecExpr::OptionalField {
            base, name, span, ..
        } => eval_optional_local_field(base, name, *span, env),
        _ => Ok(present_optional(eval_expr(expr, env)?)),
    }
}

/// Materialize an expression into a slot of declared type `slot`, the one boundary
/// that carries a `T?` read/call into a `const`/`var` binding or a function
/// argument. An optional slot admits the empty optional: an absent maybe-present
/// read, call, or `absent` literal flows in as [`Value::Absent`], the real optional
/// value. A non-optional slot reads present-or-fatal through [`eval_expr`], so a
/// required or narrowed-present read that finds absence stays a fatal
/// invalid-attached-data fault.
pub(crate) fn eval_into_slot(
    expr: &ExecExpr,
    slot: Option<&MarrowType>,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if matches!(slot, Some(MarrowType::Optional(_))) {
        Ok(eval_optional(expr, env)?.unwrap_or(Value::Absent))
    } else {
        eval_expr(expr, env)
    }
}

/// A [`Value`] carried at an optional boundary: the empty optional becomes `None`,
/// every present value becomes `Some`.
fn present_optional(value: Value) -> Option<Value> {
    match value {
        Value::Absent => None,
        value => Some(value),
    }
}

/// Evaluate a non-saved call at an optional boundary. A program function that
/// returns the empty optional completes as `Ok(None)`; a maybe-present builtin,
/// stdlib op, host op, or local-collection read yields [`Value::Absent`], which
/// collapses to `None`. A genuine fault — including a definite host op's
/// `run.absent_element`, such as a missing required env var — propagates so a
/// surrounding `catch` can bind it.
fn eval_optional_call(
    call: &ExecExpr,
    args: &[marrow_check::CheckedArg],
    target: &marrow_check::CheckedCallTarget,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    Ok(eval_call(call, args, target, span, env)?.and_then(present_optional))
}

/// Evaluate `base?.name` over a non-saved base. An absent base short-circuits to
/// the empty optional; a present record reads its member, which may itself be an
/// absent sparse field and so flows through the same optional collapse.
fn eval_optional_local_field(
    base: &ExecExpr,
    name: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match eval_optional(base, env)? {
        None => Ok(None),
        Some(base) => Ok(present_optional(local_field_value(base, name, span)?)),
    }
}

/// `place ?? default`: the left operand's present value, or `default` when the left
/// is the empty optional.
pub(crate) fn eval_coalesce(
    left: &ExecExpr,
    default: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match eval_optional(left, env)? {
        Some(value) => Ok(value),
        None => eval_expr(default, env),
    }
}

pub(crate) fn eval_expr(expr: &ExecExpr, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
    if let Some(place) = direct_root_place(expr) {
        return read_resource(place, &[], expr.span(), env);
    }
    match expr {
        ExecExpr::Literal { kind, text, span } => eval_literal(*kind, text, *span),
        ExecExpr::Absent { .. } => Ok(Value::Absent),
        ExecExpr::Name {
            segments,
            enum_member,
            span,
        } => eval_name(segments, enum_member.as_ref(), *span, env),
        ExecExpr::Unary { op, operand, span } => eval_unary(*op, operand, *span, env),
        ExecExpr::Binary {
            op,
            left,
            right,
            span,
        } => eval_binary(*op, left, right, *span, env),
        ExecExpr::Call {
            args, target, span, ..
        } => eval_call_expr(expr, args, target, *span, env),
        ExecExpr::Interpolation { parts, span } => eval_interpolation(parts, *span, env),
        ExecExpr::Field {
            base, name, span, ..
        } => eval_field(expr, base, name, *span, env),
        ExecExpr::OptionalField {
            base, name, span, ..
        } => eval_optional_field(expr, base, name, *span, env),
        _ => Err(unsupported("this expression", expr.span())),
    }
}

fn eval_name(
    segments: &[String],
    enum_member: Option<&CheckedEnumMemberRef>,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    if let Some(member) = enum_member {
        return enum_member_value(member, span, env);
    }
    if segments.len() != 1 {
        return Err(unsupported("a qualified name", span));
    }
    env.lookup(&segments[0]).cloned().ok_or_else(|| {
        RuntimeError::fault(
            RUN_UNBOUND_NAME,
            format!("`{}` is not bound", segments[0]),
            span,
        )
    })
}

fn enum_member_value(
    member: &CheckedEnumMemberRef,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    let enum_id = enum_id_from_ref(member.enum_ref);
    let member_fact = env
        .program
        .facts()
        .enum_member(member.member_id)
        .ok_or_else(|| unsupported("this enum member", span))?;
    if member_fact.enum_id != enum_id {
        return Err(unsupported("this enum member", span));
    }
    enum_value_from_member(env.program.facts(), member.member_id)
        .map(Value::Enum)
        .ok_or_else(|| unsupported("this enum member", span))
}

fn eval_call_expr(
    call: &ExecExpr,
    args: &[marrow_check::CheckedArg],
    target: &marrow_check::CheckedCallTarget,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match eval_call(call, args, target, span, env)? {
        Some(value) => Ok(value),
        None => Err(RuntimeError::fault(
            RUN_NO_VALUE,
            "a call to a function that returns no value cannot be used as a value".into(),
            span,
        )),
    }
}

fn eval_field(
    expr: &ExecExpr,
    base: &ExecExpr,
    name: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if base.saved_place().is_some() {
        eval_saved_field(expr, env)
    } else {
        eval_local_field_get(base, name, span, env)
    }
}

/// Evaluate an interpolated string `$"...{expr}..."`, rendering embedded
/// expressions to text and unescaping `{{`/`}}` in literal segments.
pub(crate) fn eval_interpolation(
    parts: &[InterpolationPart],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    enum PreparedInterpolationPart<'a> {
        Text(String),
        Expr(&'a ExecExpr),
    }

    let mut prepared = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            InterpolationPart::Text { text, .. } => {
                let text = decode_string_escapes(text)
                    .map_err(|error| string_literal_fault(error, span))?;
                // Literal text is validated before expression holes run, so malformed
                // escapes cannot be hidden behind side effects or expression faults.
                prepared.push(PreparedInterpolationPart::Text(
                    if text.contains(['{', '}']) {
                        text.replace("{{", "{").replace("}}", "}")
                    } else {
                        text
                    },
                ));
            }
            InterpolationPart::Expr(expr) => prepared.push(PreparedInterpolationPart::Expr(expr)),
        }
    }

    let mut result = String::new();
    for part in prepared {
        match part {
            PreparedInterpolationPart::Text(text) => result.push_str(&text),
            PreparedInterpolationPart::Expr(expr) => {
                result.push_str(&render(eval_expr(expr, env)?, span)?)
            }
        }
    }
    Ok(Value::Str(result))
}

pub(crate) fn eval_literal(
    kind: LiteralKind,
    text: &str,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match kind {
        LiteralKind::Integer => text.parse::<i64>().map(Value::Int).map_err(|_| {
            raise_fault(
                RUN_OVERFLOW,
                format!("integer literal `{text}` is out of range"),
                span,
            )
        }),
        LiteralKind::Duration => eval_duration_literal(text, span),
        LiteralKind::Bool => Ok(Value::Bool(text == "true")),
        LiteralKind::String => eval_string_literal(text, span),
        LiteralKind::Decimal => Decimal::parse(text).map(Value::Decimal).ok_or_else(|| {
            raise_fault(
                RUN_DECIMAL_OVERFLOW,
                format!("decimal literal `{text}` is out of range"),
                span,
            )
        }),
        LiteralKind::Bytes => eval_bytes_literal(text, span),
    }
}

/// Decode a duration literal `NUMBER.UNIT` to its nanosecond span. Checked
/// literals should already be parser-shaped; malformed checked text faults as a
/// type error, while an out-of-range magnitude faults as overflow.
fn eval_duration_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    let overflow = || {
        raise_fault(
            RUN_OVERFLOW,
            format!("duration literal `{text}` is out of range"),
            span,
        )
    };
    let Some((magnitude, unit)) = text.split_once('.') else {
        return Err(type_error("invalid duration literal", span));
    };
    if magnitude.is_empty() || !magnitude.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(type_error("invalid duration literal", span));
    }
    let magnitude: i128 = magnitude.parse().map_err(|_| overflow())?;
    let Some(seconds) = duration_unit_seconds(unit) else {
        return Err(type_error("invalid duration literal", span));
    };
    let nanos = magnitude
        .checked_mul(seconds as i128)
        .and_then(|total_seconds| total_seconds.checked_mul(1_000_000_000))
        .ok_or_else(overflow)?;
    Ok(Value::Duration(nanos))
}

/// Decode a string literal; `text` is the raw source, including surrounding quotes.
pub(crate) fn eval_string_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    decode_string_literal(text)
        .map(Value::Str)
        .map_err(|error| string_literal_fault(error, span))
}

/// A string literal that fails to decode at run time. The checker rejects an
/// unsupported escape before a run, so reaching this is a checker/runtime
/// disagreement; the fault names the real cause rather than claiming the runtime
/// does not evaluate escapes at all.
fn string_literal_fault(error: StringLiteralError, span: SourceSpan) -> RuntimeError {
    let cause = match error {
        StringLiteralError::Unquoted => "an unquoted string literal",
        StringLiteralError::BadEscape { .. } => {
            "an unsupported string escape; only `\\\\`, `\\\"`, `\\n`, `\\r`, and `\\t` are recognized"
        }
    };
    raise_fault(RUN_TYPE, format!("invalid string literal: {cause}"), span)
}

/// Decode a bytes literal `b"..."` through the `marrow_syntax` escape owner. The
/// checker rejects a malformed bytes escape before a run, so a decode failure
/// here is a checker/runtime disagreement, not the primary validation; it faults
/// defensively rather than passing bad bytes through.
pub(crate) fn eval_bytes_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    decode_bytes_literal(text)
        .map(Value::Bytes)
        .map_err(|error| bytes_literal_fault(error, span))
}

fn bytes_literal_fault(error: BytesLiteralError, span: SourceSpan) -> RuntimeError {
    let cause = match error {
        BytesLiteralError::Unquoted => "an unquoted bytes literal",
        BytesLiteralError::BadEscape { .. } => {
            "an unsupported bytes escape; only `\\\\`, `\\\"`, `\\n`, `\\r`, `\\t`, and `\\xNN` are recognized"
        }
    };
    raise_fault(RUN_TYPE, format!("invalid bytes literal: {cause}"), span)
}

pub(crate) fn eval_unary(
    op: UnaryOp,
    operand: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match (op, eval_expr(operand, env)?) {
        (UnaryOp::Neg, Value::Int(n)) => n
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| overflow(span)),
        (UnaryOp::Neg, Value::Decimal(d)) => Decimal::from_parts(-d.coefficient(), d.scale())
            .map(Value::Decimal)
            .ok_or_else(|| decimal_overflow(span)),
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, _) => Err(type_error("negation expects a number", span)),
        (UnaryOp::Not, _) => Err(type_error("`not` expects a boolean", span)),
    }
}

pub(crate) fn eval_binary(
    op: BinaryOp,
    left: &ExecExpr,
    right: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        // Logical operators short-circuit: the right side is evaluated only when
        // the left does not already decide the result.
        BinaryOp::And => Ok(Value::Bool(eval_bool(left, env)? && eval_bool(right, env)?)),
        BinaryOp::Or => Ok(Value::Bool(eval_bool(left, env)? || eval_bool(right, env)?)),
        BinaryOp::Add
        | BinaryOp::Subtract
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Remainder => {
            eval_arithmetic_with_left_value(op, eval_expr(left, env)?, right, span, env)
        }
        BinaryOp::Less => compare_values(left, right, env, span, |o| o == Ordering::Less),
        BinaryOp::LessEqual => compare_values(left, right, env, span, |o| o != Ordering::Greater),
        BinaryOp::Greater => compare_values(left, right, env, span, |o| o == Ordering::Greater),
        BinaryOp::GreaterEqual => compare_values(left, right, env, span, |o| o != Ordering::Less),
        BinaryOp::Equal => Ok(Value::Bool(values_equal(left, right, env, span)?)),
        BinaryOp::NotEqual => Ok(Value::Bool(!values_equal(left, right, env, span)?)),
        BinaryOp::Is => eval_is(left, right, span, env),
        BinaryOp::Coalesce => eval_coalesce(left, right, env),
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => {
            Err(unsupported("this operator", span))
        }
    }
}

pub(crate) fn eval_arithmetic_with_left_value(
    op: BinaryOp,
    left: Value,
    right: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let right = eval_expr(right, env)?;
    match op {
        BinaryOp::Add => add_value_pair(left, right, span),
        BinaryOp::Subtract => subtract_value_pair(left, right, span),
        BinaryOp::Multiply => {
            numeric_value_pair(left, right, span, i64::checked_mul, Decimal::checked_mul)
        }
        BinaryOp::Divide => decimal_div_values(left, right, span),
        BinaryOp::Remainder => int_remainder_values(left, right, span),
        _ => Err(unsupported("this compound assignment operator", span)),
    }
}

/// Whether the left enum value sits at or under the right member in its enum's
/// descendant hierarchy.
pub(crate) fn eval_is(
    left: &ExecExpr,
    right: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Value::Enum(value) = eval_expr(left, env)? else {
        return Err(type_error("operator `is` requires an enum value", span));
    };
    let ExecExpr::Name {
        enum_member: Some(member),
        ..
    } = right
    else {
        return Err(unsupported("the right operand of `is`", span));
    };
    let enum_id = enum_id_from_ref(member.enum_ref);
    let right_member_id = member.member_id;
    let member = env
        .program
        .facts()
        .enum_member(right_member_id)
        .ok_or_else(|| unsupported("the right operand of `is`", span))?;
    if member.enum_id != enum_id {
        return Err(unsupported("the right operand of `is`", span));
    }
    Ok(Value::Bool(env.program.facts().enum_member_is_descendant(
        value.member_id,
        right_member_id,
    )))
}

fn add_value_pair(left: Value, right: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => a
            .checked_add(b)
            .map(Value::Int)
            .ok_or_else(|| overflow(span)),
        (Value::Decimal(a), Value::Decimal(b)) => a
            .checked_add(b)
            .map(Value::Decimal)
            .ok_or_else(|| decimal_overflow(span)),
        (Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
        (Value::Instant(a), Value::Duration(b)) => instant_result(a.checked_add(b), span),
        (Value::Duration(a), Value::Duration(b)) => duration_result(a.checked_add(b), span),
        _ => Err(type_error("operator `+` expects compatible operands", span)),
    }
}

fn subtract_value_pair(left: Value, right: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => a
            .checked_sub(b)
            .map(Value::Int)
            .ok_or_else(|| overflow(span)),
        (Value::Decimal(a), Value::Decimal(b)) => a
            .checked_sub(b)
            .map(Value::Decimal)
            .ok_or_else(|| decimal_overflow(span)),
        (Value::Instant(a), Value::Instant(b)) => duration_result(a.checked_sub(b), span),
        (Value::Instant(a), Value::Duration(b)) => instant_result(a.checked_sub(b), span),
        (Value::Duration(a), Value::Duration(b)) => duration_result(a.checked_sub(b), span),
        _ => Err(type_error("operator `-` expects compatible operands", span)),
    }
}

fn duration_result(result: Option<i128>, span: SourceSpan) -> Result<Value, RuntimeError> {
    result
        .map(Value::Duration)
        .ok_or_else(|| temporal_overflow(span))
}

fn instant_result(result: Option<i128>, span: SourceSpan) -> Result<Value, RuntimeError> {
    let nanos = result.ok_or_else(|| temporal_overflow(span))?;
    if instant_in_saved_range(nanos) {
        Ok(Value::Instant(nanos))
    } else {
        Err(temporal_overflow(span))
    }
}

fn instant_in_saved_range(nanos: i128) -> bool {
    supported_instant_nanos(nanos)
}

/// The checker rejects mixed int/decimal operands, so a mismatch here is
/// defensive; overflow maps to a typed runtime fault.
fn numeric_value_pair(
    left: Value,
    right: Value,
    span: SourceSpan,
    int_op: fn(i64, i64) -> Option<i64>,
    decimal_op: fn(Decimal, Decimal) -> Option<Decimal>,
) -> Result<Value, RuntimeError> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => {
            int_op(a, b).map(Value::Int).ok_or_else(|| overflow(span))
        }
        (Value::Decimal(a), Value::Decimal(b)) => decimal_op(a, b)
            .map(Value::Decimal)
            .ok_or_else(|| decimal_overflow(span)),
        _ => Err(type_error(
            "arithmetic expects two operands of the same numeric type",
            span,
        )),
    }
}

/// Divide two numeric operands as decimals: a zero divisor faults as
/// `run.divide_by_zero`, a result outside the decimal envelope as
/// `run.decimal_overflow`.
fn decimal_div_values(left: Value, right: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let dividend = to_decimal(left, span)?;
    let divisor = to_decimal(right, span)?;
    if divisor.is_zero() {
        return Err(divide_by_zero("division by zero", span));
    }
    dividend
        .checked_div(divisor)
        .map(Value::Decimal)
        .ok_or_else(|| decimal_overflow(span))
}

/// Coerce a value to a decimal: an integer becomes exact, decimals are
/// preserved, other types fault.
pub(crate) fn to_decimal(value: Value, span: SourceSpan) -> Result<Decimal, RuntimeError> {
    match value {
        Value::Decimal(decimal) => Ok(decimal),
        Value::Int(n) => Decimal::from_parts(i128::from(n), 0)
            .ok_or_else(|| type_error("an integer that is not a valid decimal", span)),
        _ => Err(type_error("division expects numeric operands", span)),
    }
}

/// The only integer division-family operator (division yields decimals via
/// `decimal_div`). Shares the integer-remainder path and zero-divisor message
/// with `std::math::remainder`.
fn int_remainder_values(
    left: Value,
    right: Value,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    let Value::Int(a) = left else {
        return Err(type_error("expected an integer", span));
    };
    let Value::Int(b) = right else {
        return Err(type_error("expected an integer", span));
    };
    int_remainder(a, b, span).map(Value::Int)
}

/// Operands must be the same orderable type (integers or strings). Booleans and
/// mismatched types are not orderable; the checker rejects those statically, so
/// a mismatch here is defensive.
pub(crate) fn compare_values(
    left: &ExecExpr,
    right: &ExecExpr,
    env: &mut Env<'_>,
    span: SourceSpan,
    want: fn(Ordering) -> bool,
) -> Result<Value, RuntimeError> {
    let ordering = match (eval_expr(left, env)?, eval_expr(right, env)?) {
        (Value::Int(a), Value::Int(b)) => a.cmp(&b),
        (Value::Str(a), Value::Str(b)) => a.cmp(&b),
        (Value::Decimal(a), Value::Decimal(b)) => a.cmp(&b),
        (Value::Bytes(a), Value::Bytes(b)) => a.cmp(&b),
        // Temporal values order by their underlying instant/day/nanosecond count.
        (Value::Instant(a), Value::Instant(b)) => a.cmp(&b),
        (Value::Date(a), Value::Date(b)) => a.cmp(&b),
        (Value::Duration(a), Value::Duration(b)) => a.cmp(&b),
        _ => {
            return Err(type_error(
                "cannot order values of different or unordered types",
                span,
            ));
        }
    };
    Ok(Value::Bool(want(ordering)))
}

/// Operands must share a type; the checker rejects cross-type comparison
/// statically, so the mismatch arm is defensive.
pub(crate) fn values_equal(
    left: &ExecExpr,
    right: &ExecExpr,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<bool, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
        (Value::Int(a), Value::Int(b)) => Ok(a == b),
        (Value::Bool(a), Value::Bool(b)) => Ok(a == b),
        (Value::Str(a), Value::Str(b)) => Ok(a == b),
        (Value::Decimal(a), Value::Decimal(b)) => Ok(a == b),
        (Value::Bytes(a), Value::Bytes(b)) => Ok(a == b),
        (Value::Instant(a), Value::Instant(b)) => Ok(a == b),
        (Value::Date(a), Value::Date(b)) => Ok(a == b),
        (Value::Duration(a), Value::Duration(b)) => Ok(a == b),
        (Value::Enum(a), Value::Enum(b)) => {
            Ok(a.enum_id == b.enum_id && a.member_id == b.member_id)
        }
        // The checker's nominal rule requires both identities to name the same
        // resource, so comparing key segments is the whole verdict.
        (Value::Identity(a), Value::Identity(b)) => Ok(a == b),
        _ => Err(type_error("cannot compare values of different types", span)),
    }
}

pub(crate) fn eval_int(expr: &ExecExpr, env: &mut Env<'_>) -> Result<i64, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Int(n) => Ok(n),
        _ => Err(type_error("expected an integer", expr.span())),
    }
}

/// A `None` condition means it did not parse, which the checker rejects before
/// the runtime; guard anyway so it faults rather than panics.
pub(crate) fn eval_condition(
    condition: Option<&ExecExpr>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    match condition {
        Some(condition) => eval_bool(condition, env),
        None => Err(unsupported("a condition that did not parse", span)),
    }
}

pub(crate) fn eval_bool(expr: &ExecExpr, env: &mut Env<'_>) -> Result<bool, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Bool(b) => Ok(b),
        _ => Err(type_error("expected a boolean", expr.span())),
    }
}

#[cfg(test)]
mod tests {
    use marrow_check::CheckedLiteralKind as LiteralKind;
    use marrow_syntax::SourceSpan;

    use crate::error::{RUN_OVERFLOW, RUN_TYPE};
    use crate::expr::{eval_duration_literal, eval_literal};
    use crate::value::Value;

    /// The checker folds a leading `-` over an integer literal into the literal
    /// text, so the runtime sees the signed spelling. `-9223372036854775808` is
    /// `i64::MIN`, in range even though its bare magnitude `i64::MAX + 1` is not.
    /// A bare out-of-range magnitude still faults, matching the checker, which
    /// rejects it before a run reaches here.
    #[test]
    fn integer_literal_boundaries_match_the_checker() {
        let span = SourceSpan::default();
        assert_eq!(
            eval_literal(LiteralKind::Integer, "-9223372036854775808", span).unwrap(),
            Value::Int(i64::MIN)
        );
        assert_eq!(
            eval_literal(LiteralKind::Integer, "9223372036854775807", span).unwrap(),
            Value::Int(i64::MAX)
        );
        assert_eq!(
            eval_literal(LiteralKind::Integer, "9223372036854775808", span)
                .unwrap_err()
                .code(),
            RUN_OVERFLOW
        );
        assert_eq!(
            eval_literal(LiteralKind::Integer, "-9223372036854775809", span)
                .unwrap_err()
                .code(),
            RUN_OVERFLOW
        );
    }

    #[test]
    fn malformed_checked_duration_literals_fault_without_panicking() {
        let span = SourceSpan::default();
        assert_eq!(
            eval_duration_literal("1", span).unwrap_err().code(),
            RUN_TYPE
        );
        assert_eq!(
            eval_duration_literal(".seconds", span).unwrap_err().code(),
            RUN_TYPE
        );
        assert_eq!(
            eval_duration_literal("many.seconds", span)
                .unwrap_err()
                .code(),
            RUN_TYPE
        );
        assert_eq!(
            eval_duration_literal("1.year", span).unwrap_err().code(),
            RUN_TYPE
        );
    }

    #[test]
    fn oversized_checked_duration_literals_keep_overflow_fault() {
        let span = SourceSpan::default();
        let literal = format!("{}.seconds", i128::MAX);
        assert_eq!(
            eval_duration_literal(&literal, span).unwrap_err().code(),
            RUN_OVERFLOW
        );
    }
}
