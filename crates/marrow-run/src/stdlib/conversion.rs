use marrow_check::CheckedArg as ExecArg;
use marrow_store::Decimal;
use marrow_store::value::{SavedValue, ScalarType, decode_value, encode_value};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{Located, RuntimeError, conversion_error, decimal_overflow, type_error};
use crate::expr::eval_expr;
use crate::value::{Value, saved_value_to_value};

pub(crate) fn eval_bytes_conversion(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("`bytes` takes one argument", span));
    };
    match eval_expr(&arg.value, env)? {
        Value::Str(text) => Ok(Value::Bytes(text.into_bytes())),
        Value::Bytes(bytes) => Ok(Value::Bytes(bytes)),
        _ => Err(conversion_error("bytes", span)),
    }
}

pub(crate) fn eval_conversion(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error(&format!("`{name}` takes one argument"), span));
    };
    let value = eval_expr(&arg.value, env)?;
    match name {
        "bool" => convert_to_bool(value, span),
        "int" => convert_to_int(value, span),
        "decimal" => convert_to_decimal(value, span),
        "string" => convert_to_string(value, span),
        "date" => convert_to_canonical_scalar(value, ScalarType::Date, "date", span),
        "instant" => convert_to_canonical_scalar(value, ScalarType::Instant, "instant", span),
        "duration" => convert_to_canonical_scalar(value, ScalarType::Duration, "duration", span),
        "ErrorCode" => convert_to_error_code(value, span),
        _ => Err(conversion_error(name, span)),
    }
}

fn convert_to_bool(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let result = match &value {
        Value::Bool(_) => return Ok(value),
        Value::Int(0) => false,
        Value::Int(1) => true,
        _ => return Err(conversion_error("bool", span)),
    };
    Ok(Value::Bool(result))
}

fn convert_to_int(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match value {
        Value::Int(_) => Ok(value),
        Value::Str(text) => match decode_value(text.as_bytes(), ScalarType::Int) {
            Some(SavedValue::Int(n)) => Ok(Value::Int(n)),
            _ => Err(conversion_error("int", span)),
        },
        Value::Decimal(decimal) if decimal.scale() == 0 => i64::try_from(decimal.coefficient())
            .map(Value::Int)
            .map_err(|_| conversion_error("int", span)),
        _ => Err(conversion_error("int", span)),
    }
}

fn convert_to_decimal(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match value {
        Value::Decimal(_) => Ok(value),
        Value::Int(n) => Decimal::from_parts(i128::from(n), 0)
            .map(Value::Decimal)
            .ok_or_else(|| conversion_error("decimal", span)),
        Value::Str(text) => match decode_value(text.as_bytes(), ScalarType::Decimal) {
            Some(SavedValue::Decimal(decimal)) => Ok(Value::Decimal(decimal)),
            _ if canonical_decimal_text_shape(&text) => Err(decimal_overflow(span)),
            _ => Err(conversion_error("decimal", span)),
        },
        _ => Err(conversion_error("decimal", span)),
    }
}

fn canonical_decimal_text_shape(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    if text.is_empty() || text == "0" {
        return false;
    }
    let (integer, fraction) = text
        .split_once('.')
        .map_or((text, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    if !canonical_integer_part(integer) {
        return false;
    }
    let Some(fraction) = fraction else {
        return true;
    };
    !fraction.is_empty()
        && fraction.bytes().all(|byte| byte.is_ascii_digit())
        && !fraction.ends_with('0')
}

fn canonical_integer_part(text: &str) -> bool {
    match text.as_bytes() {
        [b'0'] => true,
        [first, rest @ ..] if first.is_ascii_digit() && *first != b'0' => {
            rest.iter().all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
}

fn convert_to_string(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let text = match value {
        Value::Str(text) => text,
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Decimal(decimal) => decimal.to_text(),
        Value::Bytes(bytes) => {
            String::from_utf8(bytes).map_err(|_| conversion_error("string", span))?
        }
        Value::Date(days) => canonical_value_text(SavedValue::Date(days), span)?,
        Value::Instant(nanos) => canonical_value_text(SavedValue::Instant(nanos), span)?,
        Value::Duration(nanos) => canonical_value_text(SavedValue::Duration(nanos), span)?,
        Value::Sequence(_)
        | Value::LocalTree(_)
        | Value::Resource(_)
        | Value::Identity(_)
        | Value::Enum(_) => return Err(conversion_error("string", span)),
    };
    Ok(Value::Str(text))
}

fn convert_to_error_code(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match value {
        Value::Str(text) if is_error_code_text(&text) => Ok(Value::Str(text)),
        _ => Err(conversion_error("ErrorCode", span)),
    }
}

fn is_error_code_text(text: &str) -> bool {
    let mut saw_dot = false;
    let mut segment_has_char = false;
    for byte in text.bytes() {
        match byte {
            b'.' => {
                if !segment_has_char {
                    return false;
                }
                saw_dot = true;
                segment_has_char = false;
            }
            b'a'..=b'z' | b'0'..=b'9' | b'_' => {
                segment_has_char = true;
            }
            _ => return false,
        }
    }
    saw_dot && segment_has_char
}

fn convert_to_canonical_scalar(
    value: Value,
    ty: ScalarType,
    name: &str,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match value {
        Value::Date(_) if ty == ScalarType::Date => Ok(value),
        Value::Instant(_) if ty == ScalarType::Instant => Ok(value),
        Value::Duration(_) if ty == ScalarType::Duration => Ok(value),
        Value::Str(text) => decode_value(text.as_bytes(), ty)
            .map(saved_value_to_value)
            .ok_or_else(|| conversion_error(name, span)),
        _ => Err(conversion_error(name, span)),
    }
}

fn canonical_value_text(value: SavedValue, span: SourceSpan) -> Result<String, RuntimeError> {
    let bytes = encode_value(&value).map_err(|error| error.located(span))?;
    Ok(String::from_utf8(bytes).expect("canonical scalar text is UTF-8"))
}
