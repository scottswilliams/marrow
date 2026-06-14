use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, error_field, std_arity, type_error};
use crate::expr::eval_expr;
use crate::stdlib::eval_text;
use crate::value::Value;

pub(crate) fn eval_error(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "code" | "message" => {
            let [err] = args else {
                return Err(std_arity("error", op, span));
            };
            let err = eval_expr(&err.value, env)?;
            let field = if op == "code" {
                marrow_schema::error::CODE
            } else {
                marrow_schema::error::MESSAGE
            };
            Ok(Value::Str(error_field(&err, field).ok_or_else(|| {
                type_error("std::error helper expects an Error", span)
            })?))
        }
        "hasCode" => {
            let [err, code] = args else {
                return Err(std_arity("error", op, span));
            };
            let err = eval_expr(&err.value, env)?;
            let code = eval_text(code, env, span)?;
            if !marrow_schema::error::is_error_code_text(&code) {
                return Err(type_error(
                    "std::error::hasCode expects an error code",
                    span,
                ));
            }
            let actual = error_field(&err, marrow_schema::error::CODE)
                .ok_or_else(|| type_error("std::error helper expects an Error", span))?;
            Ok(Value::Bool(actual == code))
        }
        _ => Err(crate::error::unsupported(
            &format!("std::error::{op}"),
            span,
        )),
    }
}
