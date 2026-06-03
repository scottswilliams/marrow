use marrow_check::CheckedArg as ExecArg;
use marrow_store::Decimal;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, type_error};
use crate::expr::eval_expr;
use crate::value::Value;

pub(crate) fn eval_typed_arg<T>(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
    expected: &str,
    extract: impl FnOnce(Value) -> Option<T>,
) -> Result<T, RuntimeError> {
    extract(eval_expr(&arg.value, env)?)
        .ok_or_else(|| type_error(&format!("expected {expected}"), span))
}

pub(crate) fn eval_bytes_arg(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Vec<u8>, RuntimeError> {
    eval_typed_arg(arg, env, span, "bytes", |value| match value {
        Value::Bytes(bytes) => Some(bytes),
        _ => None,
    })
}

pub(crate) fn eval_decimal_arg(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Decimal, RuntimeError> {
    eval_typed_arg(arg, env, span, "a decimal", |value| match value {
        Value::Decimal(decimal) => Some(decimal),
        _ => None,
    })
}

pub(crate) fn eval_instant_arg(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<i128, RuntimeError> {
    eval_typed_arg(arg, env, span, "an instant", |value| match value {
        Value::Instant(nanos) => Some(nanos),
        _ => None,
    })
}

pub(crate) fn eval_date_arg(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<i32, RuntimeError> {
    eval_typed_arg(arg, env, span, "a date", |value| match value {
        Value::Date(days) => Some(days),
        _ => None,
    })
}

pub(crate) fn eval_duration_arg(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<i128, RuntimeError> {
    eval_typed_arg(arg, env, span, "a duration", |value| match value {
        Value::Duration(nanos) => Some(nanos),
        _ => None,
    })
}

pub(crate) fn eval_text(
    arg: &ExecArg,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<String, RuntimeError> {
    eval_typed_arg(arg, env, span, "a string", |value| match value {
        Value::Str(text) => Some(text),
        _ => None,
    })
}
