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
    fn is_write(&self) -> bool {
        matches!(self, IoOp::Write(_))
    }

    /// The catchable error code a failed call raises: a read failure and a write
    /// failure are distinct, catchable categories.
    fn error_code(&self) -> &'static str {
        if self.is_write() {
            "io.write"
        } else {
            "io.read"
        }
    }

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
        return Err(RuntimeError::fault(
            RUN_CAPABILITY,
            format!("this run provides no filesystem capability for `std::io::{op}`"),
            span,
        ));
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
    // A write is a rollback-sensitive effect; reject it inside an open
    // transaction before touching the filesystem.
    if io.is_write() {
        env.guard_rollback_sensitive_host_effect(&format!("std::io::{op}"), span)?;
    }
    let error_code = io.error_code();
    io.run(path)
        .map_err(|error| raise(io_error(error_code, op, path, &error), span, None))
}
