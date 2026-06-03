use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RUN_ASSERT, RuntimeError, raise_fault, type_error, unsupported};
use crate::expr::eval_expr;
use crate::path::saved_path_present;
use crate::value::Value;

pub(crate) fn eval_assert(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match op {
        "isTrue" | "isFalse" => eval_bool_assert(op, args, span, env),
        "absent" => eval_absent_assert(args, span, env),
        "fail" => eval_fail_assert(args, span, env),
        other => Err(unsupported(&format!("std::assert::{other}"), span)),
    }
}

fn eval_bool_assert(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let [arg] = args else {
        return Err(type_error(
            &format!("`std::assert::{op}` takes one boolean"),
            span,
        ));
    };
    let Value::Bool(actual) = eval_expr(&arg.value, env)? else {
        return Err(type_error(
            &format!("`std::assert::{op}` takes a boolean"),
            span,
        ));
    };
    if actual != (op == "isTrue") {
        return Err(raise_fault(
            RUN_ASSERT,
            format!("assertion failed: {op}({actual})"),
            span,
        ));
    }
    Ok(None)
}

fn eval_absent_assert(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("`std::assert::absent` takes one path", span));
    };
    if saved_path_present(&arg.value, span, env)? {
        return Err(raise_fault(
            RUN_ASSERT,
            "assertion failed: expected the path to be absent".into(),
            span,
        ));
    }
    Ok(None)
}

fn eval_fail_assert(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("`std::assert::fail` takes one message", span));
    };
    let Value::Str(message) = eval_expr(&arg.value, env)? else {
        return Err(type_error(
            "`std::assert::fail` takes a string message",
            span,
        ));
    };
    Err(raise_fault(RUN_ASSERT, message, span))
}
