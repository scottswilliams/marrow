//! Pure `std::` helpers with no host capability.

use marrow_check::CheckedArg as ExecArg;
use marrow_store::value::{SavedValue, ScalarType, decode_value, encode_value};
use marrow_syntax::SourceSpan;

use crate::base64;
use crate::env::Env;
use crate::error::{Located, RuntimeError, overflow, std_arity, type_error, unsupported};
use crate::expr::eval_int;
use crate::stdlib::{
    eval_bytes_arg, eval_date_arg, eval_decimal_arg, eval_duration_arg, eval_instant_arg,
    eval_text, int_modulo, int_remainder,
};
use crate::value::Value;

pub(crate) fn eval_std(
    module: &str,
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match module {
        "text" => eval_text_std(op, args, span, env),
        "math" => eval_math_std(op, args, span, env),
        "bytes" => eval_bytes_std(op, args, span, env),
        "clock" => eval_clock_std(op, args, span, env),
        _ => Err(unsupported(&format!("std::{module}::{op}"), span)),
    }
}

fn eval_text_std(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "length" => {
            let [text] = args else {
                return Err(std_arity("text", op, span));
            };
            Ok(Value::Int(
                eval_text(text, env, span)?.chars().count() as i64
            ))
        }
        "trim" => {
            let [text] = args else {
                return Err(std_arity("text", op, span));
            };
            Ok(Value::Str(eval_text(text, env, span)?.trim().to_string()))
        }
        "contains" => {
            let [text, needle] = args else {
                return Err(std_arity("text", op, span));
            };
            let text = eval_text(text, env, span)?;
            let needle = eval_text(needle, env, span)?;
            Ok(Value::Bool(text.contains(&needle)))
        }
        "split" => {
            let [text, separator] = args else {
                return Err(std_arity("text", op, span));
            };
            let text = eval_text(text, env, span)?;
            let separator = eval_text(separator, env, span)?;
            Ok(Value::Sequence(
                text.split(separator.as_str())
                    .map(|part| Value::Str(part.to_string()))
                    .collect(),
            ))
        }
        other => Err(unsupported(&format!("std::text::{other}"), span)),
    }
}

fn eval_math_std(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "absInt" => {
            let [value] = args else {
                return Err(std_arity("math", op, span));
            };
            Ok(Value::Int(
                eval_int(&value.value, env)?
                    .checked_abs()
                    .ok_or_else(|| overflow(span))?,
            ))
        }
        "remainder" => {
            let [a, b] = args else {
                return Err(std_arity("math", op, span));
            };
            let remainder =
                int_remainder(eval_int(&a.value, env)?, eval_int(&b.value, env)?, span)?;
            Ok(Value::Int(remainder))
        }
        "modulo" => {
            let [a, b] = args else {
                return Err(std_arity("math", op, span));
            };
            let modulo = int_modulo(eval_int(&a.value, env)?, eval_int(&b.value, env)?, span)?;
            Ok(Value::Int(modulo))
        }
        "absDecimal" => {
            let [value] = args else {
                return Err(std_arity("math", op, span));
            };
            Ok(Value::Decimal(eval_decimal_arg(value, env, span)?.abs()))
        }
        "floor" => {
            let [value] = args else {
                return Err(std_arity("math", op, span));
            };
            let floored = eval_decimal_arg(value, env, span)?.floor();
            i64::try_from(floored)
                .map(Value::Int)
                .map_err(|_| overflow(span))
        }
        other => Err(unsupported(&format!("std::math::{other}"), span)),
    }
}

