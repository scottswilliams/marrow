//! Runtime faults: `RuntimeError`, the `run.*` codes, and the fault constructors.

#[cfg(test)]
use std::cell::Cell;

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
/// When `catchable` is true, a surrounding `try`/`catch` can bind an `Error`
/// value for this fault. Language throws and host-generated Error values may
/// already carry that value in `throw`; deterministic runtime faults usually
/// keep only `code`/`message` until a catch site actually binds them. When
/// `catchable` is false the fault is fatal and uncatchable (unknown functions,
/// store corruption, unsupported runtime constructs, host capability failures,
/// ...). `code`/`message` always describe how the fault renders if it escapes
/// uncaught.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: &'static str,
    pub message: String,
    pub span: SourceSpan,
    /// An already-materialized `Error` value. Runtime faults raised by
    /// [`raise_fault`] leave this empty until a catch site materializes it from
    /// `code`/`message`.
    pub throw: Option<Box<Value>>,
    /// Whether a `catch` can bind this fault as an `Error`.
    pub catchable: bool,
    /// True only while the fault is the specific outcome escaping an inner
    /// transaction toward the outermost transaction boundary.
    pub transaction_escape: bool,
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
            catchable: false,
            transaction_escape: false,
            origin: None,
        }
    }

    pub fn entry_surface(message: impl Into<String>) -> Self {
        RuntimeError::fault(RUN_ENTRY_SURFACE, message.into(), SourceSpan::default())
    }

    /// Whether this fault can be handled by a language `catch`.
    pub fn is_catchable(&self) -> bool {
        self.catchable
    }

    pub(crate) fn is_transaction_escape(&self) -> bool {
        self.transaction_escape
    }

    pub(crate) fn with_transaction_escape(mut self, transaction_escape: bool) -> Self {
        self.transaction_escape = transaction_escape;
        self
    }

    /// Return the `Error` value a catch would bind, materializing it lazily for
    /// runtime faults that have not crossed a catch site yet.
    pub fn error_value(&self) -> Option<Value> {
        if !self.catchable {
            None
        } else if let Some(error) = self.throw.as_deref() {
            Some(error.clone())
        } else {
            Some(error_resource(self.code, &self.message))
        }
    }

    /// The original `Error.code` carried by an uncaught language throw.
    pub fn uncaught_throw_code(&self) -> Option<String> {
        if !self.catchable || self.code != RUN_UNCAUGHT_THROW {
            return None;
        }
        self.throw
            .as_deref()
            .and_then(|value| error_field(value, marrow_schema::error::CODE))
    }

    /// Consume the fault and return the `Error` value a catch should bind.
    pub(crate) fn into_catch_value(self) -> Result<Value, RuntimeError> {
        if !self.catchable {
            return Err(self);
        }
        match self.throw {
            Some(error) => Ok(*error),
            None => Ok(error_resource(self.code, &self.message)),
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

/// Temporal arithmetic exceeded the saved date, instant, or duration envelope.
pub const RUN_TEMPORAL_OVERFLOW: &str = "run.temporal_overflow";

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

/// An entry parameter supplied through the host/CLI argument surface failed decoding.
pub const RUN_ENTRY_ARGUMENT: &str = "run.entry_argument";

/// An entry parameter or return value is outside the v0.1 host/CLI surface.
pub const RUN_ENTRY_SURFACE: &str = "run.entry_surface";

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

/// Function-call nesting exceeded [`CALL_DEPTH_BUDGET`]. Runaway or unbounded
/// recursion stops here as a located fault rather than overflowing the native
/// stack.
pub const RUN_DEPTH: &str = "run.depth";

/// The deepest call nesting a run will descend before raising [`RUN_DEPTH`].
/// The entry function runs at depth 1. Attempting depth 257 raises a typed fault
/// instead of recursing. Fixed in v0.1, not configurable.
pub const CALL_DEPTH_BUDGET: usize = 256;

/// A `run.depth` fault raised at the call site that would have descended past
/// [`CALL_DEPTH_BUDGET`].
pub(crate) fn call_depth_exceeded(
    function_name: &str,
    observed_depth: usize,
    span: SourceSpan,
) -> RuntimeError {
    RuntimeError::fault(
        RUN_DEPTH,
        format!(
            "call nesting exceeded the call-depth budget while calling `{function_name}` \
             (budget={CALL_DEPTH_BUDGET}, observed_depth={observed_depth})"
        ),
        span,
    )
}

/// Raise `error` as a catchable language throw on the `Err` channel: the value
/// rides the [`RuntimeError`]'s `throw` field, so a surrounding `try`/`catch`
/// binds it. With no surrounding handler, the activation re-surfaces it. The
/// `code`/`message` carry how it renders if it escapes uncaught: the code
/// `run.uncaught_error` and `uncaught error [{code}]: {message}` from the
/// `Error`'s own fields. Assumes a well-formed Error with string `code` and
/// `message`; a malformed one renders blank, which the constructor and the throw
/// guard make unreachable in practice.
pub(crate) fn raise(error: Value, span: SourceSpan, origin: Option<FileId>) -> RuntimeError {
    raise_with_transaction_escape(error, span, origin, false)
}

pub(crate) fn raise_with_transaction_escape(
    error: Value,
    span: SourceSpan,
    origin: Option<FileId>,
    transaction_escape: bool,
) -> RuntimeError {
    let code = error_field(&error, marrow_schema::error::CODE).unwrap_or_default();
    let message = error_field(&error, marrow_schema::error::MESSAGE).unwrap_or_default();
    RuntimeError {
        code: RUN_UNCAUGHT_THROW,
        message: format!("uncaught error [{code}]: {message}"),
        span,
        throw: Some(Box::new(error)),
        catchable: true,
        transaction_escape,
        // The completion carries the file the throw was first raised in; this
        // caller-frame re-span keeps it rather than re-deriving a shallower one.
        origin,
    }
}

/// A `run.unknown_function` fault for a call/entry that resolves to no function.
pub(crate) fn unknown_function(name: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        throw: None,
        catchable: false,
        transaction_escape: false,
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
        catchable: false,
        transaction_escape: false,
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
        catchable: false,
        transaction_escape: false,
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

pub(crate) fn reraise_fault_with_transaction_escape(
    code: &'static str,
    message: String,
    span: SourceSpan,
    origin: Option<FileId>,
    transaction_escape: bool,
) -> RuntimeError {
    RuntimeError {
        code,
        message,
        span,
        throw: None,
        catchable: true,
        transaction_escape,
        // Kept from the completion so the file the fault was raised in survives
        // this caller-frame re-span.
        origin,
    }
}

/// Raise a recoverable runtime fault (such as a managed-write failure or host
/// capability absence) as a catchable fault while keeping its dotted code. The
/// `Error` value is constructed lazily at the catch site; an uncaught fault
/// surfaces with the same code it did before it became catchable.
pub(crate) fn raise_fault(code: &'static str, message: String, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code,
        message,
        span,
        throw: None,
        catchable: true,
        transaction_escape: false,
        origin: None,
    }
}

#[cfg(test)]
thread_local! {
    static ERROR_VALUE_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// Reset the diagnostic counter for runtime-constructed Error resources.
#[cfg(test)]
fn reset_error_value_allocation_count() {
    ERROR_VALUE_ALLOCATIONS.with(|count| count.set(0));
}

/// The number of Error resources constructed through the runtime fault helper.
#[cfg(test)]
fn error_value_allocation_count() -> usize {
    ERROR_VALUE_ALLOCATIONS.with(Cell::get)
}

#[cfg(test)]
fn note_error_value_allocation() {
    ERROR_VALUE_ALLOCATIONS.with(|count| count.set(count.get() + 1));
}

/// The catchable `Error` resource shape: a `code` field and a `message` field, in
/// that order. The single owner of the runtime's Error-value layout.
fn error_resource(code: &str, message: &str) -> Value {
    #[cfg(test)]
    note_error_value_allocation();
    Value::Resource(vec![
        (
            marrow_schema::error::CODE.to_string(),
            Value::Str(code.to_string()),
        ),
        (
            marrow_schema::error::MESSAGE.to_string(),
            Value::Str(message.to_string()),
        ),
    ])
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
    error_resource(code, &format!("std::io::{op} failed for `{path}`: {error}"))
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
        catchable: false,
        transaction_escape: false,
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

pub(crate) fn entry_argument(message: impl Into<String>) -> RuntimeError {
    RuntimeError::fault(RUN_ENTRY_ARGUMENT, message.into(), SourceSpan::default())
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

pub(crate) fn temporal_overflow(span: SourceSpan) -> RuntimeError {
    raise_fault(
        RUN_TEMPORAL_OVERFLOW,
        "temporal arithmetic exceeded the saved date, instant, or duration envelope".into(),
        span,
    )
}

pub(crate) fn divide_by_zero(message: &str, span: SourceSpan) -> RuntimeError {
    raise_fault(RUN_DIVIDE_BY_ZERO, message.to_string(), span)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use marrow_store::tree::TreeStore;

    use crate::entry::{CheckedEntryCall, run_entry, run_entry_with_host};
    use crate::host::Host;
    use crate::value::Value;

    use super::{
        RUN_ABSENT, RUN_UNCAUGHT_THROW, RuntimeError, error_value_allocation_count,
        reset_error_value_allocation_count,
    };

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(name: &str, source: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after unix epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "marrow-run-unit-{name}-{}-{nanos}-{}",
                std::process::id(),
                TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let src = root.join("src");
            fs::create_dir_all(&src).expect("create unit-test project");
            fs::write(src.join("test.mw"), source).expect("write unit-test source");
            Self { root }
        }

        fn path(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn coalesced_absent_read_allocates_no_error_value() -> Result<(), Box<dyn std::error::Error>> {
        let project = TempProject::new(
            "coalesced-absent",
            "module test\n\nresource Book\n    title: string\nstore ^books(id: int): Book\n\n\
             pub fn read(): string\n    return ^books(1).title ?? \"missing\"\n",
        );
        let program = marrow_check::test_support::commit_then_check(project.path())?.runtime();
        let store = TreeStore::memory();
        let call = CheckedEntryCall::new(&program, "test::read", vec![]).expect("entry");
        let mut output = String::new();

        reset_error_value_allocation_count();
        let value = run_entry(&store, &call, &mut output)
            .expect("coalesced absence resolves")
            .value;

        assert_eq!(value, Some(Value::Str("missing".into())));
        assert_eq!(error_value_allocation_count(), 0);
        Ok(())
    }

    #[test]
    fn caught_absent_fault_allocates_one_error_value_at_the_catch_site()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = TempProject::new(
            "caught-absent",
            "module test\n\npub fn read(): string\n\
             \x20\x20\x20\x20try\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return std::env::require(\"MISSING\")\n\
             \x20\x20\x20\x20catch err: Error\n\
             \x20\x20\x20\x20\x20\x20\x20\x20return err.code\n",
        );
        let program = marrow_check::test_support::commit_then_check(project.path())?.runtime();
        let store = TreeStore::memory();
        let host = Host::new().with_environment(HashMap::new());
        let call = CheckedEntryCall::new(&program, "test::read", vec![]).expect("entry");
        let mut output = String::new();

        reset_error_value_allocation_count();
        let value = run_entry_with_host(&store, &host, &call, &mut output)
            .expect("caught absence resolves")
            .value;

        assert_eq!(value, Some(Value::Str(RUN_ABSENT.into())));
        assert_eq!(error_value_allocation_count(), 1);
        Ok(())
    }

    #[test]
    fn fatal_error_with_throw_does_not_produce_catch_value() {
        let fatal = RuntimeError {
            code: "run.fatal",
            message: "fatal".into(),
            span: marrow_syntax::SourceSpan::default(),
            throw: Some(Box::new(Value::Resource(Vec::new()))),
            catchable: false,
            transaction_escape: false,
            origin: None,
        };

        assert!(fatal.error_value().is_none());
        assert!(matches!(
            fatal.into_catch_value(),
            Err(error) if error.code == "run.fatal" && error.throw.is_some()
        ));

        let fatal_uncaught_throw = RuntimeError {
            code: RUN_UNCAUGHT_THROW,
            message: "fatal uncaught throw".into(),
            span: marrow_syntax::SourceSpan::default(),
            throw: Some(Box::new(Value::Resource(vec![
                (
                    marrow_schema::error::CODE.to_string(),
                    Value::Str("debug.fatal".into()),
                ),
                (
                    marrow_schema::error::MESSAGE.to_string(),
                    Value::Str("debugger fatal throw".into()),
                ),
            ]))),
            catchable: false,
            transaction_escape: false,
            origin: None,
        };

        assert!(fatal_uncaught_throw.error_value().is_none());
        assert!(fatal_uncaught_throw.uncaught_throw_code().is_none());
        assert!(matches!(
            fatal_uncaught_throw.into_catch_value(),
            Err(error) if error.code == RUN_UNCAUGHT_THROW && error.throw.is_some()
        ));
    }
}
