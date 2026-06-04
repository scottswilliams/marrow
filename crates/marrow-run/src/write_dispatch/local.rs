use marrow_check::CheckedExpr as ExecExpr;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, assign_error, unsupported};
use crate::expr::eval_expr;
use crate::value::Value;

pub(crate) fn eval_local_field_set(
    base: &ExecExpr,
    field: &str,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let ExecExpr::Name { segments, .. } = base else {
        return Err(unsupported("setting a field of this value", span));
    };
    let [name] = segments.as_slice() else {
        return Err(unsupported("setting a field of this value", span));
    };
    let new_value = eval_expr(value, env)?;
    write_local_field(name, field, new_value, span, env)
}

/// Update (or insert) `field` of the local resource bound to `base` with an
/// already-evaluated value, rebinding the variable. Shared by
/// [`eval_local_field_set`] and `inout` write-back.
pub(crate) fn write_local_field(
    base: &str,
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Some(Value::Resource(mut fields)) = env.lookup(base).cloned() else {
        return Err(unsupported("setting a field of a non-resource local", span));
    };
    match fields.iter().position(|(existing, _)| existing == field) {
        Some(index) => fields[index].1 = value,
        None => fields.push((field.to_string(), value)),
    }
    env.assign(base, Value::Resource(fields))
        .map_err(|error| assign_error(base, error, span))
}
