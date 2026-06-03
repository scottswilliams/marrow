//! Runtime faults: `RuntimeError`, the `run.*` codes, and the fault constructors.

use marrow_check::{CheckedRuntimeModule, CheckedRuntimeProgram, FileId};
use marrow_store::StoreError;
use marrow_store::value::{ScalarType, ValueError};
use marrow_syntax::SourceSpan;

use crate::env::AssignError;
use crate::value::Value;
use crate::write::WriteError;

/// A runtime fault: a stable `run.*` code, a human-readable message, and the
/// source span of the construct that raised it.
///
/// When `throw` is `Some`, the fault is a catchable unwinding throw carrying its
/// `Error` value: a `throw` from a called function, a deterministic evaluator
/// fault, or a recoverable `write.*` fault that a surrounding `try`/`catch` can
/// bind. The value rides this `Err` channel directly, so an expression-position
/// throw (from a call or builtin) and a statement-position throw (`Flow::Throw`)
/// agree through one mechanism with no out-of-band carrier. When `throw` is
/// `None` the fault is fatal and uncatchable (unknown functions, store
/// corruption, unsupported runtime constructs, host capability failures, …).
/// `code`/`message` always describe how the fault renders if it escapes uncaught.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: &'static str,
    pub message: String,
    pub span: SourceSpan,
    /// The `Error` value a catchable fault carries so a surrounding `try`/`catch`
    /// can bind it; `None` for a fatal fault. Boxed because it is set on only a
    /// few faults yet would otherwise dominate this error's size on every cold
    /// `Err` the runtime threads through its `Result`s.
    pub throw: Option<Box<Value>>,
    /// The file the fault was raised in, as a [`FileId`] into the running
    /// [`CheckedRuntimeProgram`]. The `span`'s byte offsets are per-file, so this
    /// supplies the file identity they lack. It is `None` until the fault leaves
    /// the activation that raised it and stays `None` for activations without
    /// module context.
    pub origin: Option<FileId>,
}

impl RuntimeError {
    /// A fatal, uncatchable runtime fault.
    pub(crate) fn fault(code: &'static str, message: String, span: SourceSpan) -> Self {
        RuntimeError {
            code,
            message,
            span,
            throw: None,
            origin: None,
        }
    }

    /// Stamp `module`'s file id as this fault's origin, but only if it has none
    /// yet, so the deepest frame — the one that actually raised the fault — wins
    /// as the fault unwinds through outer frames. With no module or an
    /// unrecognized one, the origin is left as it was.
    pub(crate) fn with_origin_from(
        mut self,
        program: &CheckedRuntimeProgram,
        module: Option<&CheckedRuntimeModule>,
    ) -> Self {
        if self.origin.is_none() {
            self.origin = module.and_then(|module| program.file_id_of(module));
        }
        self
    }
}

impl marrow_syntax::Diagnose for RuntimeError {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
}

/// A value was used where another type was required (e.g. `+` on a non-integer,
/// a non-boolean condition, or assigning to an immutable binding).
pub const RUN_TYPE: &str = "run.type";

/// A name was read or assigned that is not bound in scope.
pub const RUN_UNBOUND_NAME: &str = "run.unbound_name";

/// Integer arithmetic overflowed the 64-bit range.
pub const RUN_OVERFLOW: &str = "run.overflow";

/// Decimal arithmetic exceeded the 34-digit / 34-place decimal envelope.
pub const RUN_DECIMAL_OVERFLOW: &str = "run.decimal_overflow";

/// Integer division or remainder by zero.
pub const RUN_DIVIDE_BY_ZERO: &str = "run.divide_by_zero";

/// A `break` or `continue` reached the top of a function with no loop to target.
pub const RUN_NO_ENCLOSING_LOOP: &str = "run.no_enclosing_loop";

/// A call named a function the program does not declare.
pub const RUN_UNKNOWN_FUNCTION: &str = "run.unknown_function";

/// An entry name matched more than one public function and must be qualified.
pub const RUN_AMBIGUOUS_FUNCTION: &str = "run.ambiguous_function";

/// A qualified call named a function that exists but is not `pub`, so it is not
/// callable from the calling module. The checker (`check.private_function`)
/// catches this before a run; this is the runtime backstop.
pub const RUN_PRIVATE_FUNCTION: &str = "run.private_function";

/// A call to a function that returns no value was used where a value is needed.
pub const RUN_NO_VALUE: &str = "run.no_value";

/// A direct read of a saved element that is absent (unpopulated).
pub const RUN_ABSENT: &str = "run.absent_element";

/// The store reported an error (e.g. a corrupt stored path) during a read.
pub const RUN_STORE: &str = "run.store";

