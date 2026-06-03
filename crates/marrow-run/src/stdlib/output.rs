use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, type_error};
use crate::expr::eval_expr;
use crate::value::{Value, render};

pub(crate) fn eval_output(
    name: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let [arg] = args else {
        return Err(type_error(&format!("`{name}` takes one argument"), span));
    };
    let text = render(eval_expr(&arg.value, env)?, span)?;
    env.guard_rollback_sensitive_host_effect(name, span)?;
    let mut output = env.output.borrow_mut();
    output.push_str(&text);
    if name == "print" {
        output.push('\n');
    }
    Ok(None)
}
