//! Pure `std::` helpers with no host capability.

use marrow_check::CheckedArg as ExecArg;
use marrow_schema::stdlib;
use marrow_store::Decimal;
use marrow_store::value::{SavedValue, ScalarType, date_days, date_parts, decode_value};
use marrow_syntax::SourceSpan;

use crate::base64;
use crate::collection::absent_read;
use crate::env::Env;
use crate::error::{RuntimeError, overflow, std_arity, temporal_overflow, type_error, unsupported};
use crate::expr::eval_int;
use crate::stdlib::{
    eval_bytes_arg, eval_date_arg, eval_decimal_arg, eval_duration_arg, eval_instant_arg,
    eval_text, int_modulo, int_remainder,
};
use crate::value::{Value, canonical_scalar_text, diagnostic_text_preview, saved_value_to_value};

pub(crate) fn eval_std(
    module: &str,
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Some(entry) = stdlib::lookup(module, op) else {
        return Err(unsupported(&format!("std::{module}::{op}"), span));
    };
    if entry.requires_capability.is_some() || entry.module == "assert" {
        return Err(unsupported(&format!("std::{module}::{op}"), span));
    }
    match entry.module {
        "text" => eval_text_std(op, args, span, env),
        "math" => eval_math_std(op, args, span, env),
        "bytes" => eval_bytes_std(op, args, span, env),
        "clock" => eval_clock_std(op, args, span, env),
        "json" => crate::std_json::eval_json(op, args, span, env),
        "csv" => crate::std_csv::eval_csv(op, args, span, env),
        "id" => crate::std_id::eval_id(op, args, span, env),
        "random" => crate::std_random::eval_random(op, args, span, env),
        "audit" => crate::std_audit::eval_audit(op, args, span, env),
        "error" => crate::std_error_helpers::eval_error(op, args, span, env),
        "matrix" => crate::std_matrix::eval_matrix(op, args, span, env),
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
        "length" => eval_text_length(args, span, env),
        "trim" => eval_text_trim(args, span, env),
        "contains" => eval_text_contains(args, span, env),
        "split" => eval_text_split(args, span, env),
        "slice" => eval_text_slice(args, span, env),
        "startsWith" => eval_text_starts_with(args, span, env),
        "endsWith" => eval_text_ends_with(args, span, env),
        "indexOf" => eval_text_index_of(args, span, env),
        "replace" => eval_text_replace(args, span, env),
        "join" => eval_text_join(args, span, env),
        "toUpper" => eval_text_to_upper(args, span, env),
        "toLower" => eval_text_to_lower(args, span, env),
        other => Err(unsupported(&format!("std::text::{other}"), span)),
    }
}

fn eval_text_length(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text] = args else {
        return Err(std_arity("text", "length", span));
    };
    Ok(Value::Int(
        eval_text(text, env, span)?.chars().count() as i64
    ))
}

fn eval_text_trim(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text] = args else {
        return Err(std_arity("text", "trim", span));
    };
    Ok(Value::Str(eval_text(text, env, span)?.trim().to_string()))
}

fn eval_text_contains(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text, needle] = args else {
        return Err(std_arity("text", "contains", span));
    };
    let text = eval_text(text, env, span)?;
    let needle = eval_text(needle, env, span)?;
    Ok(Value::Bool(text.contains(&needle)))
}

fn eval_text_split(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text, separator] = args else {
        return Err(std_arity("text", "split", span));
    };
    let text = eval_text(text, env, span)?;
    let separator = eval_text(separator, env, span)?;
    Ok(Value::Sequence(
        text.split(separator.as_str())
            .map(|part| Value::Str(part.to_string()))
            .collect(),
    ))
}

fn eval_text_slice(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text, start, end] = args else {
        return Err(std_arity("text", "slice", span));
    };
    let text = eval_text(text, env, span)?;
    let start = eval_text_index(start, env, span)?;
    let end = eval_text_index(end, env, span)?;
    if start > end {
        return Err(type_error(
            "slice start must be less than or equal to end",
            span,
        ));
    }
    let len = text.chars().count();
    if end > len {
        return Err(type_error("slice index is outside the text", span));
    }
    Ok(Value::Str(
        text.chars().skip(start).take(end - start).collect(),
    ))
}

fn eval_text_starts_with(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text, prefix] = args else {
        return Err(std_arity("text", "startsWith", span));
    };
    Ok(Value::Bool(
        eval_text(text, env, span)?.starts_with(&eval_text(prefix, env, span)?),
    ))
}

fn eval_text_ends_with(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text, suffix] = args else {
        return Err(std_arity("text", "endsWith", span));
    };
    Ok(Value::Bool(
        eval_text(text, env, span)?.ends_with(&eval_text(suffix, env, span)?),
    ))
}

