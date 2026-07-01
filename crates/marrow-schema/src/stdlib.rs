//! The one descriptor table for the `std::<module>::<op>` helpers.
//!
//! Each row states a helper's positional parameter types, its result type, and
//! required host capability. The checker derives a std call's arity, argument,
//! and return types from these rows — a maybe-present op carries an optional
//! return type, resolved like any other `T?` — and the runtime derives which
//! recognized ops it must handle from the same table.

use crate::ScalarType;

/// A std helper's positional parameter, in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    /// A concrete storable scalar argument.
    Scalar(ScalarType),
    /// Any scalar argument; helper-specific checks may constrain relationships
    /// between multiple scalar-any parameters.
    ScalarAny,
    /// A `sequence[T]` of scalar values.
    Sequence(ScalarType),
    /// An `Error` value (`std::log::error`), the one checker-only argument type.
    Error,
    /// A path expression rather than a scalar (`std::assert::isAbsent`); the checker
    /// leaves it unchecked, as it does other path arguments.
    Path,
}

/// A std helper's result type. `Void` helpers (`std::log`, `std::assert`,
/// `std::io::write*`) yield no value, leaving the call's type to the surrounding
/// checks. An `OptionalScalar` op may have no result, so the call types as `T?`
/// and must be resolved like a maybe-present saved read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnType {
    Scalar(ScalarType),
    /// A maybe-present scalar result, typed `T?` at the call site.
    OptionalScalar(ScalarType),
    /// A `sequence[T]` of a scalar element (`std::text::split: sequence[string]`).
    Sequence(ScalarType),
    Void,
}

/// Host capabilities a std helper may require.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Clock,
    Context,
    Environment,
    Log,
    Filesystem,
}

/// One `std::<module>::<op>` descriptor.
#[derive(Debug)]
pub struct StdOp {
    pub module: &'static str,
    pub op: &'static str,
    pub params: &'static [ParamType],
    pub ret: ReturnType,
    pub requires_capability: Option<Capability>,
}

use Capability::{Clock, Context, Environment, Filesystem, Log};
use ParamType::{Error as ErrorArg, Path, Scalar, ScalarAny, Sequence as SequenceArg};
use ReturnType::{Sequence, Void};
use ScalarType::{Bool, Bytes, Date, Decimal, Duration, Instant, Int, Str};

/// Keeps the table terse enough to read as a flat signature list.
const fn row(
    module: &'static str,
    op: &'static str,
    params: &'static [ParamType],
    ret: ReturnType,
    requires_capability: Option<Capability>,
) -> StdOp {
    StdOp {
        module,
        op,
        params,
        ret,
        requires_capability,
    }
}

const fn scalar(scalar: ScalarType) -> ReturnType {
    ReturnType::Scalar(scalar)
}

/// A maybe-present scalar result: the op may have no value, so its call types as
/// `T?` and must be resolved.
const fn optional(scalar: ScalarType) -> ReturnType {
    ReturnType::OptionalScalar(scalar)
}