/// A construct the runtime does not evaluate.
pub const RUN_UNSUPPORTED: &str = "run.unsupported";

/// A host capability a builtin needs (e.g. the clock for `std::clock::now`) was
/// not provided to this run.
pub const RUN_CAPABILITY: &str = "run.capability";

/// A `std::assert::*` testing assertion did not hold. `marrow test` reports these
/// as located test failures.
pub const RUN_ASSERT: &str = "run.assertion";

/// An `Error` raised by `throw` reached the top of a function with no `catch` to
/// handle it. The fault message carries the error's own code and message.
pub const RUN_UNCAUGHT_THROW: &str = "run.uncaught_error";

/// A write, delete, or append changed the saved layer a loop was actively
/// traversing. The static rule `check.loop_mutates_traversed_layer` catches the
/// obvious cases; this is the dynamic guard for a path the checker cannot prove.
pub const RUN_TRAVERSAL: &str = "run.traversal";

/// Raise `error` as a catchable language throw on the `Err` channel: the value
/// rides the [`RuntimeError`]'s `throw` field, so a surrounding `try`/`catch`
/// binds it. With no surrounding handler, the activation re-surfaces it. The
/// `code`/`message` carry how it renders if it escapes uncaught: the sentinel
/// `run.uncaught_error` and `uncaught error [{code}]: {message}` from the
/// `Error`'s own fields. Assumes a well-formed Error with string `code` and
/// `message`; a malformed one renders blank, which the constructor and the throw
/// guard make unreachable in practice.
pub(crate) fn raise(error: Value, span: SourceSpan, origin: Option<FileId>) -> RuntimeError {
    let code = error_field(&error, "code").unwrap_or_default();
    let message = error_field(&error, "message").unwrap_or_default();
    RuntimeError {
        code: RUN_UNCAUGHT_THROW,
        message: format!("uncaught error [{code}]: {message}"),
        span,
        throw: Some(Box::new(error)),
        // The completion carries the file the throw was first raised in; this
        // caller-frame re-span keeps it rather than re-deriving a shallower one.
        origin,
    }
}

/// A `run.unknown_function` fault for a call/entry that resolves to no function.
pub(crate) fn unknown_function(name: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        throw: None,
        origin: None,
        code: RUN_UNKNOWN_FUNCTION,
        message: format!("the program has no function `{name}`"),
        span,
    }
}

/// A `run.ambiguous_function` fault for a bare entry name that needs a module.
pub(crate) fn ambiguous_function(name: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        throw: None,
        origin: None,
        code: RUN_AMBIGUOUS_FUNCTION,
        message: format!("entry `{name}` is ambiguous; qualify it as `module::{name}`"),
        span,
    }
}

/// A `run.private_function` fault for an entry that names a private function.
pub(crate) fn private_function(name: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        throw: None,
        origin: None,
        code: RUN_PRIVATE_FUNCTION,
        message: format!("function `{name}` is private to its module"),
        span,
    }
}

/// Map an [`AssignError`] from a failed reassignment to a runtime fault.
pub(crate) fn assign_error(name: &str, error: AssignError, span: SourceSpan) -> RuntimeError {
    match error {
        AssignError::Immutable => RuntimeError::fault(
            RUN_TYPE,
            format!("cannot assign to immutable `{name}`"),
            span,
        ),
        AssignError::Unbound => {
            RuntimeError::fault(RUN_UNBOUND_NAME, format!("`{name}` is not bound"), span)
        }
    }
}

/// Re-raise a recoverable fault that escaped a called function so the caller's
/// `try` can bind it: the `Error` value rides the `throw` field while the
/// [`RuntimeError`] keeps the fault's own dotted `code`, so an uncaught one
/// surfaces unchanged (mirrors [`raise`] for language throws).
pub(crate) fn reraise_fault(
    error: Value,
    code: &'static str,
    span: SourceSpan,
    origin: Option<FileId>,
) -> RuntimeError {
    RuntimeError {
        code,
        message: error_field(&error, "message").unwrap_or_default(),
        span,
        throw: Some(Box::new(error)),
        // Kept from the completion so the file the fault was raised in survives
        // this caller-frame re-span.
        origin,
    }
}

/// Raise a recoverable runtime fault (a managed-write failure or an absent-element
/// read) as a catchable Error while keeping its dotted code. Like [`raise`], the
/// `Error` value carrying `code`/`message` rides the `throw` field so an enclosing
/// `try`/`catch` can bind it. Unlike [`raise`], the returned [`RuntimeError`] keeps
/// the fault's own dotted code rather than the `RUN_UNCAUGHT_THROW` sentinel, so an
/// uncaught fault surfaces with the same code it did before it became catchable.
pub(crate) fn raise_fault(code: &'static str, message: String, span: SourceSpan) -> RuntimeError {
    let error = Value::Resource(vec![
        ("code".to_string(), Value::Str(code.to_string())),
        ("message".to_string(), Value::Str(message.clone())),
    ]);
    RuntimeError {
        code,
        message,
        span,
        throw: Some(Box::new(error)),
        origin: None,
    }
}

