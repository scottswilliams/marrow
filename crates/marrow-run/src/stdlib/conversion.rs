use marrow_check::CheckedArg as ExecArg;
use marrow_store::value::{SavedValue, ScalarType, decode_value};
use marrow_store::{Decimal, DecimalParseError};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, conversion_error, decimal_overflow, type_error};
use crate::expr::eval_expr;
use crate::value::{Value, canonical_scalar_text, saved_value_to_value};

/// The conversion a checked call resolves to: a scalar target or the `ErrorCode`
/// spelling, whose storage envelope is a string. The checker already settled
/// which one through `CheckedBuiltinCall`, so the runtime branches on this typed
/// kind rather than re-parsing a name string.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ConversionKind {
    Scalar(ScalarType),
    ErrorCode,
}

impl ConversionKind {
    /// The conversion's language spelling, used only to render error messages.
    fn name(self) -> &'static str {
        match self {
            ConversionKind::Scalar(scalar) => scalar.name(),
            ConversionKind::ErrorCode => "ErrorCode",
        }
    }
}

pub(crate) fn eval_bytes_conversion(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("`bytes` takes one argument", span));
    };
    convert_to_bytes(eval_expr(&arg.value, env)?, span)
}

pub(crate) fn eval_conversion(
    kind: ConversionKind,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error(
            &format!("`{}` takes one argument", kind.name()),
            span,
        ));
    };
    let value = eval_expr(&arg.value, env)?;
    match kind {
        ConversionKind::ErrorCode => convert_to_error_code(value, span),
        ConversionKind::Scalar(scalar) => match scalar {
            ScalarType::Bool => convert_to_bool(value, span),
            ScalarType::Int => convert_to_int(value, span),
            ScalarType::Decimal => convert_to_decimal(value, span),
            ScalarType::Str => convert_to_string(value, span),
            ScalarType::Bytes => convert_to_bytes(value, span),
            ScalarType::Date | ScalarType::Instant | ScalarType::Duration => {
                convert_to_canonical_scalar(value, scalar, span)
            }
        },
    }
}

fn convert_to_bytes(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match value {
        Value::Str(text) => Ok(Value::Bytes(text.into_bytes())),
        Value::Bytes(bytes) => Ok(Value::Bytes(bytes)),
        _ => Err(conversion_error("bytes", span)),
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
        // The store owns the overflow-vs-malformed distinction: a canonical decimal
        // spelling that exceeds the envelope is a recoverable decimal overflow,
        // while any other text is a type error.
        Value::Str(text) => match Decimal::parse_canonical(&text) {
            Ok(decimal) => Ok(Value::Decimal(decimal)),
            Err(DecimalParseError::Overflow) => Err(decimal_overflow(span)),
            Err(DecimalParseError::Malformed) => Err(conversion_error("decimal", span)),
        },
        _ => Err(conversion_error("decimal", span)),
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
        Value::Date(days) => canonical_scalar_text(SavedValue::Date(days), span)?,
        Value::Instant(nanos) => canonical_scalar_text(SavedValue::Instant(nanos), span)?,
        Value::Duration(nanos) => canonical_scalar_text(SavedValue::Duration(nanos), span)?,
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
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match value {
        Value::Date(_) if ty == ScalarType::Date => Ok(value),
        Value::Instant(_) if ty == ScalarType::Instant => Ok(value),
        Value::Duration(_) if ty == ScalarType::Duration => Ok(value),
        Value::Str(text) => decode_value(text.as_bytes(), ty)
            .map(saved_value_to_value)
            .ok_or_else(|| conversion_error(ty.name(), span)),
        _ => Err(conversion_error(ty.name(), span)),
    }
}
