//! The one descriptor table for the `std::<module>::<op>` helpers.
//!
//! Each row states a helper's positional parameter types, its result type, and
//! which runtime capability family evaluates it. The checker derives a std call's
//! arity, argument, and return checks from these rows; the runtime derives which
//! handler a recognized op routes to. A new std helper is one row here, not a
//! parallel entry in the checker's signature tables and the runtime's dispatch.

use crate::ScalarType;

/// A std helper's positional parameter, in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    /// A concrete storable scalar argument.
    Scalar(ScalarType),
    /// An `Error` value (`std::log::error`), the one checker-only argument type.
    Error,
    /// A path expression rather than a scalar (`std::assert::absent`); the checker
    /// leaves it unchecked, as it does other path arguments.
    Path,
}

/// A std helper's result type. `Void` helpers (`std::log`, `std::assert`,
/// `std::io::write*`) yield no value, leaving the call's type to the surrounding
/// checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnType {
    Scalar(ScalarType),
    /// A `sequence[T]` of a scalar element (`std::text::split: sequence[string]`).
    Sequence(ScalarType),
    Void,
}

/// The runtime handler family a recognized op routes to. The capability families
/// each read a host capability (clock/env/log/filesystem) or raise an assertion;
/// `Pure` helpers compute in place and need no capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Pure,
    Clock,
    Env,
    Log,
    Io,
    Assert,
}

/// One `std::<module>::<op>` descriptor.
#[derive(Debug)]
pub struct StdOp {
    pub module: &'static str,
    pub op: &'static str,
    pub params: &'static [ParamType],
    pub ret: ReturnType,
    pub capability: Capability,
}

use Capability::{Assert, Clock, Env, Io, Log, Pure};
use ParamType::{Error as ErrorArg, Path, Scalar};
use ReturnType::{Sequence, Void};
use ScalarType::{Bool, Bytes, Date, Decimal, Duration, Instant, Int, Str};

/// Keeps the table terse enough to read as a flat signature list.
const fn row(
    module: &'static str,
    op: &'static str,
    params: &'static [ParamType],
    ret: ReturnType,
    capability: Capability,
) -> StdOp {
    StdOp {
        module,
        op,
        params,
        ret,
        capability,
    }
}

const fn scalar(scalar: ScalarType) -> ReturnType {
    ReturnType::Scalar(scalar)
}

/// The descriptor table. Every enumerated std helper has exactly one row. Calls
/// under a known std module that are absent from this table are checker errors,
/// not runtime extension hooks.
#[rustfmt::skip]
const TABLE: &[StdOp] = &[
    row("text", "length", &[Scalar(Str)], scalar(Int), Pure),
    row("text", "trim", &[Scalar(Str)], scalar(Str), Pure),
    row("text", "contains", &[Scalar(Str), Scalar(Str)], scalar(Bool), Pure),
    row("text", "split", &[Scalar(Str), Scalar(Str)], Sequence(Str), Pure),
    row("bytes", "length", &[Scalar(Bytes)], scalar(Int), Pure),
    row("bytes", "base64Encode", &[Scalar(Bytes)], scalar(Str), Pure),
    row("bytes", "base64Decode", &[Scalar(Str)], scalar(Bytes), Pure),
    row("math", "absInt", &[Scalar(Int)], scalar(Int), Pure),
    row("math", "absDecimal", &[Scalar(Decimal)], scalar(Decimal), Pure),
    row("math", "floor", &[Scalar(Decimal)], scalar(Int), Pure),
    row("math", "modulo", &[Scalar(Int), Scalar(Int)], scalar(Int), Pure),
    row("math", "remainder", &[Scalar(Int), Scalar(Int)], scalar(Int), Pure),
    row("clock", "now", &[], scalar(Instant), Clock),
    row("clock", "today", &[], scalar(Date), Clock),
    row("clock", "parseInstant", &[Scalar(Str)], scalar(Instant), Pure),
    row("clock", "parseDate", &[Scalar(Str)], scalar(Date), Pure),
    row("clock", "parseDuration", &[Scalar(Str)], scalar(Duration), Pure),
    row("clock", "formatInstant", &[Scalar(Instant)], scalar(Str), Pure),
    row("clock", "formatDate", &[Scalar(Date)], scalar(Str), Pure),
    row("clock", "formatDuration", &[Scalar(Duration)], scalar(Str), Pure),
    row("env", "exists", &[Scalar(Str)], scalar(Bool), Env),
    row("env", "get", &[Scalar(Str), Scalar(Str)], scalar(Str), Env),
    row("env", "require", &[Scalar(Str)], scalar(Str), Env),
    row("io", "readText", &[Scalar(Str)], scalar(Str), Io),
    row("io", "readBytes", &[Scalar(Str)], scalar(Bytes), Io),
    row("io", "writeText", &[Scalar(Str), Scalar(Str)], Void, Io),
    row("io", "writeBytes", &[Scalar(Str), Scalar(Bytes)], Void, Io),
    row("assert", "isTrue", &[Scalar(Bool)], Void, Assert),
    row("assert", "isFalse", &[Scalar(Bool)], Void, Assert),
    row("assert", "absent", &[Path], Void, Assert),
    row("assert", "fail", &[Scalar(Str)], Void, Assert),
    row("log", "info", &[Scalar(Str)], Void, Log),
    row("log", "warn", &[Scalar(Str)], Void, Log),
    row("log", "error", &[ErrorArg], Void, Log),
];

/// The descriptor for `std::<module>::<op>`, or `None` for an unrecognized op.
pub fn lookup(module: &str, op: &str) -> Option<&'static StdOp> {
    TABLE
        .iter()
        .find(|entry| entry.module == module && entry.op == op)
}

/// Every descriptor in declaration order. Editor tooling enumerates the table to
/// offer `std::<module>::` completions; the checker and runtime still reach a
/// single op through [`lookup`].
pub fn all() -> &'static [StdOp] {
    TABLE
}
