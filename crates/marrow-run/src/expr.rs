//! Pure-expression evaluation: literals, operators, and coercions.

use crate::*;

/// Evaluate `path ?? default`: the value at the left path read, or `default` when
/// it is absent. Schema/type errors are not hidden — only an absent element
/// (`run.absent_element`) falls back to the default.
pub(crate) fn eval_coalesce(
    path: &Expression,
    default: &Expression,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match eval_expr(path, env) {
        // `??` absorbs an absent read as ordinary control flow, falling back to
        // the default. The absent fault's catchable `throw` value rides the `Err`
        // and is simply discarded here, so it never unwinds as a throw. A `?.`
        // chain on the left short-circuits to this same absent fault.
        Err(error) if error.code == RUN_ABSENT => eval_expr(default, env),
        other => other,
    }
}

/// Resolve a member-path literal (`Enum::member`, `Cat::tiger::bengal`, or a
/// qualified `mod::Enum::member`) to its enum schema and the member's pre-order
/// ordinal, or `None` when the path is not a member reference (too few segments, or
/// no such enum). The enum is the longest visible-enum prefix and the rest is the
/// member path, walked down the schema's member tree by the same shared walk the
/// checker uses, so a value and its check cannot disagree. A path that does not
/// walk to a single member only arises in an unchecked program and is a fault.
/// Shared by the `Name` value eval and the `is` operator.
fn resolve_enum_member<'p>(
    segments: &[String],
    span: SourceSpan,
    env: &Env<'p>,
) -> Result<Option<(&'p EnumSchema, usize)>, RuntimeError> {
    if segments.len() < 2 {
        return Ok(None);
    }
    // A bare `Enum::path` takes `segments[0]` as a same-module enum; the rest is the
    // member path. Otherwise the enum is a qualified `module::Enum`, found by the
    // longest module prefix that names a known enum (leaving ≥1 member segment).
    let (schema, path) = if let Some(schema) = resolve_enum(env.program, env.module, &segments[0]) {
        (schema, &segments[1..])
    } else {
        let mut found = None;
        for enum_index in (1..segments.len() - 1).rev() {
            let module =
                marrow_check::expand_module_alias(&segments[..enum_index].join("::"), &env.aliases);
            if let Some(schema) = enum_in(env.program, &module, &segments[enum_index]) {
                found = Some((schema, &segments[enum_index + 1..]));
                break;
            }
        }
        let Some(found) = found else {
            return Ok(None);
        };
        found
    };
    let walk_path: Vec<&str> = path.iter().map(String::as_str).collect();
    match schema.walk_member_path(&walk_path) {
        MemberPathResolution::Found(ordinal) => Ok(Some((schema, ordinal))),
        // The checker proved a value/`is` operand walks to exactly one member, so
        // a miss here is a fault, not a reachable path.
        _ => Err(unsupported("a qualified name", span)),
    }
}

pub(crate) fn eval_expr(expr: &Expression, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
    match expr {
        Expression::Literal { kind, text, span } => eval_literal(*kind, text, *span),
        Expression::Name { segments, span } => {
            // An `Enum::member` (bare) or `mod::Enum::member` (qualified) evaluates
            // to the member's declaration-order ordinal as an int — the enum's
            // stored form. The checker has already validated the reference, so an
            // unknown enum/member here only arises in an unchecked program and is a
            // fatal fault.
            // The last segment is the member, the one before it the enum name; any
            // remaining prefix is the owning module, joined by `::` so a nested
            // module stays whole (`a::b::Status::active` → module `a::b`, enum
            // `Status`, member `active`). A bare `Enum::member` (no prefix) resolves
            // relative to the running module, mirroring the checker.
            if let Some((_, ordinal)) = resolve_enum_member(segments, *span, env)? {
                return Ok(Value::Int(ordinal as i64));
            }
            if segments.len() != 1 {
                return Err(unsupported("a qualified name", *span));
            }
            env.lookup(&segments[0])
                .cloned()
                .ok_or_else(|| RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_UNBOUND_NAME,
                    message: format!("`{}` is not bound", segments[0]),
                    span: *span,
                })
        }
        Expression::Unary { op, operand, span } => eval_unary(*op, operand, *span, env),
        Expression::Binary {
            op,
            left,
            right,
            span,
        } => eval_binary(*op, left, right, *span, env),
        Expression::Call { callee, args, span } => match eval_call(callee, args, *span, env)? {
            Some(value) => Ok(value),
            None => Err(RuntimeError {
                throw: None,
                origin: None,
                code: RUN_NO_VALUE,
                message: "a call to a function that returns no value cannot be used as a value"
                    .into(),
                span: *span,
            }),
        },
        Expression::Interpolation { parts, span } => eval_interpolation(parts, *span, env),
        // A dotted field read: off a saved root (`^books(id).title`) it is a
        // saved read; off a local it reads the resource value's field.
        Expression::Field {
            base, name, span, ..
        } => {
            if is_saved_path(base) {
                eval_saved_field(expr, env)
            } else {
                eval_local_field_get(base, name, *span, env)
            }
        }
        // An optional field read `base?.name`: the same read as `Field`, but an
        // absent base or field short-circuits the chain to absent.
        Expression::OptionalField {
            base, name, span, ..
        } => eval_optional_field(expr, base, name, *span, env),
        // A bare saved root read (`^settings`) is a whole-resource read of a
        // keyless singleton; a keyed root needs a `^root(key…)` call.
        Expression::SavedRoot { name, span, .. } => read_resource(name, &[], *span, env),
    }
}

