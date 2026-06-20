use marrow_check::CheckedArg as ExecArg;
use marrow_store::value::{SavedValue, ScalarType, decode_value};
use marrow_store::{Decimal, DecimalParseError};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, conversion_error, decimal_overflow, type_error};
use crate::expr::eval_expr;
use crate::stdlib::{parse_iso8601_duration_nanos, parse_rfc3339_instant_nanos};
use crate::value::{Value, canonical_scalar_text, diagnostic_value_preview, saved_value_to_value};

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
        other => Err(conversion_error_for_value(&other, "bytes", span)),
    }
}

fn convert_to_bool(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let result = match &value {
        Value::Bool(_) => return Ok(value),
        Value::Int(0) => false,
        Value::Int(1) => true,
        _ => return Err(conversion_error_for_value(&value, "bool", span)),
    };
    Ok(Value::Bool(result))
}

fn convert_to_int(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match &value {
        Value::Int(_) => Ok(value),
        Value::Str(text) => match decode_value(text.as_bytes(), ScalarType::Int) {
            Some(SavedValue::Int(n)) => Ok(Value::Int(n)),
            _ => Err(conversion_error_for_value(&value, "int", span)),
        },
        Value::Decimal(decimal) if decimal.scale() == 0 => i64::try_from(decimal.coefficient())
            .map(Value::Int)
            .map_err(|_| conversion_error_for_value(&value, "int", span)),
        _ => Err(conversion_error_for_value(&value, "int", span)),
    }
}

fn convert_to_decimal(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match &value {
        Value::Decimal(_) => Ok(value),
        Value::Int(n) => Decimal::from_parts(i128::from(*n), 0)
            .map(Value::Decimal)
            .ok_or_else(|| conversion_error_for_value(&value, "decimal", span)),
        // The store owns the overflow-vs-malformed distinction: a canonical decimal
        // spelling that exceeds the envelope is a recoverable decimal overflow,
        // while any other text is a type error.
        Value::Str(text) => match Decimal::parse_canonical(text) {
            Ok(decimal) => Ok(Value::Decimal(decimal)),
            Err(DecimalParseError::Overflow) => Err(decimal_overflow(span)),
            Err(DecimalParseError::Malformed) => {
                Err(conversion_error_for_value(&value, "decimal", span))
            }
        },
        _ => Err(conversion_error_for_value(&value, "decimal", span)),
    }
}

fn convert_to_string(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let text = match &value {
        Value::Str(text) => text.clone(),
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Decimal(decimal) => decimal.to_text(),
        Value::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_string(),
            Err(_) => return Err(conversion_error_for_value(&value, "string", span)),
        },
        Value::Date(days) => canonical_scalar_text(SavedValue::Date(*days), span)?,
        Value::Instant(nanos) => canonical_scalar_text(SavedValue::Instant(*nanos), span)?,
        Value::Duration(nanos) => canonical_scalar_text(SavedValue::Duration(*nanos), span)?,
        Value::Sequence(_)
        | Value::LocalTree(_)
        | Value::Resource(_)
        | Value::Identity(_)
        | Value::Enum(_) => return Err(conversion_error_for_value(&value, "string", span)),
    };
    Ok(Value::Str(text))
}

/// Validate a value coerced into an `ErrorCode` place, returning it unchanged when
/// it satisfies the dotted-lowercase grammar and a catchable `run.type` error
/// otherwise. The runtime's one error-code coercion gate, shared by the
/// `ErrorCode(...)` constructor and a dynamic value stored into an `ErrorCode`
/// field or binding, so invalid code text can never reach saved data.
pub(crate) fn convert_to_error_code(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match &value {
        Value::Str(text) if marrow_schema::error::is_error_code_text(text) => {
            Ok(Value::Str(text.clone()))
        }
        _ => Err(conversion_error_for_value(&value, "ErrorCode", span)),
    }
}

fn convert_to_canonical_scalar(
    value: Value,
    ty: ScalarType,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match &value {
        Value::Date(_) if ty == ScalarType::Date => Ok(value),
        Value::Instant(_) if ty == ScalarType::Instant => Ok(value),
        Value::Duration(_) if ty == ScalarType::Duration => Ok(value),
        // Instants and durations from text share the wider standard input surface;
        // dates read through the canonical store decoder.
        Value::Str(text) if ty == ScalarType::Instant => parse_rfc3339_instant_nanos(text)
            .map(Value::Instant)
            .ok_or_else(|| conversion_error_for_value(&value, ty.name(), span)),
        Value::Str(text) if ty == ScalarType::Duration => parse_iso8601_duration_nanos(text)
            .map(Value::Duration)
            .ok_or_else(|| conversion_error_for_value(&value, ty.name(), span)),
        Value::Str(text) => decode_value(text.as_bytes(), ty)
            .map(saved_value_to_value)
            .ok_or_else(|| conversion_error_for_value(&value, ty.name(), span)),
        _ => Err(conversion_error_for_value(&value, ty.name(), span)),
    }
}

fn conversion_error_for_value(value: &Value, name: &str, span: SourceSpan) -> RuntimeError {
    match diagnostic_value_preview(value) {
        Some(preview) => type_error(&format!("cannot convert value {preview} to {name}"), span),
        None => conversion_error(name, span),
    }
}
