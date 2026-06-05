use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, type_error};
use crate::expr::eval_expr;
use crate::value::Value;

pub(crate) fn eval_error_constructor(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut fields: Vec<(String, Value)> = Vec::new();
    for arg in args {
        let Some(name) = &arg.name else {
            return Err(type_error("`Error(...)` takes named arguments", span));
        };
        if marrow_schema::error::field(name).is_none() {
            return Err(type_error(&format!("`Error` has no field `{name}`"), span));
        }
        if fields.iter().any(|(existing, _)| existing == name) {
            return Err(type_error(
                &format!("`{name}` is supplied more than once"),
                span,
            ));
        }
        fields.push((name.clone(), eval_expr(&arg.value, env)?));
    }
    for required in marrow_schema::error::fields()
        .iter()
        .filter(|field| field.required)
    {
        if !fields.iter().any(|(name, _)| name == required.name) {
            let name = required.name;
            return Err(type_error(&format!("`Error` requires `{name}`"), span));
        }
    }
    Ok(Value::Resource(fields))
}