fn eval_text_index_of(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text, needle] = args else {
        return Err(std_arity("text", "indexOf", span));
    };
    let text = eval_text(text, env, span)?;
    let needle = eval_text(needle, env, span)?;
    let Some(byte_index) = text.find(&needle) else {
        return Err(absent_read(
            "`std::text::indexOf` found no match".into(),
            span,
        ));
    };
    Ok(Value::Int(text[..byte_index].chars().count() as i64))
}

fn eval_text_replace(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text, from, to] = args else {
        return Err(std_arity("text", "replace", span));
    };
    Ok(Value::Str(eval_text(text, env, span)?.replace(
        &eval_text(from, env, span)?,
        &eval_text(to, env, span)?,
    )))
}

fn eval_text_join(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [parts, separator] = args else {
        return Err(std_arity("text", "join", span));
    };
    let parts = eval_string_sequence(parts, env, span)?;
    let separator = eval_text(separator, env, span)?;
    Ok(Value::Str(parts.join(&separator)))
}

fn eval_text_to_upper(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text] = args else {
        return Err(std_arity("text", "toUpper", span));
    };
    Ok(Value::Str(
        eval_text(text, env, span)?
            .chars()
            .map(simple_uppercase)
            .collect(),
    ))
}

fn eval_text_to_lower(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [text] = args else {
        return Err(std_arity("text", "toLower", span));
    };
    Ok(Value::Str(
        eval_text(text, env, span)?
            .chars()
            .map(simple_lowercase)
            .collect(),
    ))
}

fn simple_uppercase(value: char) -> char {
    match value {
        '\u{1F80}'..='\u{1F87}' | '\u{1F90}'..='\u{1F97}' | '\u{1FA0}'..='\u{1FA7}' => {
            return char::from_u32(value as u32 + 8).unwrap_or(value);
        }
        '\u{1FB3}' => return '\u{1FBC}',
        '\u{1FC3}' => return '\u{1FCC}',
        '\u{1FF3}' => return '\u{1FFC}',
        _ => {}
    }
    let mut mapped = value.to_uppercase();
    let Some(first) = mapped.next() else {
        return value;
    };
    if mapped.next().is_some() {
        value
    } else {
        first
    }
}

fn simple_lowercase(value: char) -> char {
    if value == '\u{0130}' {
        return 'i';
    }
    let mut mapped = value.to_lowercase();
    let Some(first) = mapped.next() else {
        return value;
    };
    if mapped.next().is_some() {
        value
    } else {
        first
    }
}

fn eval_math_std(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "absInt" => eval_math_abs_int(args, span, env),
        "remainder" => eval_math_remainder(args, span, env),
        "modulo" => eval_math_modulo(args, span, env),
        "absDecimal" => eval_math_abs_decimal(args, span, env),
        "floor" => eval_math_floor(args, span, env),
        "minInt" => eval_math_min_int(args, span, env),
        "maxInt" => eval_math_max_int(args, span, env),
        "minDecimal" => eval_math_min_decimal(args, span, env),
        "maxDecimal" => eval_math_max_decimal(args, span, env),
        "round" => eval_math_round(args, span, env),
        "roundDecimal" => eval_math_round_decimal(args, span, env),
        "ceiling" => eval_math_ceiling(args, span, env),
        "powInt" => eval_math_pow_int(args, span, env),
        "clampInt" => eval_math_clamp_int(args, span, env),
        "clampDecimal" => eval_math_clamp_decimal(args, span, env),
        other => Err(unsupported(&format!("std::math::{other}"), span)),
    }
}

fn eval_math_abs_int(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [value] = args else {
        return Err(std_arity("math", "absInt", span));
    };
    Ok(Value::Int(
        eval_int(&value.value, env)?
            .checked_abs()
            .ok_or_else(|| overflow(span))?,
    ))
}

fn eval_math_remainder(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [a, b] = args else {
        return Err(std_arity("math", "remainder", span));
    };
    let remainder = int_remainder(eval_int(&a.value, env)?, eval_int(&b.value, env)?, span)?;
    Ok(Value::Int(remainder))
}

fn eval_math_modulo(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [a, b] = args else {
        return Err(std_arity("math", "modulo", span));
    };
    let modulo = int_modulo(eval_int(&a.value, env)?, eval_int(&b.value, env)?, span)?;
    Ok(Value::Int(modulo))
}

fn eval_math_abs_decimal(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [value] = args else {
        return Err(std_arity("math", "absDecimal", span));
    };
    Ok(Value::Decimal(eval_decimal_arg(value, env, span)?.abs()))
}

fn eval_math_floor(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [value] = args else {
        return Err(std_arity("math", "floor", span));
    };
    decimal_to_i64(eval_decimal_arg(value, env, span)?.floor(), span).map(Value::Int)
}