/// Evaluate an interpolated string `$"...{expr}..."` to a string value: literal
/// segments contribute their text (with `{{`/`}}` unescaped to single braces),
/// and embedded expressions are rendered to text.
pub(crate) fn eval_interpolation(
    parts: &[InterpolationPart],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut decoded_text = Vec::new();
    for part in parts {
        if let InterpolationPart::Text { text, .. } = part {
            let text = decode_string_escapes(text, span)?;
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
        LiteralKind::Integer => text
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| RuntimeError {
                throw: None,
                origin: None,
                code: RUN_OVERFLOW,
                message: format!("integer literal `{text}` is out of range"),
                span,
            }),
        LiteralKind::Duration => eval_duration_literal(text, span),
        LiteralKind::Bool => Ok(Value::Bool(text == "true")),
        LiteralKind::String => eval_string_literal(text, span),
        LiteralKind::Decimal => {
            Decimal::parse(text)
                .map(Value::Decimal)
                .ok_or_else(|| RuntimeError {
                    throw: None,
                    origin: None,
                    code: RUN_OVERFLOW,
                    message: format!("decimal literal `{text}` is out of range"),
                    span,
                })
        }
        LiteralKind::Bytes => eval_bytes_literal(text, span),
    }
}

/// Decode a duration literal `NUMBER.UNIT` to its fixed nanosecond span, the
/// same value `duration("PT<seconds>S")` produces. The lexer guarantees the
/// shape — digits, a dot, and a known unit — so only an out-of-range magnitude
/// faults, sharing the int/decimal overflow path.
fn eval_duration_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    let overflow = || RuntimeError {
        throw: None,
        origin: None,
        code: RUN_OVERFLOW,
        message: format!("duration literal `{text}` is out of range"),
        span,
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

/// Decode a string literal's value. The literal `text` is the raw source,
/// including the surrounding quotes.
pub(crate) fn eval_string_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| unsupported("this string literal", span))?;
    Ok(Value::Str(decode_string_escapes(inner, span)?))
}

fn decode_string_escapes(text: &str, span: SourceSpan) -> Result<String, RuntimeError> {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            result.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(unsupported("string escape sequences", span));
        };
        result.push(match escaped {
            '\\' => '\\',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            _ => return Err(unsupported("string escape sequences", span)),
        });
    }
    Ok(result)
}

/// Decode a bytes literal `b"..."` to its raw bytes. Ordinary text contributes
/// its UTF-8 bytes, while bytes escapes can emit arbitrary byte values.
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
    operand: &Expression,
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
            .ok_or_else(|| overflow(span)),
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, _) => Err(type_error("negation expects a number", span)),
        (UnaryOp::Not, _) => Err(type_error("`not` expects a boolean", span)),
    }
}

pub(crate) fn eval_binary(
    op: BinaryOp,
    left: &Expression,
    right: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        // Logical operators short-circuit: the right side is evaluated only when
        // the left does not already decide the result.
        BinaryOp::And => Ok(Value::Bool(eval_bool(left, env)? && eval_bool(right, env)?)),
        BinaryOp::Or => Ok(Value::Bool(eval_bool(left, env)? || eval_bool(right, env)?)),
        BinaryOp::Add => numeric_op(
            left,
            right,
            env,
            span,
            i64::checked_add,
            Decimal::checked_add,
        ),
        BinaryOp::Subtract => numeric_op(
            left,
            right,
            env,
            span,
            i64::checked_sub,
            Decimal::checked_sub,
        ),
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
        BinaryOp::Concat => concat(left, right, env, span),
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => {
            Err(unsupported("this operator", span))
        }
    }
}

