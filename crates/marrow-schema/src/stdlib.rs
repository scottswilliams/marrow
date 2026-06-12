//! The one descriptor table for the `std::<module>::<op>` helpers.
//!
//! Each row states a helper's positional parameter types, its result type, result
//! presence, and required host capability. The checker derives a std call's
//! arity, argument, return, and maybe-present checks from these rows; the runtime
//! derives which recognized ops it must handle from the same table.

use crate::ScalarType;

/// A std helper's positional parameter, in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    /// A concrete storable scalar argument.
    Scalar(ScalarType),
    /// A `sequence[T]` of scalar values.
    Sequence(ScalarType),
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

/// Whether a value-returning helper always yields a value or can be absent at the
/// read site. Maybe-present results must be resolved with the same language forms
/// as maybe-present saved reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnPresence {
    Always,
    MaybePresent,
}

/// Host capabilities a std helper may require.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Clock,
    Environment,
    Log,
    Filesystem,
    Maintenance,
}

/// One `std::<module>::<op>` descriptor.
#[derive(Debug)]
pub struct StdOp {
    pub module: &'static str,
    pub op: &'static str,
    pub params: &'static [ParamType],
    pub ret: ReturnType,
    pub presence: ReturnPresence,
    pub requires_capability: Option<Capability>,
}

use Capability::{Clock, Environment, Filesystem, Log};
use ParamType::{Error as ErrorArg, Path, Scalar, Sequence as SequenceArg};
use ReturnPresence::{Always, MaybePresent};
use ReturnType::{Sequence, Void};
use ScalarType::{Bool, Bytes, Date, Decimal, Duration, Instant, Int, Str};

/// Keeps the table terse enough to read as a flat signature list.
const fn row(
    module: &'static str,
    op: &'static str,
    params: &'static [ParamType],
    ret: ReturnType,
    presence: ReturnPresence,
    requires_capability: Option<Capability>,
) -> StdOp {
    StdOp {
        module,
        op,
        params,
        ret,
        presence,
        requires_capability,
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
    row("text", "length", &[Scalar(Str)], scalar(Int), Always, None),
    row("text", "trim", &[Scalar(Str)], scalar(Str), Always, None),
    row("text", "contains", &[Scalar(Str), Scalar(Str)], scalar(Bool), Always, None),
    row("text", "split", &[Scalar(Str), Scalar(Str)], Sequence(Str), Always, None),
    row("text", "slice", &[Scalar(Str), Scalar(Int), Scalar(Int)], scalar(Str), Always, None),
    row("text", "startsWith", &[Scalar(Str), Scalar(Str)], scalar(Bool), Always, None),
    row("text", "endsWith", &[Scalar(Str), Scalar(Str)], scalar(Bool), Always, None),
    row("text", "indexOf", &[Scalar(Str), Scalar(Str)], scalar(Int), MaybePresent, None),
    row("text", "replace", &[Scalar(Str), Scalar(Str), Scalar(Str)], scalar(Str), Always, None),
    row("text", "join", &[SequenceArg(Str), Scalar(Str)], scalar(Str), Always, None),
    row("text", "toUpper", &[Scalar(Str)], scalar(Str), Always, None),
    row("text", "toLower", &[Scalar(Str)], scalar(Str), Always, None),
    row("bytes", "length", &[Scalar(Bytes)], scalar(Int), Always, None),
    row("bytes", "base64Encode", &[Scalar(Bytes)], scalar(Str), Always, None),
    row("bytes", "base64Decode", &[Scalar(Str)], scalar(Bytes), Always, None),
    row("math", "absInt", &[Scalar(Int)], scalar(Int), Always, None),
    row("math", "absDecimal", &[Scalar(Decimal)], scalar(Decimal), Always, None),
    row("math", "floor", &[Scalar(Decimal)], scalar(Int), Always, None),
    row("math", "minInt", &[Scalar(Int), Scalar(Int)], scalar(Int), Always, None),
    row("math", "maxInt", &[Scalar(Int), Scalar(Int)], scalar(Int), Always, None),
    row("math", "minDecimal", &[Scalar(Decimal), Scalar(Decimal)], scalar(Decimal), Always, None),
    row("math", "maxDecimal", &[Scalar(Decimal), Scalar(Decimal)], scalar(Decimal), Always, None),
    row("math", "round", &[Scalar(Decimal)], scalar(Int), Always, None),
    row("math", "ceiling", &[Scalar(Decimal)], scalar(Int), Always, None),
    row("math", "powInt", &[Scalar(Int), Scalar(Int)], scalar(Int), Always, None),
    row("math", "modulo", &[Scalar(Int), Scalar(Int)], scalar(Int), Always, None),
    row("math", "remainder", &[Scalar(Int), Scalar(Int)], scalar(Int), Always, None),
    row("clock", "now", &[], scalar(Instant), Always, Some(Clock)),
    row("clock", "today", &[], scalar(Date), Always, Some(Clock)),
    row("clock", "parseInstant", &[Scalar(Str)], scalar(Instant), Always, None),
    row("clock", "parseDate", &[Scalar(Str)], scalar(Date), Always, None),
    row("clock", "parseDuration", &[Scalar(Str)], scalar(Duration), Always, None),
    row("clock", "formatInstant", &[Scalar(Instant)], scalar(Str), Always, None),
    row("clock", "formatDate", &[Scalar(Date)], scalar(Str), Always, None),
    row("clock", "formatDuration", &[Scalar(Duration)], scalar(Str), Always, None),
    row("env", "exists", &[Scalar(Str)], scalar(Bool), Always, Some(Environment)),
    row("env", "get", &[Scalar(Str), Scalar(Str)], scalar(Str), Always, Some(Environment)),
    row("env", "require", &[Scalar(Str)], scalar(Str), Always, Some(Environment)),
    row("io", "readText", &[Scalar(Str)], scalar(Str), Always, Some(Filesystem)),
    row("io", "readBytes", &[Scalar(Str)], scalar(Bytes), Always, Some(Filesystem)),
    row("io", "writeText", &[Scalar(Str), Scalar(Str)], Void, Always, Some(Filesystem)),
    row("io", "writeBytes", &[Scalar(Str), Scalar(Bytes)], Void, Always, Some(Filesystem)),
    row("assert", "isTrue", &[Scalar(Bool)], Void, Always, None),
    row("assert", "isFalse", &[Scalar(Bool)], Void, Always, None),
    row("assert", "absent", &[Path], Void, Always, None),
    row("assert", "fail", &[Scalar(Str)], Void, Always, None),
    row("log", "info", &[Scalar(Str)], Void, Always, Some(Log)),
    row("log", "warn", &[Scalar(Str)], Void, Always, Some(Log)),
    row("log", "error", &[ErrorArg], Void, Always, Some(Log)),
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