/// The type error for a value that cannot be converted to `name`.
pub(crate) fn conversion_error(name: &str, span: SourceSpan) -> RuntimeError {
    type_error(&format!("cannot convert this value to {name}"), span)
}

/// The wrong-argument-count error for a `std::*` helper.
pub(crate) fn std_arity(module: &str, op: &str, span: SourceSpan) -> RuntimeError {
    type_error(
        &format!("`std::{module}::{op}` got the wrong number of arguments"),
        span,
    )
}

/// Build a catchable `Error` value (code + message) for a failed `std::io` call.
pub(crate) fn io_error(code: &str, op: &str, path: &str, error: &std::io::Error) -> Value {
    Value::Resource(vec![
        ("code".to_string(), Value::Str(code.to_string())),
        (
            "message".to_string(),
            Value::Str(format!("std::io::{op} failed for `{path}`: {error}")),
        ),
    ])
}

/// The string value of an `Error` resource's named field (`code`/`message`), or
/// `None` if the value is not an Error-shaped resource carrying that string
/// field. Shared by uncaught-throw reporting and `std::log::error`.
pub(crate) fn error_field(value: &Value, name: &str) -> Option<String> {
    match value {
        Value::Resource(fields) => fields.iter().find_map(|(field, value)| match value {
            Value::Str(text) if field == name => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    }
}

/// A runtime fault for a key whose scalar kind does not match the declared key
/// type. The keyspace is typed, so writing a wrong-typed key would corrupt it;
/// this stops the write before it reaches the store.
pub(crate) fn key_type_fault(
    expected: ScalarType,
    found: ScalarType,
    span: SourceSpan,
) -> RuntimeError {
    RuntimeError {
        throw: None,
        origin: None,
        code: RUN_TYPE,
        message: format!(
            "a key of type `{}` was given where `{}` is declared",
            found.name(),
            expected.name()
        ),
        span,
    }
}

/// A store or codec error met at a known source construct becomes a
/// [`RuntimeError`] anchored to that span, so a backend or value-range failure
/// reports at the path expression that triggered it.
pub(crate) trait Located {
    fn located(self, span: SourceSpan) -> RuntimeError;
}

impl Located for StoreError {
    fn located(self, span: SourceSpan) -> RuntimeError {
        RuntimeError::fault(
            RUN_STORE,
            format!("a saved-data operation failed: {self}"),
            span,
        )
    }
}

/// A value-encoding range error (e.g. a date/instant outside year 0001-9999)
/// keeps the codec's stable dotted code.
impl Located for ValueError {
    fn located(self, span: SourceSpan) -> RuntimeError {
        raise_fault(self.code(), self.to_string(), span)
    }
}

/// Surface a managed-write planning failure (a `WriteError`) as a
/// catchable fault: a rejected managed write — a unique conflict,
/// a missing required field, a type or identity mismatch, a value-range error, or
/// a store read error met while planning — is recoverable, so a `try`/`catch`
/// can bind it and a transaction can continue or roll back. The fault keeps the
/// `write.*` (or value-codec) code so an uncaught one surfaces unchanged. Call
/// after dropping any `env.store` borrow held while planning.
pub(crate) fn write_fault(error: WriteError, span: SourceSpan) -> RuntimeError {
    raise_fault(error.code, error.message, span)
}

pub(crate) fn unsupported(what: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError::fault(
        RUN_UNSUPPORTED,
        format!("the runtime does not evaluate {what}"),
        span,
    )
}

pub(crate) fn type_error(message: &str, span: SourceSpan) -> RuntimeError {
    raise_fault(RUN_TYPE, message.to_string(), span)
}

pub(crate) fn overflow(span: SourceSpan) -> RuntimeError {
    raise_fault(RUN_OVERFLOW, "integer arithmetic overflowed".into(), span)
}

pub(crate) fn decimal_overflow(span: SourceSpan) -> RuntimeError {
    raise_fault(
        RUN_DECIMAL_OVERFLOW,
        "decimal arithmetic exceeded the 34-digit / 34-place envelope".into(),
        span,
    )
}

pub(crate) fn divide_by_zero(message: &str, span: SourceSpan) -> RuntimeError {
    raise_fault(RUN_DIVIDE_BY_ZERO, message.to_string(), span)
}
