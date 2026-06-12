//! Pure-expression evaluation: literals, operators, and coercions.

use std::cmp::Ordering;

use marrow_check::{
    CheckedBinaryOp as BinaryOp, CheckedEnumMemberRef, CheckedExpr as ExecExpr,
    CheckedInterpolationPart as InterpolationPart, CheckedLiteralKind as LiteralKind,
    CheckedUnaryOp as UnaryOp,
};
use marrow_store::Decimal;
use marrow_store::value::{NANOS_PER_DAY, date_days};
use marrow_syntax::{
    SourceSpan, StringLiteralError, decode_string_escapes, decode_string_literal,
    duration_unit_seconds,
};

use crate::call::{call_target_maybe_present, eval_call, expression_absent_at_resolution_site};
use crate::durable_read::{
    eval_optional_field, eval_saved_field, read_resource, read_saved_value_if_present,
};
use crate::env::Env;
use crate::error::{
    RUN_ABSENT, RUN_DECIMAL_OVERFLOW, RUN_NO_VALUE, RUN_OVERFLOW, RUN_TYPE, RUN_UNBOUND_NAME,
    RuntimeError, decimal_overflow, divide_by_zero, overflow, raise_fault, temporal_overflow,
    type_error, unsupported,
};
use crate::path::direct_root_place;
use crate::read::eval_local_field_get;
use crate::stdlib::int_remainder;
use crate::value::{Value, enum_id_from_ref, enum_value_from_member, render};

pub(crate) fn eval_coalesce(
    path: &ExecExpr,
    default: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if path.saved_place().is_some() {
        return match read_saved_value_if_present(path, path.span(), env)? {
            Some(value) => Ok(value),
            None => eval_expr(default, env),
        };
    }
    match eval_expr(path, env) {
        // Non-saved absence faults, such as host environment lookups, keep the
        // catchable error path. Saved-data absence is handled above by probing
        // the fixed read site before any fatal read occurs.
        Err(error) if expression_absent_at_resolution_site(path, &error) => eval_expr(default, env),
        other => other,
    }
}

pub(crate) fn eval_expr(expr: &ExecExpr, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
    if let Some(place) = direct_root_place(expr) {
        return read_resource(place, &[], expr.span(), env);
    }
    match expr {
        ExecExpr::Literal { kind, text, span } => eval_literal(*kind, text, *span),
        ExecExpr::Name {
            segments,
            enum_member,
            span,
        } => eval_name(segments, *enum_member, *span, env),
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
    enum_member: Option<marrow_check::CheckedEnumMemberRef>,
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
    member: CheckedEnumMemberRef,
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
        None if call_target_maybe_present(target) => Err(raise_fault(
            RUN_ABSENT,
            "maybe-present call returned absent".into(),
            span,
        )),
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
    let mut decoded_text = Vec::new();
    for part in parts {
        if let InterpolationPart::Text { text, .. } = part {
            let text =
                decode_string_escapes(text).map_err(|error| string_literal_fault(error, span))?;
            // Literal text is validated before expression holes run, so malformed
            // escapes cannot be hidden behind side effects or expression faults.
            decoded_text.push(if text.contains(['{', '}']) {
                text.replace("{{", "{").replace("}}", "}")
            } else {
                text
            });
        }
    }

    let mut decoded_text = decoded_text.into_iter();
    let mut result = String::new();
    for part in parts {
        match part {
            InterpolationPart::Text { .. } => {
                result.push_str(
                    &decoded_text
                        .next()
                        .expect("one decoded text segment per interpolation text part"),
                );
            }
            InterpolationPart::Expr(expr) => result.push_str(&render(eval_expr(expr, env)?, span)?),
        }
    }
    debug_assert!(decoded_text.next().is_none());
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

/// Decode a duration literal `NUMBER.UNIT` to its nanosecond span. The lexer
/// guarantees the shape (digits, a dot, a known unit), so the runtime checks
/// only an out-of-range magnitude.
fn eval_duration_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    let overflow = || {
        raise_fault(
            RUN_OVERFLOW,
            format!("duration literal `{text}` is out of range"),
            span,
        )
    };
    let (magnitude, unit) = text.split_once('.').expect("a duration literal has a dot");
    let magnitude: i128 = magnitude.parse().map_err(|_| overflow())?;
    let seconds = duration_unit_seconds(unit).expect("a duration literal names a known unit");
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
        StringLiteralError::BadEscape => {
            "an unsupported string escape; only `\\\\`, `\\\"`, `\\n`, `\\r`, and `\\t` are recognized"
        }
    };
    raise_fault(RUN_TYPE, format!("invalid string literal: {cause}"), span)
}