fn eval_math_min_int(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [a, b] = args else {
        return Err(std_arity("math", "minInt", span));
    };
    Ok(Value::Int(
        eval_int(&a.value, env)?.min(eval_int(&b.value, env)?),
    ))
}

fn eval_math_max_int(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [a, b] = args else {
        return Err(std_arity("math", "maxInt", span));
    };
    Ok(Value::Int(
        eval_int(&a.value, env)?.max(eval_int(&b.value, env)?),
    ))
}

fn eval_math_min_decimal(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [a, b] = args else {
        return Err(std_arity("math", "minDecimal", span));
    };
    Ok(Value::Decimal(
        eval_decimal_arg(a, env, span)?.min(eval_decimal_arg(b, env, span)?),
    ))
}

fn eval_math_max_decimal(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [a, b] = args else {
        return Err(std_arity("math", "maxDecimal", span));
    };
    Ok(Value::Decimal(
        eval_decimal_arg(a, env, span)?.max(eval_decimal_arg(b, env, span)?),
    ))
}

fn eval_math_round(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [value] = args else {
        return Err(std_arity("math", "round", span));
    };
    decimal_to_i64(
        round_decimal_half_even_to_integer(eval_decimal_arg(value, env, span)?),
        span,
    )
    .map(Value::Int)
}

fn eval_math_round_decimal(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [value, scale] = args else {
        return Err(std_arity("math", "roundDecimal", span));
    };
    let value = eval_decimal_arg(value, env, span)?;
    let scale = eval_round_decimal_scale(scale, env, span)?;
    let rounded = value.round_to_scale(scale).ok_or_else(|| overflow(span))?;
    Ok(Value::Decimal(rounded))
}

