//! Runtime handlers for host-backed stdlib capabilities.

use marrow_check::CheckedArg as ExecArg;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{
    RUN_ABSENT, RUN_CAPABILITY, RuntimeError, error_field, io_error, raise, raise_fault, std_arity,
    type_error,
};
use crate::expr::eval_expr;
use crate::stdlib::eval_text;
use crate::value::Value;

const NANOS_PER_DAY: i128 = 86_400_000_000_000;

pub(crate) fn eval_clock_capability(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if !args.is_empty() {
        return Err(type_error(
            &format!("`std::clock::{op}` takes no arguments"),
            span,
        ));
    }
    let nanos = env.host.clock.ok_or_else(|| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_CAPABILITY,
        message: format!("this run provides no clock capability for `std::clock::{op}`"),
        span,
    })?;
    match op {
        "now" => Ok(Value::Instant(nanos)),
        "today" => Ok(Value::Date(nanos.div_euclid(NANOS_PER_DAY) as i32)),
        _ => unreachable!("the stdlib table routes only `now`/`today` to the clock capability"),
    }
}

pub(crate) fn eval_env(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let names: Vec<String> = args
        .iter()
        .map(|arg| eval_text(arg, env, span))
        .collect::<Result<_, _>>()?;
    let variables = env.host.environment.as_ref().ok_or_else(|| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_CAPABILITY,
        message: format!("this run provides no environment capability for `std::env::{op}`"),
        span,
    })?;
    match (op, names.as_slice()) {
        ("exists", [name]) => Ok(Value::Bool(variables.contains_key(name))),
        ("get", [name, default]) => Ok(Value::Str(
            variables
                .get(name)
                .cloned()
                .unwrap_or_else(|| default.clone()),
        )),
        ("require", [name]) => match variables.get(name).cloned() {
            Some(value) => Ok(Value::Str(value)),
            None => Err(raise_fault(
                RUN_ABSENT,
                format!("required environment variable `{name}` is absent"),
                span,
            )),
        },
        ("exists" | "get" | "require", _) => Err(std_arity("env", op, span)),
        _ => unreachable!(
            "the stdlib table routes only `exists`/`get`/`require` to the env capability"
        ),
    }
}

pub(crate) fn eval_log(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let values: Vec<Value> = args
        .iter()
        .map(|arg| eval_expr(&arg.value, env))
        .collect::<Result<_, _>>()?;
    let sink = env.host.log.as_ref().ok_or_else(|| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_CAPABILITY,
        message: format!("this run provides no log capability for `std::log::{op}`"),
        span,
    })?;
    let line = match (op, values.as_slice()) {
        ("info", [Value::Str(message)]) => format!("INFO {message}\n"),
        ("warn", [Value::Str(message)]) => format!("WARN {message}\n"),
        ("info" | "warn", [_]) => return Err(type_error("expected a string message", span)),
        ("error", [value]) => {
            let code = error_field(value, "code")
                .ok_or_else(|| type_error("`std::log::error` expects an Error", span))?;
            let message = error_field(value, "message").unwrap_or_default();
            format!("ERROR [{code}] {message}\n")
        }
        ("info" | "warn" | "error", _) => return Err(std_arity("log", op, span)),
        _ => {
            unreachable!("the stdlib table routes only `info`/`warn`/`error` to the log capability")
        }
    };
    env.guard_rollback_sensitive_host_effect(&format!("std::log::{op}"), span)?;
    sink.borrow_mut().push_str(&line);
    Ok(None)
}

pub(crate) fn eval_io(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let values: Vec<Value> = args
        .iter()
        .map(|arg| eval_expr(&arg.value, env))
        .collect::<Result<_, _>>()?;
    if !env.host.filesystem {
        return Err(RuntimeError::fault(
            RUN_CAPABILITY,
            format!("this run provides no filesystem capability for `std::io::{op}`"),
            span,
        ));
    }
    match (op, values.as_slice()) {
        ("readText", [Value::Str(path)]) => match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(Value::Str(text))),
            Err(error) => Err(raise(io_error("io.read", op, path, &error), span, None)),
        },
        ("writeText", [Value::Str(path), Value::Str(text)]) => {
            env.guard_rollback_sensitive_host_effect(&format!("std::io::{op}"), span)?;
            match std::fs::write(path, text) {
                Ok(()) => Ok(None),
                Err(error) => Err(raise(io_error("io.write", op, path, &error), span, None)),
            }
        }
        ("readBytes", [Value::Str(path)]) => match std::fs::read(path) {
            Ok(bytes) => Ok(Some(Value::Bytes(bytes))),
            Err(error) => Err(raise(io_error("io.read", op, path, &error), span, None)),
        },
        ("writeBytes", [Value::Str(path), Value::Bytes(data)]) => {
            env.guard_rollback_sensitive_host_effect(&format!("std::io::{op}"), span)?;
            match std::fs::write(path, data) {
                Ok(()) => Ok(None),
                Err(error) => Err(raise(io_error("io.write", op, path, &error), span, None)),
            }
        }
        ("readText" | "writeText" | "readBytes" | "writeBytes", _) => Err(type_error(
            &format!("`std::io::{op}` got the wrong arguments"),
            span,
        )),
        _ => unreachable!(
            "the stdlib table routes only readText/writeText/readBytes/writeBytes to the io capability"
        ),
    }
}