/// The descriptor table. Every enumerated std helper has exactly one row. Calls
/// under a known std module that are absent from this table are checker errors,
/// not runtime extension hooks.
#[rustfmt::skip]
const TABLE: &[StdOp] = &[
    row("text", "length", &[Scalar(Str)], scalar(Int), None),
    row("text", "trim", &[Scalar(Str)], scalar(Str), None),
    row("text", "contains", &[Scalar(Str), Scalar(Str)], scalar(Bool), None),
    row("text", "split", &[Scalar(Str), Scalar(Str)], Sequence(Str), None),
    row("text", "slice", &[Scalar(Str), Scalar(Int), Scalar(Int)], scalar(Str), None),
    row("text", "startsWith", &[Scalar(Str), Scalar(Str)], scalar(Bool), None),
    row("text", "endsWith", &[Scalar(Str), Scalar(Str)], scalar(Bool), None),
    row("text", "indexOf", &[Scalar(Str), Scalar(Str)], optional(Int), None),
    row("text", "replace", &[Scalar(Str), Scalar(Str), Scalar(Str)], scalar(Str), None),
    row("text", "join", &[SequenceArg(Str), Scalar(Str)], scalar(Str), None),
    row("text", "toUpper", &[Scalar(Str)], scalar(Str), None),
    row("text", "toLower", &[Scalar(Str)], scalar(Str), None),
    row("text", "urlEncode", &[Scalar(Str)], scalar(Str), None),
    row("text", "urlDecode", &[Scalar(Str)], scalar(Str), None),
    row("bytes", "length", &[Scalar(Bytes)], scalar(Int), None),
    row("bytes", "base64Encode", &[Scalar(Bytes)], scalar(Str), None),
    row("bytes", "base64Decode", &[Scalar(Str)], scalar(Bytes), None),
    row("bytes", "fromText", &[Scalar(Str)], scalar(Bytes), None),
    row("bytes", "toText", &[Scalar(Bytes)], scalar(Str), None),
    row("bytes", "hexEncode", &[Scalar(Bytes)], scalar(Str), None),
    row("bytes", "hexDecode", &[Scalar(Str)], scalar(Bytes), None),
    row("hash", "sha256", &[Scalar(Bytes)], scalar(Bytes), None),
    row("hash", "sha512", &[Scalar(Bytes)], scalar(Bytes), None),
    row("hash", "hmacSha256", &[Scalar(Bytes), Scalar(Bytes)], scalar(Bytes), None),
    row("math", "absInt", &[Scalar(Int)], scalar(Int), None),
    row("math", "absDecimal", &[Scalar(Decimal)], scalar(Decimal), None),
    row("math", "floor", &[Scalar(Decimal)], scalar(Int), None),
    row("math", "minInt", &[Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("math", "maxInt", &[Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("math", "minDecimal", &[Scalar(Decimal), Scalar(Decimal)], scalar(Decimal), None),
    row("math", "maxDecimal", &[Scalar(Decimal), Scalar(Decimal)], scalar(Decimal), None),
    row("math", "round", &[Scalar(Decimal)], scalar(Int), None),
    row("math", "roundDecimal", &[Scalar(Decimal), Scalar(Int)], scalar(Decimal), None),
    row("math", "ceiling", &[Scalar(Decimal)], scalar(Int), None),
    row("math", "powInt", &[Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("math", "modulo", &[Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("math", "remainder", &[Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("math", "quotient", &[Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("math", "divFloor", &[Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("math", "clampInt", &[Scalar(Int), Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("math", "clampDecimal", &[Scalar(Decimal), Scalar(Decimal), Scalar(Decimal)], scalar(Decimal), None),
    row("json", "valid", &[Scalar(Str)], scalar(Bool), None),
    row("json", "stringLit", &[Scalar(Str)], scalar(Str), None),
    row("json", "stringArray", &[SequenceArg(Str)], scalar(Str), None),
    row("json", "string", &[Scalar(Str), Scalar(Str)], optional(Str), None),
    row("json", "int", &[Scalar(Str), Scalar(Str)], optional(Int), None),
    row("json", "decimal", &[Scalar(Str), Scalar(Str)], optional(Decimal), None),
    row("json", "bool", &[Scalar(Str), Scalar(Str)], optional(Bool), None),
    row("json", "count", &[Scalar(Str), Scalar(Str)], optional(Int), None),
    row("csv", "row", &[SequenceArg(Str)], scalar(Str), None),
    row("csv", "rowCount", &[Scalar(Str)], scalar(Int), None),
    row("csv", "hasColumn", &[Scalar(Str), Scalar(Str)], scalar(Bool), None),
    row("csv", "string", &[Scalar(Str), Scalar(Int), Scalar(Str)], optional(Str), None),
    row("csv", "int", &[Scalar(Str), Scalar(Int), Scalar(Str)], optional(Int), None),
    row("csv", "decimal", &[Scalar(Str), Scalar(Int), Scalar(Str)], optional(Decimal), None),
    row("csv", "bool", &[Scalar(Str), Scalar(Int), Scalar(Str)], optional(Bool), None),
    row("id", "slug", &[Scalar(Str)], scalar(Str), None),
    row("id", "stableUuid", &[Scalar(Str)], scalar(Str), None),
    row("random", "int", &[Scalar(Str), Scalar(Int), Scalar(Int), Scalar(Int)], scalar(Int), None),
    row("random", "bool", &[Scalar(Str), Scalar(Int)], scalar(Bool), None),
    row("random", "decimal", &[Scalar(Str), Scalar(Int)], scalar(Decimal), None),
    row("context", "actor", &[], optional(Str), Some(Context)),
    row("context", "requestId", &[], optional(Str), Some(Context)),
    row("context", "idempotencyKey", &[], optional(Str), Some(Context)),
    row("audit", "event", &[Scalar(Str), Scalar(Str), Scalar(Str)], scalar(Str), None),
    row("audit", "change", &[Scalar(Str), Scalar(Str), Scalar(Str)], scalar(Str), None),
    row("error", "code", &[ErrorArg], scalar(Str), None),
    row("error", "message", &[ErrorArg], scalar(Str), None),
    row("error", "hasCode", &[ErrorArg, Scalar(Str)], scalar(Bool), None),
    row("matrix", "parse", &[Scalar(Str)], scalar(Str), None),
    row("matrix", "identity", &[Scalar(Int)], scalar(Str), None),
    row("matrix", "rows", &[Scalar(Str)], scalar(Int), None),
    row("matrix", "cols", &[Scalar(Str)], scalar(Int), None),
    row("matrix", "get", &[Scalar(Str), Scalar(Int), Scalar(Int)], scalar(Decimal), None),
    row("matrix", "add", &[Scalar(Str), Scalar(Str)], scalar(Str), None),
    row("matrix", "multiply", &[Scalar(Str), Scalar(Str)], scalar(Str), None),
    row("matrix", "transpose", &[Scalar(Str)], scalar(Str), None),
    row("clock", "now", &[], scalar(Instant), Some(Clock)),
    row("clock", "today", &[], scalar(Date), Some(Clock)),
    row("clock", "parseInstant", &[Scalar(Str)], scalar(Instant), None),
    row("clock", "parseDate", &[Scalar(Str)], scalar(Date), None),
    row("clock", "parseDuration", &[Scalar(Str)], scalar(Duration), None),
    row("clock", "formatInstant", &[Scalar(Instant)], scalar(Str), None),
    row("clock", "formatDate", &[Scalar(Date)], scalar(Str), None),
    row("clock", "formatDuration", &[Scalar(Duration)], scalar(Str), None),
    row("clock", "addDays", &[Scalar(Date), Scalar(Int)], scalar(Date), None),
    row("clock", "daysBetween", &[Scalar(Date), Scalar(Date)], scalar(Int), None),
    row("clock", "year", &[Scalar(Date)], scalar(Int), None),
    row("clock", "month", &[Scalar(Date)], scalar(Int), None),
    row("clock", "day", &[Scalar(Date)], scalar(Int), None),
    row("env", "exists", &[Scalar(Str)], scalar(Bool), Some(Environment)),
    row("env", "get", &[Scalar(Str), Scalar(Str)], scalar(Str), Some(Environment)),
    row("env", "require", &[Scalar(Str)], scalar(Str), Some(Environment)),
    row("io", "readText", &[Scalar(Str)], scalar(Str), Some(Filesystem)),
    row("io", "readBytes", &[Scalar(Str)], scalar(Bytes), Some(Filesystem)),
    row("io", "writeText", &[Scalar(Str), Scalar(Str)], Void, Some(Filesystem)),
    row("io", "writeBytes", &[Scalar(Str), Scalar(Bytes)], Void, Some(Filesystem)),
    row("assert", "isTrue", &[Scalar(Bool)], Void, None),
    row("assert", "isFalse", &[Scalar(Bool)], Void, None),
    row("assert", "equal", &[ScalarAny, ScalarAny], Void, None),
    row("assert", "isAbsent", &[Path], Void, None),
    row("assert", "fail", &[Scalar(Str)], Void, None),
    row("log", "info", &[Scalar(Str)], Void, Some(Log)),
    row("log", "warn", &[Scalar(Str)], Void, Some(Log)),
    row("log", "error", &[ErrorArg], Void, Some(Log)),
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
