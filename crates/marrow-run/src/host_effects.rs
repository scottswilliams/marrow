//! Runtime handlers for host-backed stdlib capabilities.

use marrow_check::CheckedArg as ExecArg;
use marrow_store::value::NANOS_PER_DAY;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{
    RUN_ABSENT, RUN_CAPABILITY, RuntimeError, error_field, io_error, raise, raise_fault, std_arity,
    type_error,
};
use crate::expr::eval_expr;
use crate::stdlib::eval_text;
use crate::value::Value;

fn no_capability(capability: &str, module: &str, op: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError::fault(
        RUN_CAPABILITY,
        format!("this run provides no {capability} capability for `std::{module}::{op}`"),
        span,
    )
}

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
    let nanos = env
        .host
        .clock
        .ok_or_else(|| no_capability("clock", "clock", op, span))?;
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
    let variables = env
        .host
        .environment
        .as_ref()
        .ok_or_else(|| no_capability("environment", "env", op, span))?;
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

pub(crate) fn eval_context(
    op: &str,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if !args.is_empty() {
        return Err(std_arity("context", op, span));
    }
    let context = env
        .host
        .context
        .as_ref()
        .ok_or_else(|| no_capability("context", "context", op, span))?;
    let value = match op {
        "actor" => context.actor(),
        "requestId" => context.request_id(),
        "idempotencyKey" => context.idempotency_key(),
        _ => unreachable!("the stdlib table routes only context ops to eval_context"),
    };
    value
        .map(|value| Value::Str(value.to_string()))
        .ok_or_else(|| raise_fault(RUN_ABSENT, format!("context `{op}` is absent"), span))
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
    let sink = env
        .host
        .log
        .as_ref()
        .ok_or_else(|| no_capability("log", "log", op, span))?;
    let line = match (op, values.as_slice()) {
        ("info", [Value::Str(message)]) => format!("INFO {message}\n"),
        ("warn", [Value::Str(message)]) => format!("WARN {message}\n"),
        ("info" | "warn", [_]) => return Err(type_error("expected a string message", span)),
        ("error", [value]) => {
            let code = error_field(value, marrow_schema::error::CODE)
                .ok_or_else(|| type_error("`std::log::error` expects an Error", span))?;
            let message = error_field(value, marrow_schema::error::MESSAGE).unwrap_or_default();
            format!("ERROR [{code}] {message}\n")
        }
        ("info" | "warn" | "error", _) => return Err(std_arity("log", op, span)),
        _ => {
            unreachable!("the stdlib table routes only `info`/`warn`/`error` to the log capability")
        }
    };
    env.guard_rollback_sensitive_host_effect(&format!("std::log::{op}"), span)?;
    sink.borrow_mut().write_log(&line);
    Ok(None)
}

/// A resolved filesystem op carrying its write payload, so direction (the
/// catchable error code and whether it is a rollback-sensitive effect) and the
/// fs call both derive from one value. A write borrows its bytes from the
/// evaluated argument.
enum IoOp<'a> {
    ReadText,
    ReadBytes,
    Write(&'a [u8]),
}

impl IoOp<'_> {
    fn run(self, path: &str) -> std::io::Result<Option<Value>> {
        match self {
            IoOp::ReadText => std::fs::read_to_string(path).map(|text| Some(Value::Str(text))),
            IoOp::ReadBytes => std::fs::read(path).map(|bytes| Some(Value::Bytes(bytes))),
            IoOp::Write(data) => std::fs::write(path, data).map(|()| None),
        }
    }
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
        return Err(no_capability("filesystem", "io", op, span));
    }
    let (path, io) = match (op, values.as_slice()) {
        ("readText", [Value::Str(path)]) => (path, IoOp::ReadText),
        ("readBytes", [Value::Str(path)]) => (path, IoOp::ReadBytes),
        ("writeText", [Value::Str(path), Value::Str(text)]) => (path, IoOp::Write(text.as_bytes())),
        ("writeBytes", [Value::Str(path), Value::Bytes(data)]) => (path, IoOp::Write(data)),
        ("readText" | "readBytes" | "writeText" | "writeBytes", _) => {
            return Err(type_error(
                &format!("`std::io::{op}` got the wrong arguments"),
                span,
            ));
        }
        _ => unreachable!("the stdlib table routes only the four io ops to eval_io"),
    };
    // A read failure and a write failure are distinct, catchable categories; a
    // write is also a rollback-sensitive effect, rejected inside an open
    // transaction before touching the filesystem.
    let error_code = if matches!(io, IoOp::Write(_)) {
        env.guard_rollback_sensitive_host_effect(&format!("std::io::{op}"), span)?;
        "io.write"
    } else {
        "io.read"
    };
    io.run(path)
        .map_err(|error| raise(io_error(error_code, op, path, &error), span, None))
}
