use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, type_error};
use crate::expr::eval_expr;
use crate::value::Value;

/// Build an `Error` resource from a checked `Error(...)` call. The checker owns
/// argument-shape, per-field type, and required-field validation, and rejects an
/// invalid error code when it is a string literal. A code computed at runtime
/// (for example a concatenation) cannot be checked statically, so the constructor
/// validates the resolved `code` text here and faults with `run.type` when it is
/// not a valid dotted lowercase error code.
pub(crate) fn eval_error_constructor(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let fields = marrow_schema::error::fields();
    let mut slots: Vec<Option<Value>> = vec![None; fields.len()];

    for arg in args {
        let index = arg
            .name
            .as_ref()
            .and_then(|name| fields.iter().position(|field| field.name == name))
            .expect("checked error constructor binds each argument to a field");
        debug_assert!(
            slots[index].is_none(),
            "checked error constructor supplies each field at most once",
        );
        let value = eval_expr(&arg.value, env)?;
        if fields[index].name == marrow_schema::error::CODE
            && let Value::Str(text) = &value
            && !marrow_schema::error::is_error_code_text(text)
        {
            return Err(type_error(
                "`Error.code` expects a dotted lowercase error code",
                span,
            ));
        }
        slots[index] = Some(value);
    }

    debug_assert!(
        fields
            .iter()
            .zip(&slots)
            .all(|(field, slot)| !field.required || slot.is_some()),
        "checked error constructor supplies every required field",
    );

    Ok(Value::Resource(
        fields
            .iter()
            .zip(slots)
            .filter_map(|(field, value)| value.map(|value| (field.name.to_string(), value)))
            .collect(),
    ))
}
