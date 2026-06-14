use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, std_arity};
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
                json_string(&eval_text(action, env, span)?),
                json_string(&eval_text(actor, env, span)?),
                json_string(&eval_text(subject, env, span)?),
            )))
        }
        "change" => {
            let [field, before, after] = args else {
                return Err(std_arity("audit", op, span));
            };
            Ok(Value::Str(format!(
                "{{\"field\":{},\"before\":{},\"after\":{}}}",
                json_string(&eval_text(field, env, span)?),
                json_string(&eval_text(before, env, span)?),
                json_string(&eval_text(after, env, span)?),
            )))
        }
        _ => Err(crate::error::unsupported(
            &format!("std::audit::{op}"),
            span,
        )),
    }
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).expect("serializing a string cannot fail")
}
