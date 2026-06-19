use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, std_arity};
use crate::std_json::string_literal;
use crate::stdlib::eval_text;
use crate::value::Value;

pub(crate) fn eval_audit(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        "event" => {
            let [action, actor, subject] = args else {
                return Err(std_arity("audit", op, span));
            };
            Ok(Value::Str(format!(
                "{{\"action\":{},\"actor\":{},\"subject\":{}}}",
                string_literal(&eval_text(action, env, span)?),
                string_literal(&eval_text(actor, env, span)?),
                string_literal(&eval_text(subject, env, span)?),
            )))
        }
        "change" => {
            let [field, before, after] = args else {
                return Err(std_arity("audit", op, span));
            };
            Ok(Value::Str(format!(
                "{{\"field\":{},\"before\":{},\"after\":{}}}",
                string_literal(&eval_text(field, env, span)?),
                string_literal(&eval_text(before, env, span)?),
                string_literal(&eval_text(after, env, span)?),
            )))
        }
        _ => Err(crate::error::unsupported(
            &format!("std::audit::{op}"),
            span,
        )),
    }
}