fn eval_bytes_std(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "length" => {
            let [value] = args else {
                return Err(std_arity("bytes", op, span));
            };
            Ok(Value::Int(eval_bytes_arg(value, env, span)?.len() as i64))
        }
        "base64Encode" => {
            let [value] = args else {
                return Err(std_arity("bytes", op, span));
            };
            Ok(Value::Str(base64::encode(&eval_bytes_arg(
                value, env, span,
            )?)))
        }
        "base64Decode" => {
            let [value] = args else {
                return Err(std_arity("bytes", op, span));
            };
            let text = eval_text(value, env, span)?;
            base64::decode(&text)
                .map(Value::Bytes)
                .ok_or_else(|| type_error("base64Decode: invalid base64 text", span))
        }
        other => Err(unsupported(&format!("std::bytes::{other}"), span)),
    }
}

fn eval_clock_std(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "formatInstant" => format_clock_instant(args, op, span, env),
        "parseInstant" => parse_clock_instant(args, op, span, env),
        "formatDate" => format_clock_date(args, op, span, env),
        "parseDate" => parse_clock_date(args, op, span, env),
        "formatDuration" => format_clock_duration(args, op, span, env),
        "parseDuration" => parse_clock_duration(args, op, span, env),
        "add" => add_clock_duration(args, op, span, env),
        other => Err(unsupported(&format!("std::clock::{other}"), span)),
    }
}

fn clock_unary_arg<'a>(
    args: &'a [ExecArg],
    op: &str,
    span: SourceSpan,
) -> Result<&'a ExecArg, RuntimeError> {
    let [value] = args else {
        return Err(std_arity("clock", op, span));
    };
    Ok(value)
}

fn format_clock_instant(
    args: &[ExecArg],
    op: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    format_scalar(
        SavedValue::Instant(eval_instant_arg(
            clock_unary_arg(args, op, span)?,
            env,
            span,
        )?),
        span,
    )
}

fn parse_clock_instant(
    args: &[ExecArg],
    op: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let text = eval_text(clock_unary_arg(args, op, span)?, env, span)?;
    match decode_value(text.as_bytes(), ScalarType::Instant) {
        Some(SavedValue::Instant(nanos)) => Ok(Value::Instant(nanos)),
        _ => Err(type_error("parseInstant: invalid instant text", span)),
    }
}

fn format_clock_date(
    args: &[ExecArg],
    op: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    format_scalar(
        SavedValue::Date(eval_date_arg(clock_unary_arg(args, op, span)?, env, span)?),
        span,
    )
}

fn parse_clock_date(
    args: &[ExecArg],
    op: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let text = eval_text(clock_unary_arg(args, op, span)?, env, span)?;
    match decode_value(text.as_bytes(), ScalarType::Date) {
        Some(SavedValue::Date(days)) => Ok(Value::Date(days)),
        _ => Err(type_error("parseDate: invalid date text", span)),
    }
}

fn format_clock_duration(
    args: &[ExecArg],
    op: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    format_scalar(
        SavedValue::Duration(eval_duration_arg(
            clock_unary_arg(args, op, span)?,
            env,
            span,
        )?),
        span,
    )
}

fn parse_clock_duration(
    args: &[ExecArg],
    op: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let text = eval_text(clock_unary_arg(args, op, span)?, env, span)?;
    match decode_value(text.as_bytes(), ScalarType::Duration) {
        Some(SavedValue::Duration(nanos)) => Ok(Value::Duration(nanos)),
        _ => Err(type_error("parseDuration: invalid duration text", span)),
    }
}

fn add_clock_duration(
    args: &[ExecArg],
    op: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [instant, span_arg] = args else {
        return Err(std_arity("clock", op, span));
    };
    let nanos = eval_instant_arg(instant, env, span)?;
    let offset = eval_duration_arg(span_arg, env, span)?;
    nanos
        .checked_add(offset)
        .map(Value::Instant)
        .ok_or_else(|| overflow(span))
}

fn format_scalar(value: SavedValue, span: SourceSpan) -> Result<Value, RuntimeError> {
    let bytes = encode_value(&value).map_err(|error| error.located(span))?;
    Ok(Value::Str(
        String::from_utf8(bytes).expect("a canonical scalar encodes as UTF-8 text"),
    ))
}
