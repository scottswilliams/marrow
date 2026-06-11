use marrow_check::CheckedArg as ExecArg;
use marrow_schema::Type;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, type_error};
use crate::expr::eval_expr;
use crate::value::{Value, value_scalar_type};

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
        let Some(field) = marrow_schema::error::field(name) else {
            return Err(type_error(&format!("`Error` has no field `{name}`"), span));
        };
        if fields.iter().any(|(existing, _)| existing == name) {
            return Err(type_error(
                &format!("`{name}` is supplied more than once"),
                span,
            ));
        }
        let value = eval_expr(&arg.value, env)?;
        if !value_matches_type(&value, &field.ty) {
            return Err(type_error(
                &format!("`Error.{name}` expects {}", field.ty),
                span,
            ));
        }
        if name == marrow_schema::error::CODE
            && !matches!(&value, Value::Str(text) if marrow_schema::error::is_error_code_text(text))
        {
            return Err(type_error(
                "`Error.code` expects a dotted lowercase error code",
                span,
            ));
        }
        fields.push((name.clone(), value));
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

fn value_matches_type(value: &Value, ty: &Type) -> bool {
    match ty {
        Type::Unknown => true,
        Type::Scalar(scalar) => value_scalar_type(value) == Some(*scalar),
        Type::Sequence(_) | Type::Identity(_) | Type::Named(_) => false,
    }
}