fn eval_round_decimal_scale(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<u32, RuntimeError> {
    let scale = eval_int(&arg.value, env)?;
    let scale = u32::try_from(scale)
        .map_err(|_| type_error("roundDecimal scale must be in 0..=34", span))?;
    if scale > Decimal::MAX_SCALE {
        return Err(type_error("roundDecimal scale must be in 0..=34", span));
    }
    Ok(scale)
}

fn eval_math_ceiling(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [value] = args else {
        return Err(std_arity("math", "ceiling", span));
    };
    decimal_to_i64(ceiling_decimal(eval_decimal_arg(value, env, span)?), span).map(Value::Int)
}

fn eval_math_pow_int(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [base, exp] = args else {
        return Err(std_arity("math", "powInt", span));
    };
    let exp = eval_int(&exp.value, env)?;
    if exp < 0 {
        return Err(type_error("powInt exponent must be non-negative", span));
    }
    let base = eval_int(&base.value, env)?;
    let value = match u32::try_from(exp) {
        Ok(exp) => base.checked_pow(exp).ok_or_else(|| overflow(span))?,
        Err(_) => match base {
            -1 => {
                if exp % 2 == 0 {
                    1
                } else {
                    -1
                }
            }
            0 => 0,
            1 => 1,
            _ => return Err(overflow(span)),
        },
    };
    Ok(Value::Int(value))
}

fn eval_math_clamp_int(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [value, min, max] = args else {
        return Err(std_arity("math", "clampInt", span));
    };
    let value = eval_int(&value.value, env)?;
    let min = eval_int(&min.value, env)?;
    let max = eval_int(&max.value, env)?;
    if min > max {
        return Err(type_error("clampInt min must be <= max", span));
    }
    Ok(Value::Int(value.clamp(min, max)))
}

fn eval_math_clamp_decimal(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [value, min, max] = args else {
        return Err(std_arity("math", "clampDecimal", span));
    };
    let value = eval_decimal_arg(value, env, span)?;
    let min = eval_decimal_arg(min, env, span)?;
    let max = eval_decimal_arg(max, env, span)?;
    if min > max {
        return Err(type_error("clampDecimal min must be <= max", span));
    }
    Ok(Value::Decimal(value.clamp(min, max)))
}

fn eval_text_index(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<usize, RuntimeError> {
    let value = eval_int(&arg.value, env)?;
    usize::try_from(value).map_err(|_| type_error("text index must be non-negative", span))
}

fn eval_string_sequence(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Vec<String>, RuntimeError> {
    match crate::expr::eval_expr(&arg.value, env)? {
        Value::Sequence(items) => items
            .into_iter()
            .map(|value| match value {
                Value::Str(text) => Ok(text),
                _ => Err(type_error("join parts must be strings", span)),
            })
            .collect(),
        _ => Err(type_error("join parts must be a string sequence", span)),
    }
}

fn round_decimal_half_even_to_integer(value: Decimal) -> i128 {
    value
        .round_to_scale(0)
        .expect("scale zero is inside the decimal envelope")
        .coefficient()
}

fn ceiling_decimal(value: Decimal) -> i128 {
    if value.scale() == 0 {
        return value.coefficient();
    }
    let divisor = 10i128.pow(value.scale());
    let quotient = value.coefficient() / divisor;
    let remainder = value.coefficient() % divisor;
    if value.coefficient() > 0 && remainder != 0 {
        quotient + 1
    } else {
        quotient
    }
}

fn decimal_to_i64(value: i128, span: SourceSpan) -> Result<i64, RuntimeError> {
    i64::try_from(value).map_err(|_| overflow(span))
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
    let unary = || -> Result<&ExecArg, RuntimeError> {
        match args {
            [value] => Ok(value),
            _ => Err(std_arity("clock", op, span)),
        }
    };
    match op {
        "formatInstant" => format_scalar(
            SavedValue::Instant(eval_instant_arg(unary()?, env, span)?),
            span,
        ),
        "parseInstant" => parse_clock(
            &eval_text(unary()?, env, span)?,
            ScalarType::Instant,
            "parseInstant: invalid instant text",
            span,
        ),
        "formatDate" => format_scalar(SavedValue::Date(eval_date_arg(unary()?, env, span)?), span),
        "parseDate" => {
            let text = eval_text(unary()?, env, span)?;
            parse_clock(
                &text,
                ScalarType::Date,
                &format!(
                    "parseDate: invalid date text {}",
                    diagnostic_text_preview(&text)
                ),
                span,
            )
        }
        "formatDuration" => format_scalar(
            SavedValue::Duration(eval_duration_arg(unary()?, env, span)?),
            span,
        ),
        "parseDuration" => {
            let text = eval_text(unary()?, env, span)?;
            parse_clock(
                &text,
                ScalarType::Duration,
                &format!(
                    "parseDuration: invalid duration text {}",
                    diagnostic_text_preview(&text)
                ),
                span,
            )
        }
        "addDays" => eval_clock_add_days(args, span, env),
        "daysBetween" => eval_clock_days_between(args, span, env),
        other => match ClockDatePart::from_name(other) {
            Some(part) => eval_clock_date_part(part, args, span, env),
            None => Err(unsupported(&format!("std::clock::{other}"), span)),
        },
    }
}

#[derive(Clone, Copy)]
enum ClockDatePart {
    Year,
    Month,
    Day,
}

impl ClockDatePart {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "year" => Some(Self::Year),
            "month" => Some(Self::Month),
            "day" => Some(Self::Day),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
        }
    }
}

fn eval_clock_add_days(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [date, days] = args else {
        return Err(std_arity("clock", "addDays", span));
    };
    let date = require_supported_date(eval_date_arg(date, env, span)?, span)?;
    let days = eval_int(&days.value, env)?;
    let result = i64::from(date)
        .checked_add(days)
        .ok_or_else(|| temporal_overflow(span))?;
    let result = i32::try_from(result).map_err(|_| temporal_overflow(span))?;
    Ok(Value::Date(require_supported_date(result, span)?))
}

fn eval_clock_days_between(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [start, end] = args else {
        return Err(std_arity("clock", "daysBetween", span));
    };
    let start = require_supported_date(eval_date_arg(start, env, span)?, span)?;
    let end = require_supported_date(eval_date_arg(end, env, span)?, span)?;
    Ok(Value::Int(i64::from(end) - i64::from(start)))
}

fn eval_clock_date_part(
    part: ClockDatePart,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [date] = args else {
        return Err(std_arity("clock", part.name(), span));
    };
    let parts =
        date_parts(eval_date_arg(date, env, span)?).ok_or_else(|| temporal_overflow(span))?;
    let value = match part {
        ClockDatePart::Year => i64::from(parts.year),
        ClockDatePart::Month => i64::from(parts.month),
        ClockDatePart::Day => i64::from(parts.day),
    };
    Ok(Value::Int(value))
}

fn require_supported_date(days: i32, span: SourceSpan) -> Result<i32, RuntimeError> {
    let min = date_days(1, 1, 1).expect("year 0001 lower date bound is valid");
    let max = date_days(9999, 12, 31).expect("year 9999 upper date bound is valid");
    if (min..=max).contains(&days) {
        Ok(days)
    } else {
        Err(temporal_overflow(span))
    }
}

/// Decode canonical temporal text back to its runtime value, faulting on text the
/// codec rejects for that scalar type.
fn parse_clock(
    text: &str,
    ty: ScalarType,
    invalid_message: &str,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    decode_value(text.as_bytes(), ty)
        .map(saved_value_to_value)
        .ok_or_else(|| type_error(invalid_message, span))
}

fn format_scalar(value: SavedValue, span: SourceSpan) -> Result<Value, RuntimeError> {
    canonical_scalar_text(value, span).map(Value::Str)
}
