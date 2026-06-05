use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, type_error};
use crate::expr::eval_expr;
use crate::value::{Value, render};

/// The two output builtins, which differ only in whether they append a trailing
/// newline. The checker already resolved which one was called, so the runtime
/// carries this typed kind rather than re-deriving it from a name string.
#[derive(Debug, Clone, Copy)]
pub(crate) enum OutputKind {
    Print,
    Write,
}

impl OutputKind {
    fn trailing_newline(self) -> bool {
        matches!(self, OutputKind::Print)
    }

    /// The builtin's language spelling, used only as the host-effect guard label.
    fn label(self) -> &'static str {
        match self {
            OutputKind::Print => "print",
            OutputKind::Write => "write",
        }
    }
}

pub(crate) fn eval_output(
    kind: OutputKind,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let [arg] = args else {
        return Err(type_error(
            &format!("`{}` takes one argument", kind.label()),
            span,
        ));
    };
    let text = render(eval_expr(&arg.value, env)?, span)?;
    env.guard_rollback_sensitive_host_effect(kind.label(), span)?;
    let mut output = env.output.borrow_mut();
    output.push_str(&text);
    if kind.trailing_newline() {
        output.push('\n');
    }
    Ok(None)
}