/// Decode a bytes literal `b"..."`: ordinary text contributes its UTF-8 bytes,
/// while bytes escapes can emit arbitrary byte values.
pub(crate) fn eval_bytes_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    let inner = text
        .strip_prefix("b\"")
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| unsupported("this bytes literal", span))?;
    Ok(Value::Bytes(decode_bytes_escapes(inner, span)?))
}

fn decode_bytes_escapes(text: &str, span: SourceSpan) -> Result<Vec<u8>, RuntimeError> {
    let mut result = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buffer = [0; 4];
            result.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(unsupported("bytes escape sequences", span));
        };
        match escaped {
            '\\' => result.push(b'\\'),
            '"' => result.push(b'"'),
            'n' => result.push(b'\n'),
            'r' => result.push(b'\r'),
            't' => result.push(b'\t'),
            'x' => {
                let Some(high) = chars.next().and_then(hex_digit) else {
                    return Err(unsupported("bytes escape sequences", span));
                };
                let Some(low) = chars.next().and_then(hex_digit) else {
                    return Err(unsupported("bytes escape sequences", span));
                };
                result.push((high << 4) | low);
            }
            _ => return Err(unsupported("bytes escape sequences", span)),
        }
    }
    Ok(result)
}

fn hex_digit(ch: char) -> Option<u8> {
    ch.to_digit(16).and_then(|digit| u8::try_from(digit).ok())
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
        BinaryOp::Add => add_values(left, right, env, span),
        BinaryOp::Subtract => subtract_values(left, right, env, span),
        BinaryOp::Multiply => numeric_op(
            left,
            right,
            env,
            span,
            i64::checked_mul,
            Decimal::checked_mul,
        ),
        // `/` always yields a decimal, so integer operands divide as decimals
        // too: `1 / 2` is `0.5`.
        BinaryOp::Divide => decimal_div(left, right, env, span),
        BinaryOp::Remainder => int_remainder_op(left, right, env, span),
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

pub(crate) fn add_values(
    left: &ExecExpr,
    right: &ExecExpr,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
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

pub(crate) fn subtract_values(
    left: &ExecExpr,
    right: &ExecExpr,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
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
    let min_day = date_days(1, 1, 1).expect("year 0001-01-01 is in the saved instant range");
    let max_day = date_days(9999, 12, 31).expect("year 9999-12-31 is in the saved instant range");
    let min = i128::from(min_day) * NANOS_PER_DAY;
    let max = i128::from(max_day) * NANOS_PER_DAY + (NANOS_PER_DAY - 1);
    (min..=max).contains(&nanos)
}

/// The checker rejects mixed int/decimal operands, so a mismatch here is
/// defensive; overflow maps to a typed runtime fault.
pub(crate) fn numeric_op(
    left: &ExecExpr,
    right: &ExecExpr,
    env: &mut Env<'_>,
    span: SourceSpan,
    int_op: fn(i64, i64) -> Option<i64>,
    decimal_op: fn(Decimal, Decimal) -> Option<Decimal>,
) -> Result<Value, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
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
pub(crate) fn decimal_div(
    left: &ExecExpr,
    right: &ExecExpr,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    let dividend = to_decimal(eval_expr(left, env)?, span)?;
    let divisor = to_decimal(eval_expr(right, env)?, span)?;
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
pub(crate) fn int_remainder_op(
    left: &ExecExpr,
    right: &ExecExpr,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    let a = eval_int(left, env)?;
    let b = eval_int(right, env)?;
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