/// Evaluate `left is right`: whether the left enum value sits at or under the
/// right member in its enum's hierarchy. The left evaluates to its stored ordinal;
/// the right is a member-path literal resolved to its ordinal, and the test is the
/// schema's inclusive `is_descendant` — exact for a concrete-leaf right, a subtree
/// test for a category. The checker proved both sides name the same enum, so an
/// unresolved right is a fault, not a reachable program path.
pub(crate) fn eval_is(
    left: &Expression,
    right: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Value::Int(ordinal) = eval_expr(left, env)? else {
        return Err(type_error("operator `is` requires an enum value", span));
    };
    let Expression::Name { segments, .. } = right else {
        return Err(unsupported("the right operand of `is`", span));
    };
    let (schema, ancestor) = resolve_enum_member(segments, span, env)?
        .ok_or_else(|| unsupported("the right operand of `is`", span))?;
    Ok(Value::Bool(
        schema.is_descendant(ordinal as usize, ancestor),
    ))
}

/// Apply a checked numeric operation to two operands of the same numeric type —
/// both integers or both decimals — mapping overflow to `run.overflow`. The
/// checker rejects mixed int/decimal operands, so a mismatch here is a type error.
pub(crate) fn numeric_op(
    left: &Expression,
    right: &Expression,
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
            .ok_or_else(|| overflow(span)),
        _ => Err(type_error(
            "arithmetic expects two operands of the same numeric type",
            span,
        )),
    }
}

/// Divide two numeric operands as decimals (`/` always yields a decimal). A zero
/// divisor is `run.divide_by_zero`; a result outside the decimal envelope is
/// `run.overflow`.
pub(crate) fn decimal_div(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    let dividend = to_decimal(eval_expr(left, env)?, span)?;
    let divisor = to_decimal(eval_expr(right, env)?, span)?;
    if divisor.is_zero() {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_DIVIDE_BY_ZERO,
            message: "division by zero".into(),
            span,
        });
    }
    dividend
        .checked_div(divisor)
        .map(Value::Decimal)
        .ok_or_else(|| overflow(span))
}

/// Coerce a numeric value to a decimal: an integer becomes an exact decimal, a
/// decimal is itself. Any other type is a runtime type error.
pub(crate) fn to_decimal(value: Value, span: SourceSpan) -> Result<Decimal, RuntimeError> {
    match value {
        Value::Decimal(decimal) => Ok(decimal),
        Value::Int(n) => Decimal::from_parts(i128::from(n), 0)
            .ok_or_else(|| type_error("an integer that is not a valid decimal", span)),
        _ => Err(type_error("division expects numeric operands", span)),
    }
}

/// Evaluate the integer remainder operator (`%`) over two operands. The `/`
/// operator yields a decimal and uses `decimal_div`, so `%` is the only integer
/// division-family operator; it shares the one integer-remainder path (and its
/// "integer remainder by zero" message) with `std::math::remainder`.
pub(crate) fn int_remainder_op(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    let a = eval_int(left, env)?;
    let b = eval_int(right, env)?;
    int_remainder(a, b, span).map(Value::Int)
}

/// Compare two values of the same orderable type — integers or strings — and
/// test the resulting ordering. Booleans and mismatched types are not orderable.
pub(crate) fn compare_values(
    left: &Expression,
    right: &Expression,
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

/// Concatenate two strings with `++`.
pub(crate) fn concat(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
        (Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
        _ => Err(type_error("`++` concatenates two strings", span)),
    }
}

/// Whether two values are equal. They must share a scalar type; comparing across
/// types is a runtime type error (the checker rejects it statically).
pub(crate) fn values_equal(
    left: &Expression,
    right: &Expression,
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
        // Two identities compare by their key segments. The checker's nominal rule
        // already requires both to name the same resource, so a value comparison of
        // the keys is the whole verdict; identities of different resources never
        // reach here in a well-typed program.
        (Value::Identity(a), Value::Identity(b)) => Ok(a == b),
        _ => Err(type_error("cannot compare values of different types", span)),
    }
}

pub(crate) fn eval_int(expr: &Expression, env: &mut Env<'_>) -> Result<i64, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Int(n) => Ok(n),
        _ => Err(type_error("expected an integer", expr.span())),
    }
}

/// Evaluate an `if`/`while`/`else if` condition. The condition is `None` only when
/// it did not parse, which the checker rejects before a program reaches the runtime;
/// guard against it anyway so a malformed condition faults rather than panics.
pub(crate) fn eval_condition(
    condition: Option<&Expression>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    match condition {
        Some(condition) => eval_bool(condition, env),
        None => Err(unsupported("a condition that did not parse", span)),
    }
}

pub(crate) fn eval_bool(expr: &Expression, env: &mut Env<'_>) -> Result<bool, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Bool(b) => Ok(b),
        _ => Err(type_error("expected a boolean", expr.span())),
    }
}
