//! The Marrow runtime: evaluate checked `.mw` functions.
//!
//! The evaluator runs functions over scalar values (integers, booleans,
//! strings) with locals, arithmetic/comparison/logical/`_` operators,
//! conditionals, `while`/`for` loops, interpolation, and calls between
//! functions. It reads saved data (fields and keyed-leaf entries) and writes it
//! through the managed-write layer (`^books(id).field = …`, `delete`, `append`),
//! groups writes in a `transaction` (commit/rollback with read-your-writes),
//! guards a block with `lock` (a scope released on every exit under the
//! single-writer profile), and
//! provides the `print`/`write`/`exists`/`get`/`nextId`/`append` builtins, the
//! `std::assert`/`std::text`/`std::math` library helpers, and the
//! `std::clock::now()` and `std::env` host capabilities. Whole-resource writes, `merge`, index
//! traversal, and structured errors build on this spine.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_check::{CheckedFunction, CheckedParam, CheckedProgram, MarrowType, PrimitiveType};
use marrow_schema::{IndexSchema, LayerMember, ResourceSchema};
use marrow_store::Decimal;
use marrow_store::backend::Backend;
use marrow_store::mem::{MemStore, Presence, StoreError};
use marrow_store::path::{ChildSegment, PathSegment, SavedKey, encode_path};
use marrow_store::value::{SavedValue, ValueError, ValueType, decode_value, encode_value};
use marrow_syntax::{
    ArgMode, Argument, BinaryOp, Block, Expression, ForBinding, FunctionDecl, InterpolationPart,
    LiteralKind, ParamMode, SourceSpan, Statement, UnaryOp,
};
use marrow_write::{
    FieldValue, ResourceValue, WRITE_REQUIRED_FIELD, WriteError, decode_identity, next_id,
    next_layer_pos, plan_field_delete, plan_field_write, plan_layer_group_write,
    plan_layer_leaf_write, plan_layer_merge, plan_nested_field_write, plan_resource_delete,
    plan_resource_merge, plan_resource_write,
};

pub mod base64;

/// A runtime value. This models the scalar shapes a pure function needs; saved
/// trees, identities, and error values arrive with the features that produce
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
    /// A UTC instant in nanoseconds since the Unix epoch, e.g. from
    /// `std::clock::now()`. Saves and loads as the `instant` type.
    Instant(i128),
    /// A UTC calendar date as days since the Unix epoch, e.g. from
    /// `std::clock::today()`. Saves and loads as the `date` type.
    Date(i32),
    /// A signed time span in nanoseconds, e.g. from `std::clock::parseDuration`.
    /// Saves and loads as the `duration` type.
    Duration(i128),
    /// An exact base-10 decimal. Saves and loads as the `decimal` type.
    Decimal(Decimal),
    /// Arbitrary bytes. Saves and loads as the `bytes` type; has no direct text
    /// form (use `std::bytes::base64Encode`).
    Bytes(Vec<u8>),
    /// An ordered, in-memory `sequence[T]` value, e.g. from `std::text::split`.
    /// Iterated by a `for` loop; not itself a scalar saved value.
    Sequence(Vec<Value>),
    /// A materialized resource tree: its present top-level fields, in schema
    /// order. Produced by a whole-resource read and consumed by a whole-resource
    /// write or `merge`.
    Resource(Vec<(String, Value)>),
    /// A resource identity (`Book::Id(17)`, `Enrollment::Id(...)`): its lowered
    /// key segments in declared identity-key order. Produced by an identity
    /// constructor and spliced back into the saved path at a keyed lookup. It is
    /// opaque — not a saved field value, not rendered, not iterated.
    Identity(Vec<SavedKey>),
}

/// The result of running an entry function: its returned value (if any) and
/// everything it wrote to the output stream via `print`/`write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub value: Option<Value>,
    pub output: String,
}

/// A runtime fault: a stable `run.*` code, a human-readable message, and the
/// source span of the construct that raised it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: &'static str,
    pub message: String,
    pub span: SourceSpan,
}

/// A value was used where another type was required (e.g. `+` on a non-integer,
/// a non-boolean condition, or assigning to an immutable binding).
pub const RUN_TYPE: &str = "run.type";
/// A name was read or assigned that is not bound in scope.
pub const RUN_UNBOUND_NAME: &str = "run.unbound_name";
/// Integer arithmetic overflowed the 64-bit range.
pub const RUN_OVERFLOW: &str = "run.overflow";
/// Integer division or remainder by zero.
pub const RUN_DIVIDE_BY_ZERO: &str = "run.divide_by_zero";
/// A `break` or `continue` reached the top of a function with no loop to target.
pub const RUN_NO_ENCLOSING_LOOP: &str = "run.no_enclosing_loop";
/// A call named a function the program does not declare.
pub const RUN_UNKNOWN_FUNCTION: &str = "run.unknown_function";
/// A call to a function that returns no value was used where a value is needed.
pub const RUN_NO_VALUE: &str = "run.no_value";
/// A direct read of a saved element that is absent (unpopulated).
pub const RUN_ABSENT: &str = "run.absent_element";
/// The store reported an error (e.g. a corrupt stored path) during a read.
pub const RUN_STORE: &str = "run.store";
/// A construct this slice of the runtime does not yet evaluate.
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
/// A write, delete, append, or merge changed the saved layer a loop was actively
/// traversing. The static rule `check.loop_mutates_traversed_layer` catches the
/// obvious cases; this is the dynamic guard for a path the checker cannot prove.
pub const RUN_TRAVERSAL: &str = "run.traversal";

/// The host capabilities a run may use. Pure runs need none; host modules such
/// as `std::clock` require the matching capability, and a call made without it
/// raises a typed capability error (`run.capability`). A command or embedding
/// provides the capabilities its run needs.
#[derive(Debug, Clone, Default)]
pub struct Host {
    /// The run's UTC instant in nanoseconds since the epoch, when a clock
    /// capability is provided. Captured once, so every `std::clock::now()` in
    /// the run sees one consistent instant.
    clock: Option<i128>,
    /// The run's environment variables, when an environment capability is
    /// provided. A run without it cannot use `std::env`.
    environment: Option<HashMap<String, String>>,
    /// The run's log sink, when a log capability is provided. `std::log` appends
    /// formatted lines here; the command or embedding decides where they go
    /// (e.g. standard error). A run without it cannot use `std::log`.
    log: Option<Rc<RefCell<String>>>,
    /// Whether the run may touch the real filesystem through `std::io`. Marrow
    /// does not sandbox paths; the host either grants filesystem access or not.
    filesystem: bool,
}

impl Host {
    /// A host that provides no capabilities.
    pub fn new() -> Self {
        Self::default()
    }

    /// A host whose clock reads the real system time, captured now.
    pub fn with_system_clock(mut self) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as i128)
            .unwrap_or(0);
        self.clock = Some(nanos);
        self
    }

    /// A host whose clock returns a fixed instant (nanoseconds since the Unix
    /// epoch, UTC), for deterministic runs and tests.
    pub fn with_clock(mut self, nanos: i128) -> Self {
        self.clock = Some(nanos);
        self
    }

    /// A host whose environment is the process's real environment variables,
    /// captured now.
    pub fn with_system_environment(mut self) -> Self {
        self.environment = Some(std::env::vars().collect());
        self
    }

    /// A host whose environment is the given variables, for deterministic runs
    /// and tests.
    pub fn with_environment(mut self, variables: HashMap<String, String>) -> Self {
        self.environment = Some(variables);
        self
    }

    /// A host that collects `std::log` output into `sink`. The caller owns the
    /// sink (a shared buffer), so a command can flush it to standard error and a
    /// test can inspect it.
    pub fn with_log_sink(mut self, sink: Rc<RefCell<String>>) -> Self {
        self.log = Some(sink);
        self
    }

    /// A host that grants `std::io` access to the real filesystem.
    pub fn with_filesystem(mut self) -> Self {
        self.filesystem = true;
        self
    }
}

/// Evaluate a standalone function with positional `args`, returning its returned
/// value or `None`. Calls to other functions are not resolved (there is no
/// surrounding program), and no host capabilities are provided; use [`run_entry`]
/// to run a function that calls others.
pub fn evaluate_function(
    function: &FunctionDecl,
    args: &[Value],
) -> Result<Option<Value>, RuntimeError> {
    let program = CheckedProgram::default();
    let store = RefCell::new(MemStore::new());
    let host = Host::new();
    let output = Rc::new(RefCell::new(String::new()));
    let names: Vec<&str> = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    let ctx = Context {
        program: &program,
        store: &store,
        host: &host,
    };
    match invoke(
        ctx,
        output,
        &names,
        &function.body,
        function.span,
        args,
        &[],
    )? {
        (Completion::Returned(value), _) => Ok(value),
        (Completion::Threw(error), _) => Err(uncaught_throw(&error, function.span)),
    }
}

/// Run the function named by `entry` — `"module::function"`, or a bare name
/// searched across modules — from a checked `program` with positional `args`,
/// providing no host capabilities. Calls within the body resolve against the
/// same `program`.
pub fn run_entry(
    program: &CheckedProgram,
    store: &RefCell<dyn Backend>,
    entry: &str,
    args: &[Value],
) -> Result<RunOutput, RuntimeError> {
    run_entry_with_host(program, store, &Host::new(), entry, args)
}

/// Like [`run_entry`], but with explicit host capabilities (e.g. a clock for
/// `std::clock::now()`). A command or embedding supplies the capabilities its
/// run needs.
pub fn run_entry_with_host(
    program: &CheckedProgram,
    store: &RefCell<dyn Backend>,
    host: &Host,
    entry: &str,
    args: &[Value],
) -> Result<RunOutput, RuntimeError> {
    let segments: Vec<String> = entry.split("::").map(str::to_string).collect();
    let function = resolve_function(program, &segments).ok_or_else(|| RuntimeError {
        code: RUN_UNKNOWN_FUNCTION,
        message: format!("the program has no function `{entry}`"),
        span: SourceSpan::default(),
    })?;
    let output = Rc::new(RefCell::new(String::new()));
    let names: Vec<&str> = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    let ctx = Context {
        program,
        store,
        host,
    };
    let value = match invoke(
        ctx,
        Rc::clone(&output),
        &names,
        &function.body,
        function.span,
        args,
        &[],
    )? {
        (Completion::Returned(value), _) => value,
        (Completion::Threw(error), _) => return Err(uncaught_throw(&error, function.span)),
    };
    Ok(RunOutput {
        value,
        output: output.borrow().clone(),
    })
}

/// How a function activation finished: with an optional returned value, or via
/// an uncaught throw carrying its `Error` value. A throw crosses the call
/// boundary as a catchable error (so a caller's `catch` can bind it), not as a
/// runtime fault.
enum Completion {
    Returned(Option<Value>),
    Threw(Value),
}

/// Bind `args` to `param_names`, evaluate `body` in a fresh activation, and
/// surface how it finished plus, for each `out`/`inout` parameter named in
/// `writeback`, its final value (param-order-aligned, `Some` only when the body
/// returned normally — a throw or fault skips write-back). Shared by
/// [`evaluate_function`], [`run_entry`], and call evaluation; non-`out`/`inout`
/// calls pass an empty `writeback`.
fn invoke(
    ctx: Context<'_>,
    output: Rc<RefCell<String>>,
    param_names: &[&str],
    body: &Block,
    span: SourceSpan,
    args: &[Value],
    writeback: &[&str],
) -> Result<(Completion, Vec<Option<Value>>), RuntimeError> {
    if args.len() != param_names.len() {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: format!(
                "expected {} argument(s), got {}",
                param_names.len(),
                args.len()
            ),
            span,
        });
    }
    let mut env = Env::new(ctx, output);
    env.push_scope();
    for (name, arg) in param_names.iter().zip(args) {
        // `out`/`inout` parameters are reassignable inside the callee; plain
        // parameters are read-only.
        env.bind((*name).to_string(), arg.clone(), writeback.contains(name));
    }
    let outcome = eval_block(body, &mut env);
    // A throw raised in a function this body called is stashed on the env; it
    // surfaces here as this activation's own throw rather than a fault.
    let propagated = env.pending_throw.take();
    // Harvest `out`/`inout` final values before the scope is popped, but only on a
    // normal return — a throw or fault writes nothing back.
    let finals: Vec<Option<Value>> = match &outcome {
        Ok(Flow::Return(_)) | Ok(Flow::Normal) => param_names
            .iter()
            .map(|&name| {
                if writeback.contains(&name) {
                    env.lookup(name).cloned()
                } else {
                    None
                }
            })
            .collect(),
        _ => vec![None; param_names.len()],
    };
    env.pop_scope();
    let completion = match outcome {
        Ok(Flow::Return(value)) => Completion::Returned(value),
        Ok(Flow::Normal) => Completion::Returned(None),
        Ok(Flow::Throw(value)) => Completion::Threw(value),
        // A propagating `throw` rides the `Err` channel as the `RUN_UNCAUGHT_THROW`
        // sentinel with the Error stashed; surface it as this activation's throw.
        // A catchable fault (e.g. `write.unique_conflict`, `run.absent_element`)
        // also stashes its Error so an enclosing `try` can bind it, but its `Err`
        // keeps its own dotted code: when it escapes uncaught, surface that code
        // unchanged rather than collapsing it to `run.uncaught_error`. The stashed
        // Error was already taken into `propagated`, so it does not leak onward.
        Err(error) if error.code == RUN_UNCAUGHT_THROW => match propagated {
            Some(thrown) => Completion::Threw(thrown),
            None => return Err(error),
        },
        Err(error) => return Err(error),
        Ok(Flow::Break(_)) | Ok(Flow::Continue(_)) => {
            return Err(RuntimeError {
                code: RUN_NO_ENCLOSING_LOOP,
                message: "`break` or `continue` outside a loop".into(),
                span,
            });
        }
    };
    Ok((completion, finals))
}

/// Map a thrown `Error` value that left the function uncaught to a runtime fault,
/// surfacing the error's own `code` and `message` in the fault message. Assumes a
/// well-formed Error (string `code`/`message`); a malformed one renders blank,
/// which the constructor and the throw guard make unreachable in practice.
fn uncaught_throw(value: &Value, span: SourceSpan) -> RuntimeError {
    let code = error_field(value, "code").unwrap_or_default();
    let message = error_field(value, "message").unwrap_or_default();
    RuntimeError {
        code: RUN_UNCAUGHT_THROW,
        message: format!("uncaught error [{code}]: {message}"),
        span,
    }
}

/// Resolve a function name to its declaration. A qualified name's last segment
/// is the function and the rest its module; a bare name is searched across all
/// modules. Returns `None` when no function matches.
fn resolve_function<'p>(
    program: &'p CheckedProgram,
    segments: &[String],
) -> Option<&'p CheckedFunction> {
    let (name, module) = segments.split_last()?;
    if module.is_empty() {
        program
            .modules
            .iter()
            .flat_map(|module| &module.functions)
            .find(|function| &function.name == name)
    } else {
        let module_name = module.join("::");
        program
            .modules
            .iter()
            .find(|module| module.name == module_name)?
            .functions
            .iter()
            .find(|function| &function.name == name)
    }
}

/// Bind a call's positional and named arguments to a function's parameters,
/// returning the argument values in parameter order. Positional arguments fill
/// parameters left to right and must precede any named argument; a named
/// argument binds the parameter of that name. Each parameter must be supplied
/// exactly once. This is the plain (by-value) path; a call carrying `out`/`inout`
/// arguments goes through [`bind_arguments_with_modes`] instead.
fn bind_arguments(
    params: &[CheckedParam],
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut slots: Vec<Option<Value>> = vec![None; params.len()];
    let mut next_positional = 0;
    let mut seen_named = false;
    for arg in args {
        let index = arg_param_index(arg, params, &mut next_positional, &mut seen_named, span)?;
        let value = eval_expr(&arg.value, env)?;
        place_argument(&mut slots, index, value, params, span)?;
    }
    collect_arguments(slots, params, span)
}

/// Like [`bind_arguments`], but also resolves each `out`/`inout` argument to the
/// [`Place`] to write back to (param-order-aligned: `Some` for a moded argument,
/// `None` otherwise) and validates that argument and parameter modes agree. An
/// `inout` place is read now to seed the parameter; an `out` parameter is seeded
/// with a type-directed default it is expected to overwrite.
fn bind_arguments_with_modes(
    params: &[CheckedParam],
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(Vec<Value>, Vec<Option<Place>>), RuntimeError> {
    // `Place` is not `Clone`, so build the slots without the `vec![None; n]` clone.
    let mut slots: Vec<Option<(Value, Option<Place>)>> = (0..params.len()).map(|_| None).collect();
    let mut next_positional = 0;
    let mut seen_named = false;
    for arg in args {
        let index = arg_param_index(arg, params, &mut next_positional, &mut seen_named, span)?;
        let param = params
            .get(index)
            .ok_or_else(|| type_error("call has more arguments than parameters", span))?;
        if !modes_match(arg.mode, param.mode) {
            return Err(type_error(
                &format!("argument mode does not match parameter `{}`", param.name),
                span,
            ));
        }
        let entry = match arg.mode {
            None => (eval_expr(&arg.value, env)?, None),
            Some(ArgMode::InOut) => {
                let place = resolve_place(&arg.value, span, env)?;
                let current = place.read(span, env)?;
                (current, Some(place))
            }
            Some(ArgMode::Out) => {
                // `out` does not read the place, so it need not exist yet.
                let place = resolve_place(&arg.value, span, env)?;
                (zero_value(&param.ty), Some(place))
            }
        };
        place_argument(&mut slots, index, entry, params, span)?;
    }
    let entries = collect_arguments(slots, params, span)?;
    Ok(entries.into_iter().unzip())
}

/// Resolve which parameter an argument fills: a positional argument takes the
/// next slot (and must precede any named argument); a named argument names its
/// parameter. Advances the positional cursor and the "seen a named one" flag.
fn arg_param_index(
    arg: &Argument,
    params: &[CheckedParam],
    next_positional: &mut usize,
    seen_named: &mut bool,
    span: SourceSpan,
) -> Result<usize, RuntimeError> {
    match &arg.name {
        None => {
            // A positional argument after a named one would silently back-fill an
            // earlier parameter; named arguments come last.
            if *seen_named {
                return Err(type_error(
                    "a positional argument cannot follow a named argument",
                    span,
                ));
            }
            let index = *next_positional;
            *next_positional += 1;
            Ok(index)
        }
        Some(name) => {
            *seen_named = true;
            params
                .iter()
                .position(|param| &param.name == name)
                .ok_or_else(|| type_error(&format!("call has no parameter `{name}`"), span))
        }
    }
}

/// Place `value` in parameter `index`'s slot, rejecting an out-of-range index or
/// a parameter supplied more than once.
fn place_argument<T>(
    slots: &mut [Option<T>],
    index: usize,
    value: T,
    params: &[CheckedParam],
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    let slot = slots
        .get_mut(index)
        .ok_or_else(|| type_error("call has more arguments than parameters", span))?;
    if slot.is_some() {
        return Err(type_error(
            &format!(
                "parameter `{}` is supplied more than once",
                params[index].name
            ),
            span,
        ));
    }
    *slot = Some(value);
    Ok(())
}

/// Unwrap each parameter's slot in order, erroring on a missing argument.
fn collect_arguments<T>(
    slots: Vec<Option<T>>,
    params: &[CheckedParam],
    span: SourceSpan,
) -> Result<Vec<T>, RuntimeError> {
    slots
        .into_iter()
        .zip(params)
        .map(|(slot, param)| {
            slot.ok_or_else(|| type_error(&format!("missing argument for `{}`", param.name), span))
        })
        .collect()
}

/// Whether an argument's mode matches a parameter's: both plain, both `out`, or
/// both `inout`.
fn modes_match(arg: Option<ArgMode>, param: Option<ParamMode>) -> bool {
    matches!(
        (arg, param),
        (None, None)
            | (Some(ArgMode::Out), Some(ParamMode::Out))
            | (Some(ArgMode::InOut), Some(ParamMode::InOut))
    )
}

/// A resolved assignable place for an `out`/`inout` argument, captured before the
/// call (its saved identity keys evaluated once) so it can be read for `inout` and
/// written back without re-evaluating those keys.
enum Place {
    /// A bare local variable: `n` or `book`.
    Local(String),
    /// A field of a local resource variable: `book.title`.
    LocalField { base: String, field: String },
    /// A saved scalar field: `^books(id).title`.
    SavedField {
        root: String,
        identity: Vec<SavedKey>,
        field: String,
    },
    /// A saved scalar field inside a keyed group entry, at any nesting depth:
    /// `^books(id).versions(v).comments(c).text`. `layers` is the chain of
    /// `(layer, key…)` levels from the record to the innermost group.
    SavedNestedField {
        root: String,
        identity: Vec<SavedKey>,
        layers: Vec<(String, Vec<SavedKey>)>,
        field: String,
    },
    /// A whole saved resource: `^books(id)`.
    SavedResource {
        root: String,
        identity: Vec<SavedKey>,
    },
}

/// Resolve an `out`/`inout` argument expression to its [`Place`], evaluating any
/// saved identity keys now. Supports a bare local, a field of a local resource, a
/// saved scalar field, and a whole saved resource; other shapes (keyed group
/// entries) defer.
fn resolve_place(
    expr: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Place, RuntimeError> {
    match expr {
        Expression::Name { segments, .. } if segments.len() == 1 => {
            Ok(Place::Local(segments[0].clone()))
        }
        Expression::Field { base, name, .. } if is_saved_path(base) => {
            // `^root(id…).field` is a top-level field; a deeper base
            // `^root(id…).layer(key…)….field` is a field inside a nested group
            // entry. `lower_layer_path` yields the chain (empty for the top level).
            let (root, identity, layers) = lower_layer_path(base, env)?;
            if layers.is_empty() {
                Ok(Place::SavedField {
                    root,
                    identity,
                    field: name.clone(),
                })
            } else {
                Ok(Place::SavedNestedField {
                    root,
                    identity,
                    layers,
                    field: name.clone(),
                })
            }
        }
        // `book.title` — a field of a local resource variable (the base is a bare
        // local name, not a saved path).
        Expression::Field { base, name, .. } if matches!(base.as_ref(), Expression::Name { segments, .. } if segments.len() == 1) =>
        {
            let Expression::Name { segments, .. } = base.as_ref() else {
                unreachable!("guarded by the match arm")
            };
            Ok(Place::LocalField {
                base: segments[0].clone(),
                field: name.clone(),
            })
        }
        Expression::Call { .. } if is_saved_path(expr) => {
            let (root, identity) = lower_record_identity(expr, env)?;
            Ok(Place::SavedResource { root, identity })
        }
        _ => Err(unsupported(
            "an out/inout argument that is not an assignable place",
            span,
        )),
    }
}

impl Place {
    /// The current value at this place, to seed an `inout` parameter.
    fn read(&self, span: SourceSpan, env: &Env<'_>) -> Result<Value, RuntimeError> {
        match self {
            Place::Local(name) => env.lookup(name).cloned().ok_or_else(|| RuntimeError {
                code: RUN_UNBOUND_NAME,
                message: format!("`{name}` is not bound"),
                span,
            }),
            Place::LocalField { base, field } => read_local_field(base, field, span, env),
            Place::SavedField {
                root,
                identity,
                field,
            } => read_saved_field(root, identity, field, span, env),
            Place::SavedNestedField {
                root,
                identity,
                layers,
                field,
            } => read_nested_field(root, identity, layers, field, span, env),
            Place::SavedResource { root, identity } => read_resource(root, identity, span, env),
        }
    }

    /// Write `value` back to this place after the callee returns normally.
    fn write(self, value: Value, span: SourceSpan, env: &mut Env<'_>) -> Result<(), RuntimeError> {
        match self {
            Place::Local(name) => env
                .assign(&name, value)
                .map_err(|error| assign_error(&name, error, span)),
            Place::LocalField { base, field } => write_local_field(&base, &field, value, span, env),
            Place::SavedField {
                root,
                identity,
                field,
            } => write_saved_field(&root, &identity, &field, value, span, env),
            Place::SavedNestedField {
                root,
                identity,
                layers,
                field,
            } => write_nested_field(&root, &identity, &layers, &field, value, span, env),
            Place::SavedResource { root, identity } => {
                write_resource(&root, &identity, value, span, env)
            }
        }
    }
}

/// A type's default value, used to seed an `out` parameter before the callee
/// assigns it. A correct callee assigns it before returning (a checker rule to
/// require this is still pending), so the placeholder is normally unobserved; a
/// type without a simple zero starts as an empty resource.
fn zero_value(ty: &MarrowType) -> Value {
    match ty {
        MarrowType::Primitive(PrimitiveType::Int) => Value::Int(0),
        MarrowType::Primitive(PrimitiveType::Bool) => Value::Bool(false),
        MarrowType::Primitive(PrimitiveType::String) => Value::Str(String::new()),
        MarrowType::Primitive(PrimitiveType::Bytes) => Value::Bytes(Vec::new()),
        _ => Value::Resource(Vec::new()),
    }
}

/// The default value a typed `var` with no initializer starts at, by its declared
/// type name. `None` for a type with no representable default (it stays
/// unsupported). Resource types are handled by the caller (an empty resource).
fn uninitialized_default(type_name: &str) -> Option<Value> {
    if type_name.starts_with("sequence") {
        return Some(Value::Sequence(Vec::new()));
    }
    Some(match type_name {
        "int" => Value::Int(0),
        "bool" => Value::Bool(false),
        "string" => Value::Str(String::new()),
        "bytes" => Value::Bytes(Vec::new()),
        "date" => Value::Date(0),
        "instant" => Value::Instant(0),
        "duration" => Value::Duration(0),
        "decimal" => Value::Decimal(Decimal::parse("0")?),
        _ => return None,
    })
}

/// Map an [`AssignError`] from a failed reassignment to a runtime fault.
fn assign_error(name: &str, error: AssignError, span: SourceSpan) -> RuntimeError {
    match error {
        AssignError::Immutable => RuntimeError {
            code: RUN_TYPE,
            message: format!("cannot assign to immutable `{name}`"),
            span,
        },
        AssignError::Unbound => RuntimeError {
            code: RUN_UNBOUND_NAME,
            message: format!("`{name}` is not bound"),
            span,
        },
    }
}

/// Evaluate a call to a program function, returning its returned value (or
/// `None` for a function that returns nothing). Arguments may be positional or
/// named, and `out`/`inout` arguments write back to an assignable place (a local
/// or a saved path) after the call.
fn eval_call(
    callee: &Expression,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // A call whose callee names a declared index off a saved root
    // (`^books.byIsbn(isbn)`) is an index lookup, not a keyed-layer read.
    if let Expression::Field { base, name, .. } = callee
        && let Expression::SavedRoot { name: root, .. } = base.as_ref()
        && let Some(resource) = find_resource(env.program, root)
        && let Some(index) = resource.indexes.iter().find(|index| &index.name == name)
    {
        let value = eval_index_lookup(resource, index, args, span, env);
        return catchable_read(value, env).map(Some);
    }
    // A call whose callee is a saved layer field is a keyed-layer read — a leaf
    // value `^books(id).tags(pos)` or a whole group entry `^books(id).versions(v)`.
    if let Expression::Field { .. } = callee {
        return eval_saved_layer_read(callee, args, span, env).map(Some);
    }
    // A call whose callee is a saved root is a whole-resource read, `^books(id)`.
    if let Expression::SavedRoot { .. } = callee {
        return eval_resource_read(callee, args, span, env).map(Some);
    }
    let Expression::Name { segments, .. } = callee else {
        return Err(unsupported("calling this expression", span));
    };
    // `Error(...)` is the builtin error constructor (named arguments), not a
    // program function.
    if let [name] = segments.as_slice()
        && name == "Error"
    {
        return eval_error_constructor(args, span, env).map(Some);
    }
    // `Resource::Id(...)` constructs a resource identity. It may carry named
    // (composite) keys, so it is dispatched before the named/moded guard that
    // routes named calls to program functions.
    if let [name, id] = segments.as_slice()
        && id == "Id"
        && let Some(resource) = find_resource_by_name(env.program, name)
    {
        return eval_identity_constructor(resource, args, span, env).map(Some);
    }
    // Builtins and the host capability take positional arguments only; a call
    // carrying named arguments is a program-function call, handled last.
    let has_named = args.iter().any(|arg| arg.name.is_some());
    // `out`/`inout` arguments only apply to program functions, so a moded call
    // skips the builtin and host-capability dispatch below.
    let has_moded = args.iter().any(|arg| arg.mode.is_some());
    if !has_named && !has_moded {
        // Builtins are call-shaped but are not program functions.
        if let [name] = segments.as_slice() {
            match name.as_str() {
                "print" | "write" => return eval_output(name, args, span, env),
                "exists" => return eval_exists(args, span, env).map(Some),
                "get" => return eval_get(args, span, env).map(Some),
                "nextId" => return eval_next_id(args, span, env).map(Some),
                "append" => return eval_append(args, span, env).map(Some),
                "bytes" => return eval_bytes_conversion(args, span, env).map(Some),
                "int" | "decimal" | "string" | "bool" | "date" | "instant" | "duration" => {
                    return eval_conversion(name, args, span, env).map(Some);
                }
                // `keys(<layer>)` materializes the layer's child keys as a sequence
                // value (the same enumeration `for x in <layer>` drives).
                "keys" => return eval_keys(args, span, env).map(Some),
                // `count(path)` is a one-layer tree scan over the lowered path.
                "count" => return eval_count(args, span, env).map(Some),
                // `values`/`entries` materialize each child's value (a whole record
                // for a primary root, an entry value for a keyed layer); `entries`
                // pairs it with the key for the two-name `for k, v in ...` binding.
                "values" => return eval_values(args, span, env).map(Some),
                "entries" => return eval_entries(args, span, env).map(Some),
                _ => {}
            }
        }
        // `std::clock::now()`/`today()` read the host clock capability; the rest of
        // `std::clock` is pure (matched later via `eval_std`).
        if let [first, second, op] = segments.as_slice()
            && first == "std"
            && second == "clock"
            && (op == "now" || op == "today")
        {
            return eval_clock_capability(op, args, span, env).map(Some);
        }
        // `std::env::*` reads the run's environment capability.
        if let [first, second, op] = segments.as_slice()
            && first == "std"
            && second == "env"
        {
            return eval_env(op, args, span, env).map(Some);
        }
        // `std::log::*` writes to the run's log capability and yields nothing.
        if let [first, second, op] = segments.as_slice()
            && first == "std"
            && second == "log"
        {
            return eval_log(op, args, span, env);
        }
        // `std::io::*` reads and writes files through the filesystem capability.
        if let [first, second, op] = segments.as_slice()
            && first == "std"
            && second == "io"
        {
            return eval_io(op, args, span, env);
        }
        // `std::assert::*` testing builtins raise `run.assertion` on failure.
        if let [first, second, op] = segments.as_slice()
            && first == "std"
            && second == "assert"
        {
            return eval_assert(op, args, span, env);
        }
        // Pure `std::text/math/bytes/clock` helpers. (`std::clock::now` is a host
        // capability, matched above; the rest of `std::clock` is pure.)
        if let [first, second, op] = segments.as_slice()
            && first == "std"
            && (second == "text" || second == "math" || second == "bytes" || second == "clock")
        {
            return eval_std(second, op, args, span, env).map(Some);
        }
    }
    let ctx = Context {
        program: env.program,
        store: env.store,
        host: env.host,
    };
    let function = resolve_function(ctx.program, segments).ok_or_else(|| RuntimeError {
        code: RUN_UNKNOWN_FUNCTION,
        message: format!("the program has no function `{}`", segments.join("::")),
        span,
    })?;
    if has_moded {
        return eval_call_with_modes(function, args, span, env);
    }
    let values = bind_arguments(&function.params, args, span, env)?;
    let names: Vec<&str> = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    let (completion, _) = invoke(
        ctx,
        Rc::clone(&env.output),
        &names,
        &function.body,
        function.span,
        &values,
        &[],
    )?;
    complete_call(completion, span, env)
}

/// Turn a callee's [`Completion`] into this activation's result: a normal return
/// yields its value; an uncaught throw is re-raised as a pending throw riding the
/// `Err` channel, consumed by the nearest `try` or this activation's [`invoke`].
fn complete_call(
    completion: Completion,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match completion {
        Completion::Returned(value) => Ok(value),
        Completion::Threw(error) => Err(raise(error, span, env)),
    }
}

/// Raise `error` as a catchable throw from this activation: stash it on the env
/// (it rides the `Err` channel, since calls and builtins are expressions) and
/// return the matching fault sentinel. A surrounding `try`/`catch` binds it; with
/// none, this activation's [`invoke`] re-surfaces it to its caller.
fn raise(error: Value, span: SourceSpan, env: &mut Env<'_>) -> RuntimeError {
    let sentinel = uncaught_throw(&error, span);
    env.pending_throw = Some(error);
    sentinel
}

/// Raise a recoverable runtime fault (a managed-write failure or an absent-element
/// read) as a catchable Error while keeping its dotted code. Like [`raise`], it
/// stashes an `Error` value carrying `code`/`message` so an enclosing `try`/`catch`
/// can bind it. Unlike [`raise`], the returned [`RuntimeError`] keeps the fault's
/// own dotted code rather than the `RUN_UNCAUGHT_THROW` sentinel, so an uncaught
/// fault surfaces with the same code it did before it became catchable.
fn raise_fault(
    code: &'static str,
    message: String,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> RuntimeError {
    env.pending_throw = Some(Value::Resource(vec![
        ("code".to_string(), Value::Str(code.to_string())),
        ("message".to_string(), Value::Str(message.clone())),
    ]));
    RuntimeError {
        code,
        message,
        span,
    }
}

/// Make a value-position read's absent-element fault catchable: an `Err(RUN_ABSENT)`
/// from the shared `&Env` read helpers is re-raised through [`raise_fault`] so an
/// enclosing `try`/`catch` can bind it, while keeping the `run.absent_element` code
/// for an uncaught read. Other results pass through unchanged. (The `inout`/`out`
/// seed reads in [`Place::read`] are argument binding, not value position, so they
/// keep a plain fatal fault.)
fn catchable_read(
    result: Result<Value, RuntimeError>,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match result {
        Err(error) if error.code == RUN_ABSENT => {
            Err(raise_fault(RUN_ABSENT, error.message, error.span, env))
        }
        other => other,
    }
}

/// Evaluate a program-function call that has `out`/`inout` arguments. Each moded
/// argument resolves to an assignable [`Place`] (a local or a saved path) that is
/// read (for `inout`) to seed the parameter and written back after the callee
/// returns normally; the callee's throw or fault skips write-back. The argument's
/// mode must match the parameter's.
fn eval_call_with_modes(
    function: &CheckedFunction,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let (values, places) = bind_arguments_with_modes(&function.params, args, span, env)?;
    let names: Vec<&str> = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    let writeback: Vec<&str> = function
        .params
        .iter()
        .filter(|param| param.mode.is_some())
        .map(|param| param.name.as_str())
        .collect();
    let ctx = Context {
        program: env.program,
        store: env.store,
        host: env.host,
    };
    let (completion, finals) = invoke(
        ctx,
        Rc::clone(&env.output),
        &names,
        &function.body,
        function.span,
        &values,
        &writeback,
    )?;
    // Write each out/inout parameter's final value back to its place. On a throw
    // or fault `finals` is all `None`, so nothing is written.
    for (place, final_value) in places.into_iter().zip(finals) {
        if let (Some(place), Some(value)) = (place, final_value) {
            place.write(value, span, env)?;
        }
    }
    complete_call(completion, span, env)
}

/// Evaluate a `print`/`write` output builtin: render the single argument to text
/// and append it to the output stream (`print` adds a trailing newline). Neither
/// produces a value.
fn eval_output(
    name: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: format!("`{name}` takes one argument"),
            span,
        });
    };
    let text = render(eval_expr(&arg.value, env)?, span)?;
    let mut output = env.output.borrow_mut();
    output.push_str(&text);
    if name == "print" {
        output.push('\n');
    }
    Ok(None)
}

/// Evaluate `exists(path)`: whether a saved value or child exists at the path.
fn eval_exists(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`exists` takes one argument".into(),
            span,
        });
    };
    let segments = lower_saved_path(&arg.value, env)?;
    let store = env.store.borrow();
    let presence = store
        .presence(&encode_path(&segments))
        .map_err(|error| store_error(error, span))?;
    let present = !matches!(presence, Presence::Absent);
    Ok(Value::Bool(present))
}

/// Evaluate `count(path)` per builtins.md: the number of immediate children when
/// the path has any, otherwise `1` for a present scalar value and `0` when the
/// path is absent. A path with both a value and children counts only its
/// children (its own value is `exists(path)` territory).
fn eval_count(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`count` takes one argument".into(),
            span,
        });
    };
    let path = encode_path(&lower_saved_path(&arg.value, env)?);
    let store = env.store.borrow();
    let children = store
        .child_keys(&path)
        .map_err(|error| store_error(error, span))?
        .len();
    let count = if children > 0 {
        children
    } else {
        store
            .read(&path)
            .map_err(|error| store_error(error, span))?
            .is_some() as usize
    };
    Ok(Value::Int(count as i64))
}

/// Evaluate a `std::assert::*` testing builtin (`isTrue`, `isFalse`, `absent`,
/// `fail`). A failed assertion raises a `run.assertion` error carrying the call
/// span, which `marrow test` reports as a located failure. `absent` reports a
/// populated path as a failed assertion rather than silently treating it as
/// absent. None of these produce a value.
fn eval_assert(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match op {
        "isTrue" | "isFalse" => {
            let [arg] = args else {
                return Err(RuntimeError {
                    code: RUN_TYPE,
                    message: format!("`std::assert::{op}` takes one boolean"),
                    span,
                });
            };
            let Value::Bool(actual) = eval_expr(&arg.value, env)? else {
                return Err(RuntimeError {
                    code: RUN_TYPE,
                    message: format!("`std::assert::{op}` takes a boolean"),
                    span,
                });
            };
            if actual != (op == "isTrue") {
                return Err(RuntimeError {
                    code: RUN_ASSERT,
                    message: format!("assertion failed: {op}({actual})"),
                    span,
                });
            }
            Ok(None)
        }
        "absent" => {
            let [arg] = args else {
                return Err(RuntimeError {
                    code: RUN_TYPE,
                    message: "`std::assert::absent` takes one path".into(),
                    span,
                });
            };
            let segments = lower_saved_path(&arg.value, env)?;
            let store = env.store.borrow();
            let presence = store
                .presence(&encode_path(&segments))
                .map_err(|error| store_error(error, span))?;
            if !matches!(presence, Presence::Absent) {
                return Err(RuntimeError {
                    code: RUN_ASSERT,
                    message: "assertion failed: expected the path to be absent".into(),
                    span,
                });
            }
            Ok(None)
        }
        "fail" => {
            let [arg] = args else {
                return Err(RuntimeError {
                    code: RUN_TYPE,
                    message: "`std::assert::fail` takes one message".into(),
                    span,
                });
            };
            let Value::Str(message) = eval_expr(&arg.value, env)? else {
                return Err(RuntimeError {
                    code: RUN_TYPE,
                    message: "`std::assert::fail` takes a string message".into(),
                    span,
                });
            };
            Err(RuntimeError {
                code: RUN_ASSERT,
                message,
                span,
            })
        }
        other => Err(unsupported(&format!("std::assert::{other}"), span)),
    }
}

/// Evaluate a pure `std::text::*` or `std::math::*` helper. These take positional
/// arguments and return a value; they need no host capability.
fn eval_std(
    module: &str,
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match (module, op) {
        ("text", "length") => {
            let [text] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Int(
                eval_text(text, env, span)?.chars().count() as i64
            ))
        }
        ("text", "trim") => {
            let [text] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Str(eval_text(text, env, span)?.trim().to_string()))
        }
        ("text", "contains") => {
            let [text, needle] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(text, env, span)?;
            let needle = eval_text(needle, env, span)?;
            Ok(Value::Bool(text.contains(&needle)))
        }
        ("text", "split") => {
            let [text, separator] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(text, env, span)?;
            let separator = eval_text(separator, env, span)?;
            let parts = text
                .split(separator.as_str())
                .map(|part| Value::Str(part.to_string()))
                .collect();
            Ok(Value::Sequence(parts))
        }
        ("math", "absInt") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Int(
                eval_int(&value.value, env)?
                    .checked_abs()
                    .ok_or_else(|| overflow(span))?,
            ))
        }
        ("math", "remainder") => {
            let [a, b] = args else {
                return Err(std_arity(module, op, span));
            };
            let remainder =
                int_remainder(eval_int(&a.value, env)?, eval_int(&b.value, env)?, span)?;
            Ok(Value::Int(remainder))
        }
        ("math", "modulo") => {
            let [a, b] = args else {
                return Err(std_arity(module, op, span));
            };
            let modulo = int_modulo(eval_int(&a.value, env)?, eval_int(&b.value, env)?, span)?;
            Ok(Value::Int(modulo))
        }
        ("math", "absDecimal") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Decimal(eval_decimal_arg(value, env, span)?.abs()))
        }
        ("math", "floor") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let floored = eval_decimal_arg(value, env, span)?.floor();
            i64::try_from(floored)
                .map(Value::Int)
                .map_err(|_| overflow(span))
        }
        ("bytes", "length") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Int(eval_bytes_arg(value, env, span)?.len() as i64))
        }
        ("bytes", "base64Encode") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            Ok(Value::Str(base64::encode(&eval_bytes_arg(
                value, env, span,
            )?)))
        }
        ("bytes", "base64Decode") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(value, env, span)?;
            base64::decode(&text)
                .map(Value::Bytes)
                .ok_or_else(|| type_error("base64Decode: invalid base64 text", span))
        }
        // An instant has no direct text form; format and parse go through its
        // canonical UTC representation (reusing the store's value codec).
        ("clock", "formatInstant") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let nanos = eval_instant_arg(value, env, span)?;
            let bytes = encode_value(&SavedValue::Instant(nanos))
                .map_err(|error| value_error(error, span))?;
            let text = String::from_utf8(bytes).expect("a canonical instant encodes as UTF-8 text");
            Ok(Value::Str(text))
        }
        ("clock", "parseInstant") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(value, env, span)?;
            match decode_value(text.as_bytes(), ValueType::Instant) {
                Some(SavedValue::Instant(nanos)) => Ok(Value::Instant(nanos)),
                _ => Err(type_error("parseInstant: invalid instant text", span)),
            }
        }
        // Dates and durations share the instant codec route: format and parse go
        // through their canonical text (`YYYY-MM-DD`, `PT<seconds>S`).
        ("clock", "formatDate") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let days = eval_date_arg(value, env, span)?;
            let bytes =
                encode_value(&SavedValue::Date(days)).map_err(|error| value_error(error, span))?;
            let text = String::from_utf8(bytes).expect("a canonical date encodes as UTF-8 text");
            Ok(Value::Str(text))
        }
        ("clock", "parseDate") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(value, env, span)?;
            match decode_value(text.as_bytes(), ValueType::Date) {
                Some(SavedValue::Date(days)) => Ok(Value::Date(days)),
                _ => Err(type_error("parseDate: invalid date text", span)),
            }
        }
        ("clock", "formatDuration") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let nanos = eval_duration_arg(value, env, span)?;
            let bytes = encode_value(&SavedValue::Duration(nanos))
                .map_err(|error| value_error(error, span))?;
            let text =
                String::from_utf8(bytes).expect("a canonical duration encodes as UTF-8 text");
            Ok(Value::Str(text))
        }
        ("clock", "parseDuration") => {
            let [value] = args else {
                return Err(std_arity(module, op, span));
            };
            let text = eval_text(value, env, span)?;
            match decode_value(text.as_bytes(), ValueType::Duration) {
                Some(SavedValue::Duration(nanos)) => Ok(Value::Duration(nanos)),
                _ => Err(type_error("parseDuration: invalid duration text", span)),
            }
        }
        // `add(instant, duration)`: shift an instant by a signed span of nanos.
        ("clock", "add") => {
            let [instant, span_arg] = args else {
                return Err(std_arity(module, op, span));
            };
            let nanos = eval_instant_arg(instant, env, span)?;
            let offset = eval_duration_arg(span_arg, env, span)?;
            nanos
                .checked_add(offset)
                .map(Value::Instant)
                .ok_or_else(|| overflow(span))
        }
        _ => Err(unsupported(&format!("std::{module}::{op}"), span)),
    }
}

/// Convert a string argument to bytes (`bytes(text)`): the string's UTF-8 bytes.
fn eval_bytes_conversion(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("`bytes` takes one argument", span));
    };
    match eval_expr(&arg.value, env)? {
        Value::Str(text) => Ok(Value::Bytes(text.into_bytes())),
        _ => Err(type_error("`bytes` converts a string to bytes", span)),
    }
}

/// Evaluate a scalar conversion builtin (`int`/`decimal`/`string`/`bool`/`date`/
/// `instant`/`duration`): coerce a dynamically-typed value to the named type per
/// `docs/language/types.md`. `bool(...)` accepts the canonical boolean values
/// `{false, true, 0, 1}` from a bool, int, or string; `int(...)`/`decimal(...)`
/// parse canonical numeric text from a string (and raise a typed numeric error on
/// malformed input). The remaining conversions validate that the value already
/// has the named type (the `unknown` → concrete bridge); temporal text parsing
/// lives in `std::clock`, not here.
fn eval_conversion(
    name: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(type_error(&format!("`{name}` takes one argument"), span));
    };
    let value = eval_expr(&arg.value, env)?;
    match name {
        "bool" => convert_to_bool(value, span),
        "int" => convert_to_int(value, span),
        "decimal" => convert_to_decimal(value, span),
        "string" if matches!(value, Value::Str(_)) => Ok(value),
        "date" if matches!(value, Value::Date(_)) => Ok(value),
        "instant" if matches!(value, Value::Instant(_)) => Ok(value),
        "duration" if matches!(value, Value::Duration(_)) => Ok(value),
        _ => Err(conversion_error(name, span)),
    }
}

/// Coerce to a bool: a bool is itself; an int or string is accepted only as a
/// canonical boolean value (`0`/`false` → `false`, `1`/`true` → `true`).
fn convert_to_bool(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    let result = match &value {
        Value::Bool(_) => return Ok(value),
        Value::Int(0) => false,
        Value::Int(1) => true,
        Value::Str(text) if text == "false" || text == "0" => false,
        Value::Str(text) if text == "true" || text == "1" => true,
        _ => return Err(conversion_error("bool", span)),
    };
    Ok(Value::Bool(result))
}

/// Coerce to an int: an int is itself; a string parses as a canonical `i64`
/// (a malformed or out-of-range value is a typed numeric error).
fn convert_to_int(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match value {
        Value::Int(_) => Ok(value),
        Value::Str(text) => text
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| conversion_error("int", span)),
        _ => Err(conversion_error("int", span)),
    }
}

/// Coerce to a decimal: a decimal is itself; a string parses as canonical decimal
/// text (a malformed or out-of-envelope value is a typed numeric error).
fn convert_to_decimal(value: Value, span: SourceSpan) -> Result<Value, RuntimeError> {
    match value {
        Value::Decimal(_) => Ok(value),
        Value::Str(text) => Decimal::parse(&text)
            .map(Value::Decimal)
            .ok_or_else(|| conversion_error("decimal", span)),
        _ => Err(conversion_error("decimal", span)),
    }
}

/// The type error for a value that cannot be converted to `name`.
fn conversion_error(name: &str, span: SourceSpan) -> RuntimeError {
    type_error(&format!("cannot convert this value to {name}"), span)
}

/// Evaluate `arg` to bytes, or a type error.
fn eval_bytes_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Vec<u8>, RuntimeError> {
    match eval_expr(&arg.value, env)? {
        Value::Bytes(bytes) => Ok(bytes),
        _ => Err(type_error("expected bytes", span)),
    }
}

/// Evaluate `arg` to a decimal, or a type error.
fn eval_decimal_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Decimal, RuntimeError> {
    match eval_expr(&arg.value, env)? {
        Value::Decimal(decimal) => Ok(decimal),
        _ => Err(type_error("expected a decimal", span)),
    }
}

/// Evaluate `arg` to an instant (UTC nanoseconds), or a type error.
fn eval_instant_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<i128, RuntimeError> {
    match eval_expr(&arg.value, env)? {
        Value::Instant(nanos) => Ok(nanos),
        _ => Err(type_error("expected an instant", span)),
    }
}

/// Evaluate `arg` to a date (days since the Unix epoch), or a type error.
fn eval_date_arg(arg: &Argument, env: &mut Env<'_>, span: SourceSpan) -> Result<i32, RuntimeError> {
    match eval_expr(&arg.value, env)? {
        Value::Date(days) => Ok(days),
        _ => Err(type_error("expected a date", span)),
    }
}

/// Evaluate `arg` to a duration (signed nanoseconds), or a type error.
fn eval_duration_arg(
    arg: &Argument,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<i128, RuntimeError> {
    match eval_expr(&arg.value, env)? {
        Value::Duration(nanos) => Ok(nanos),
        _ => Err(type_error("expected a duration", span)),
    }
}

/// The wrong-argument-count error for a `std::*` helper.
fn std_arity(module: &str, op: &str, span: SourceSpan) -> RuntimeError {
    type_error(
        &format!("`std::{module}::{op}` got the wrong number of arguments"),
        span,
    )
}

/// Evaluate `arg` to a string, or a type error.
fn eval_text(arg: &Argument, env: &mut Env<'_>, span: SourceSpan) -> Result<String, RuntimeError> {
    match eval_expr(&arg.value, env)? {
        Value::Str(text) => Ok(text),
        _ => Err(type_error("expected a string", span)),
    }
}

/// Truncated integer remainder (sign of the dividend), rejecting a zero divisor
/// and the `i64::MIN % -1` overflow.
fn int_remainder(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    if b == 0 {
        return Err(RuntimeError {
            code: RUN_DIVIDE_BY_ZERO,
            message: "integer remainder by zero".into(),
            span,
        });
    }
    a.checked_rem(b).ok_or_else(|| overflow(span))
}

/// Floored integer modulo (sign of the divisor).
fn int_modulo(a: i64, b: i64, span: SourceSpan) -> Result<i64, RuntimeError> {
    let remainder = int_remainder(a, b, span)?;
    // Shift the truncated remainder toward the divisor's sign when they differ.
    Ok(if remainder != 0 && (remainder < 0) != (b < 0) {
        remainder + b
    } else {
        remainder
    })
}

/// Build a builtin `Error(...)` value from named arguments as a resource-shaped
/// `Value`. `code` and `message` are required; `help` and `data` are optional.
/// Positional, duplicate, or unknown fields are type errors.
fn eval_error_constructor(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut fields: Vec<(String, Value)> = Vec::new();
    for arg in args {
        let Some(name) = &arg.name else {
            return Err(type_error("`Error(...)` takes named arguments", span));
        };
        if !matches!(name.as_str(), "code" | "message" | "help" | "data") {
            return Err(type_error(&format!("`Error` has no field `{name}`"), span));
        }
        if fields.iter().any(|(existing, _)| existing == name) {
            return Err(type_error(
                &format!("`{name}` is supplied more than once"),
                span,
            ));
        }
        fields.push((name.clone(), eval_expr(&arg.value, env)?));
    }
    for required in ["code", "message"] {
        if !fields.iter().any(|(name, _)| name == required) {
            return Err(type_error(&format!("`Error` requires `{required}`"), span));
        }
    }
    Ok(Value::Resource(fields))
}

/// Evaluate `get(path, default)`: the value at a sparse saved path, or `default`
/// when it is absent. Schema/type errors are not hidden — only absence falls
/// back to the default.
fn eval_get(args: &[Argument], span: SourceSpan, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
    let [path, default] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`get` takes a path and a default".into(),
            span,
        });
    };
    match eval_saved_field(&path.value, env) {
        Err(error) if error.code == RUN_ABSENT => {
            // An absent read is a catchable fault that stashed its Error; `get`
            // absorbs the absence as ordinary control flow, so clear the stash
            // before it can be mistaken for an unwinding throw.
            env.pending_throw = None;
            eval_expr(&default.value, env)
        }
        other => other,
    }
}

/// Evaluate `nextId(^root)`: the next integer identity for a single-`int` keyed
/// saved root (one past the highest existing key, or 1 when empty).
fn eval_next_id(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`nextId` takes one argument".into(),
            span,
        });
    };
    let Expression::SavedRoot { name, .. } = &arg.value else {
        return Err(unsupported("`nextId` of this path", span));
    };
    let next = {
        let store = env.store.borrow();
        next_id(name, &*store)
    };
    let next = next.map_err(|error| write_fault(error, span, env))?;
    Ok(Value::Int(next))
}

/// Evaluate `append(^root(key…).layer, value)`: write `value` at the next 1-based
/// position of a keyed-leaf layer and return that position. Reuses marrow-write's
/// `next_layer_pos` (over the live store) and `plan_layer_leaf_write`.
fn eval_append(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [target, value] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`append` takes a layer path and a value".into(),
            span,
        });
    };
    let Expression::Field {
        base, name: layer, ..
    } = &target.value
    else {
        return Err(unsupported("appending to this path", span));
    };
    let (root, identity) = lower_record_identity(base, env)?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("appending under this saved root", span))?;
    // Append adds a key to this layer's key set.
    env.guard_traversed_layer(&layer_prefix(&root, &identity, layer), span)?;
    let saved = value_to_saved(eval_expr(&value.value, env)?)
        .ok_or_else(|| unsupported("appending a resource value", span))?;
    let pos = {
        let store = env.store.borrow();
        next_layer_pos(resource, &identity, layer, &*store)
    };
    let pos = pos.map_err(|error| write_fault(error, span, env))?;
    let plan = plan_layer_leaf_write(resource, &identity, layer, &[SavedKey::Int(pos)], &saved)
        .map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(Value::Int(pos))
}

/// Evaluate `keys(<layer>)` as a value: enumerate the layer's child keys into a
/// [`Value::Sequence`]. The same enumeration drives `for x in <layer>`.
fn eval_keys(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`keys` takes one argument".into(),
            span,
        });
    };
    Ok(Value::Sequence(enumerate_layer(&path.value, env)?))
}

/// Evaluate `values(<layer>)`: each child materialized to its value, in key
/// order. The same materialization drives `for x in values(<layer>)`.
fn eval_values(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`values` takes one argument".into(),
            span,
        });
    };
    let values = materialize_layer(&path.value, env)?
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    Ok(Value::Sequence(values))
}

/// Evaluate `entries(<layer>)`: each child as a `[key, value]` pair sequence, in
/// key order. The two-name `for k, v in entries(<layer>)` binding unpacks each
/// pair; the same materialization drives it.
fn eval_entries(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`entries` takes one argument".into(),
            span,
        });
    };
    let entries = materialize_layer(&path.value, env)?
        .into_iter()
        .map(|(key, value)| Value::Sequence(vec![key, value]))
        .collect();
    Ok(Value::Sequence(entries))
}

/// Materialize a layer's children as `(key, value)` pairs in key order: a whole
/// record per child key for a primary root `^books`, or each entry's value for a
/// keyed/sequence child layer `^books(id).tags`. Reuses [`enumerate_layer`] for
/// the keys and the existing whole-record / layer-entry reads for the values.
/// Index branches inspect identities only (builtins.md), so `values`/`entries`
/// over one is rejected; iterate it or use `keys(...)` instead.
fn materialize_layer(
    path: &Expression,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    let keys = enumerate_layer(path, env)?;
    match path {
        // A primary keyed root: each child key is a record identity, materialized
        // by a whole-record read.
        Expression::SavedRoot { name, span } => keys
            .into_iter()
            .map(|key| {
                let identity = identity_keys(&key, *span)?;
                Ok((key, read_resource(name, &identity, *span, env)?))
            })
            .collect(),
        // A keyed/sequence child layer `^root(id…).layer`: each child key addresses
        // one entry, materialized by a layer-entry read.
        Expression::Field {
            base, name: layer, ..
        } => {
            let span = path.span();
            let (root, identity) = lower_record_identity(base, env)?;
            keys.into_iter()
                .map(|key| {
                    let layer_key = value_to_key(key.clone())
                        .ok_or_else(|| unsupported("a key of this type", span))?;
                    let value = read_layer_entry(&root, &identity, layer, &[layer_key], span, env)?;
                    Ok((key, value))
                })
                .collect()
        }
        // An index branch `^root.index(args…)` yields identities for `keys(...)`;
        // its marker values are a raw inspection detail, not `values`/`entries`.
        other => Err(unsupported(
            "values/entries over this path (use keys(...) or direct iteration)",
            other.span(),
        )),
    }
}

/// The identity keys a primary-root child value addresses: a single-key identity
/// arrives as a bare key value, a composite one as a [`Value::Identity`].
fn identity_keys(key: &Value, span: SourceSpan) -> Result<Vec<SavedKey>, RuntimeError> {
    match key {
        Value::Identity(keys) => Ok(keys.clone()),
        other => Ok(vec![
            value_to_key(other.clone()).ok_or_else(|| unsupported("a key of this type", span))?,
        ]),
    }
}

/// Number of nanoseconds in a UTC day, for `today()`'s instant-to-date reduction.
const NANOS_PER_DAY: i128 = 86_400_000_000_000;

/// Evaluate `std::clock::now()` (an instant) or `std::clock::today()` (the UTC
/// calendar date) from the host's clock capability. A run with no clock
/// capability raises a typed capability error rather than reading the wall clock
/// implicitly.
fn eval_clock_capability(
    op: &str,
    args: &[Argument],
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
        code: RUN_CAPABILITY,
        message: format!("this run provides no clock capability for `std::clock::{op}`"),
        span,
    })?;
    match op {
        "now" => Ok(Value::Instant(nanos)),
        // The UTC calendar date is the floored day count, matching the store's
        // instant-to-date reduction.
        "today" => Ok(Value::Date(nanos.div_euclid(NANOS_PER_DAY) as i32)),
        _ => Err(unsupported(&format!("std::clock::{op}"), span)),
    }
}

/// Evaluate a `std::env::*` builtin against the host's environment capability:
/// `exists(name)`, `get(name, default)`, or `require(name)`. A run with no
/// environment capability raises a typed capability error rather than reading
/// the process environment implicitly; `require` on an absent variable raises a
/// typed absence error.
fn eval_env(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    // Evaluate the string arguments before borrowing the environment, so their
    // mutable use of `env` does not overlap the shared read of the capability.
    let names: Vec<String> = args
        .iter()
        .map(|arg| eval_text(arg, env, span))
        .collect::<Result<_, _>>()?;
    let variables = env.host.environment.as_ref().ok_or_else(|| RuntimeError {
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
                env,
            )),
        },
        ("exists" | "get" | "require", _) => Err(std_arity("env", op, span)),
        _ => Err(unsupported(&format!("std::env::{op}"), span)),
    }
}

/// Evaluate a `std::log::*` builtin against the host's log capability:
/// `info(message)`, `warn(message)`, or `error(err)`. Each appends one formatted
/// line to the sink and yields nothing. A run with no log capability raises a
/// typed capability error.
fn eval_log(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // Evaluate the arguments before borrowing the sink, so their mutable use of
    // `env` does not overlap the shared read of the capability.
    let values: Vec<Value> = args
        .iter()
        .map(|arg| eval_expr(&arg.value, env))
        .collect::<Result<_, _>>()?;
    let sink = env.host.log.as_ref().ok_or_else(|| RuntimeError {
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
        _ => return Err(unsupported(&format!("std::log::{op}"), span)),
    };
    sink.borrow_mut().push_str(&line);
    Ok(None)
}

/// Evaluate a `std::io::*` file builtin against the host's filesystem capability:
/// `readText`/`writeText`/`readBytes`/`writeBytes`. A run with no filesystem
/// capability raises a typed capability fault; an IO failure (a missing file,
/// permissions) raises a catchable `Error` value the program can `catch`.
fn eval_io(
    op: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // Evaluate the arguments before checking the capability, as the other host
    // modules do.
    let values: Vec<Value> = args
        .iter()
        .map(|arg| eval_expr(&arg.value, env))
        .collect::<Result<_, _>>()?;
    if !env.host.filesystem {
        return Err(RuntimeError {
            code: RUN_CAPABILITY,
            message: format!("this run provides no filesystem capability for `std::io::{op}`"),
            span,
        });
    }
    match (op, values.as_slice()) {
        ("readText", [Value::Str(path)]) => match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(Value::Str(text))),
            Err(error) => Err(raise(io_error("io.read", op, path, &error), span, env)),
        },
        ("writeText", [Value::Str(path), Value::Str(text)]) => match std::fs::write(path, text) {
            Ok(()) => Ok(None),
            Err(error) => Err(raise(io_error("io.write", op, path, &error), span, env)),
        },
        ("readBytes", [Value::Str(path)]) => match std::fs::read(path) {
            Ok(bytes) => Ok(Some(Value::Bytes(bytes))),
            Err(error) => Err(raise(io_error("io.read", op, path, &error), span, env)),
        },
        ("writeBytes", [Value::Str(path), Value::Bytes(data)]) => {
            match std::fs::write(path, data) {
                Ok(()) => Ok(None),
                Err(error) => Err(raise(io_error("io.write", op, path, &error), span, env)),
            }
        }
        ("readText" | "writeText" | "readBytes" | "writeBytes", _) => Err(type_error(
            &format!("`std::io::{op}` got the wrong arguments"),
            span,
        )),
        _ => Err(unsupported(&format!("std::io::{op}"), span)),
    }
}

/// Build a catchable `Error` value (code + message) for a failed `std::io` call.
fn io_error(code: &str, op: &str, path: &str, error: &std::io::Error) -> Value {
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
fn error_field(value: &Value, name: &str) -> Option<String> {
    match value {
        Value::Resource(fields) => fields.iter().find_map(|(field, value)| match value {
            Value::Str(text) if field == name => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    }
}

/// Where control flow stands after a statement or block.
enum Flow {
    /// Fall through to the next statement.
    Normal,
    /// A `return`, carrying its value if it had one.
    Return(Option<Value>),
    /// A `break`, targeting the named loop, or the innermost when unlabeled.
    Break(Option<String>),
    /// A `continue`, targeting the named loop, or the innermost when unlabeled.
    Continue(Option<String>),
    /// A `throw`, carrying the thrown `Error` value, unwinding until a `catch`
    /// handles it or it leaves the function as an uncaught-error fault.
    Throw(Value),
}

/// A name binding: its value and whether it may be reassigned (`var` vs `let`).
struct Binding {
    value: Value,
    mutable: bool,
}

/// The ambient state every activation in a run shares: the checked program (to
/// resolve calls), the saved-data store, and the host capabilities. All three
/// are borrowed for the run's lifetime, so the context is cheap to copy.
#[derive(Clone, Copy)]
struct Context<'p> {
    program: &'p CheckedProgram,
    store: &'p RefCell<dyn Backend>,
    host: &'p Host,
}

/// A lexical environment: a stack of scopes, the ambient run context (program,
/// store, and host capabilities), and the shared output stream (so `print`/
/// `write` from any activation append to one buffer). A resource has few locals,
/// so lookups are linear and innermost-first.
struct Env<'p> {
    scopes: Vec<Vec<(String, Binding)>>,
    program: &'p CheckedProgram,
    store: &'p RefCell<dyn Backend>,
    host: &'p Host,
    output: Rc<RefCell<String>>,
    /// An `Error` thrown by a called function, stashed here so it rides the `Err`
    /// channel (calls are expressions) and a `catch` in this activation can bind
    /// it. Set at the call site, consumed by the nearest `try` or, if none, by
    /// this activation's [`invoke`] when it surfaces the throw to its own caller.
    /// INVARIANT: non-`None` only while an `Err` is actively unwinding a throw —
    /// every `try`/activation boundary that turns the result back into `Ok` clears
    /// it, so a stale throw can never be mistaken for a later fault.
    pending_throw: Option<Value>,
    /// Encoded path prefixes of the saved layers loops are actively traversing,
    /// innermost last. A write/delete/append/merge whose affected layer is in this
    /// set mutates a layer being iterated, which is a [`RUN_TRAVERSAL`] fault.
    traversed_layers: Vec<Vec<u8>>,
}

/// Why an assignment did not land.
enum AssignError {
    Unbound,
    Immutable,
}

impl<'p> Env<'p> {
    fn new(ctx: Context<'p>, output: Rc<RefCell<String>>) -> Self {
        Self {
            scopes: Vec::new(),
            output,
            program: ctx.program,
            store: ctx.store,
            host: ctx.host,
            pending_throw: None,
            traversed_layers: Vec::new(),
        }
    }

    /// Fault if `affected` (an encoded saved-layer prefix) is a layer a loop is
    /// actively traversing. Called before a write/delete/append/merge commits, so
    /// a self-mutating traversal stops before it changes the iterated key set.
    fn guard_traversed_layer(
        &self,
        affected: &[PathSegment],
        span: SourceSpan,
    ) -> Result<(), RuntimeError> {
        let affected = encode_path(affected);
        if self.traversed_layers.iter().any(|layer| layer == &affected) {
            return Err(RuntimeError {
                code: RUN_TRAVERSAL,
                message: "this write changes the saved layer a loop is traversing; \
                          collect the keys into a local sequence first"
                    .into(),
                span,
            });
        }
        Ok(())
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Bind `name` in the innermost scope, shadowing any binding further out.
    fn bind(&mut self, name: String, value: Value, mutable: bool) {
        self.scopes
            .last_mut()
            .expect("a scope is open")
            .push((name, Binding { value, mutable }));
    }

    /// The value bound to `name`, searching innermost scope first.
    fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .find(|(bound, _)| bound == name)
            .map(|(_, binding)| &binding.value)
    }

    /// Reassign an existing mutable binding.
    fn assign(&mut self, name: &str, value: Value) -> Result<(), AssignError> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some((_, binding)) = scope.iter_mut().rev().find(|(bound, _)| bound == name) {
                if !binding.mutable {
                    return Err(AssignError::Immutable);
                }
                binding.value = value;
                return Ok(());
            }
        }
        Err(AssignError::Unbound)
    }
}

/// Evaluate a block in its own scope, stopping at the first `return`. The scope
/// is popped on every exit, including when a statement raises an error, so the
/// environment is left balanced for reuse.
fn eval_block(block: &Block, env: &mut Env<'_>) -> Result<Flow, RuntimeError> {
    env.push_scope();
    let result = eval_statements(&block.statements, env);
    env.pop_scope();
    result
}

/// Evaluate statements in order until one returns or the block ends.
fn eval_statements(statements: &[Statement], env: &mut Env<'_>) -> Result<Flow, RuntimeError> {
    for statement in statements {
        let flow = eval_statement(statement, env)?;
        if !matches!(flow, Flow::Normal) {
            return Ok(flow);
        }
    }
    Ok(Flow::Normal)
}

fn eval_statement(statement: &Statement, env: &mut Env<'_>) -> Result<Flow, RuntimeError> {
    match statement {
        Statement::Const { name, value, .. } => {
            let value = eval_expr(value, env)?;
            env.bind(name.clone(), value, false);
            Ok(Flow::Normal)
        }
        Statement::Var {
            name,
            keys,
            ty,
            value,
            span,
        } => {
            if !keys.is_empty() {
                return Err(unsupported("a keyed local variable", *span));
            }
            let value = match value {
                Some(expr) => eval_expr(expr, env)?,
                // An uninitialized var starts at its type's default — an empty
                // resource, an empty sequence, or a scalar zero — so a declared but
                // unwritten place (e.g. an `out` argument target, the documented
                // `var n: int` then `f(out n)` pattern) is usable before its first
                // assignment.
                None => match ty {
                    Some(ty) if is_resource_type(env.program, &ty.text) => {
                        Value::Resource(Vec::new())
                    }
                    Some(ty) => uninitialized_default(&ty.text).ok_or_else(|| {
                        unsupported("an uninitialized variable of this type", *span)
                    })?,
                    None => return Err(unsupported("an uninitialized variable", *span)),
                },
            };
            env.bind(name.clone(), value, true);
            Ok(Flow::Normal)
        }
        Statement::Assign {
            target,
            value,
            span,
        } => {
            // A dotted field off a saved record is a managed field write; a
            // `^root(key…)` or bare singleton `^root` target is a whole-resource
            // write; a bare name is a local reassignment.
            if let Expression::Field { base, name, .. } = target {
                if is_saved_path(base) {
                    eval_saved_field_write(base, name, value, *span, env)?;
                } else {
                    eval_local_field_set(base, name, value, *span, env)?;
                }
            } else if let Expression::SavedRoot { .. } = target {
                eval_resource_write(target, value, *span, env)?;
            } else if let Expression::Call { callee, args, .. } = target {
                // `^root(key…).layer(key…) = v` (callee is a saved layer field) is a
                // whole-group-entry write; `^root(key…) = v` (callee is the saved
                // root) is a whole-resource write.
                if let Expression::Field { base, name, .. } = callee.as_ref()
                    && is_saved_path(base)
                {
                    eval_group_entry_write(base, name, args, value, *span, env)?;
                } else {
                    eval_resource_write(target, value, *span, env)?;
                }
            } else {
                let name = local_target(target, *span)?;
                let evaluated = eval_expr(value, env)?;
                env.assign(name, evaluated)
                    .map_err(|error| assign_error(name, error, *span))?;
            }
            Ok(Flow::Normal)
        }
        Statement::Delete { path, span } => {
            eval_delete(path, *span, env)?;
            Ok(Flow::Normal)
        }
        Statement::Merge {
            target,
            value,
            span,
        } => {
            // A `.layer` off a saved record is a keyed-layer merge; a bare local
            // name is a merge into a local resource var; a `^root(key…)` target is
            // a whole-resource saved merge.
            if let Expression::Field { base, name, .. } = target
                && is_saved_path(base)
            {
                eval_layer_merge(base, name, value, *span, env)?;
            } else if let Expression::Name { segments, .. } = target
                && let [name] = segments.as_slice()
            {
                eval_local_merge(name, value, *span, env)?;
            } else {
                eval_resource_merge(target, value, *span, env)?;
            }
            Ok(Flow::Normal)
        }
        Statement::Return { value, .. } => {
            let value = value
                .as_ref()
                .map(|expr| eval_expr(expr, env))
                .transpose()?;
            Ok(Flow::Return(value))
        }
        Statement::Expr { value, .. } => {
            // A call statement may invoke a function that returns nothing; only a
            // call in value position requires a return value.
            if let Expression::Call { callee, args, span } = value {
                eval_call(callee, args, *span, env)?;
            } else {
                eval_expr(value, env)?;
            }
            Ok(Flow::Normal)
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if eval_bool(condition, env)? {
                return eval_block(then_block, env);
            }
            for else_if in else_ifs {
                if eval_bool(&else_if.condition, env)? {
                    return eval_block(&else_if.block, env);
                }
            }
            match else_block {
                Some(block) => eval_block(block, env),
                None => Ok(Flow::Normal),
            }
        }
        Statement::Break { label, .. } => Ok(Flow::Break(label.clone())),
        Statement::Continue { label, .. } => Ok(Flow::Continue(label.clone())),
        Statement::While {
            label,
            condition,
            body,
            ..
        } => eval_while(label, condition, body, env),
        Statement::For {
            label,
            binding,
            iterable,
            body,
            span,
        } => eval_for(label, binding, iterable, body, *span, env),
        Statement::Transaction { body, span, .. } => {
            // Open a backend transaction; the backend's savepoint stack handles
            // nesting. Any non-error exit (fall-through, `return`, `break`,
            // `continue`) commits; an escaping error rolls back. Local variables
            // and output already produced are not rewound.
            env.store
                .borrow_mut()
                .begin()
                .map_err(|error| store_error(error, *span))?;
            match eval_block(body, env) {
                // A throw escapes the transaction, so it rolls back like an error
                // rather than committing.
                Ok(Flow::Throw(value)) => {
                    let _ = env.store.borrow_mut().rollback();
                    Ok(Flow::Throw(value))
                }
                Ok(flow) => {
                    env.store
                        .borrow_mut()
                        .commit()
                        .map_err(|error| store_error(error, *span))?;
                    Ok(flow)
                }
                Err(error) => {
                    let _ = env.store.borrow_mut().rollback();
                    Err(error)
                }
            }
        }
        Statement::Throw { value, span } => {
            let thrown = eval_expr(value, env)?;
            // `throw` requires an `Error` value (resource-shaped). The checker does
            // not yet type-check throw operands, so guard here.
            if !matches!(thrown, Value::Resource(_)) {
                return Err(type_error("`throw` requires an `Error` value", *span));
            }
            Ok(Flow::Throw(thrown))
        }
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            let outcome = eval_block(body, env);
            // A throw to handle is raised either directly here (`Ok(Flow::Throw)`)
            // or by a called function (an `Err` with the Error stashed on the env).
            // `catch` handles only thrown Errors; a runtime fault (an `Err` with no
            // pending throw) and other control flow pass through unchanged.
            let thrown = match &outcome {
                Ok(Flow::Throw(value)) => Some(value.clone()),
                Err(_) => env.pending_throw.take(),
                _ => None,
            };
            let handled = match (thrown, catch) {
                (Some(error), Some(clause)) => {
                    env.push_scope();
                    env.bind(clause.name.clone(), error, false);
                    let caught = eval_block(&clause.block, env);
                    env.pop_scope();
                    caught
                }
                // No `catch`: the throw keeps unwinding. A throw propagated from a
                // call (an `Err`) must keep its Error stashed for an outer handler.
                // (A present `finally` immediately reclaims this via its own take;
                // the stash is what carries the throw when there is no `finally`.)
                (Some(error), None) => {
                    if outcome.is_err() {
                        env.pending_throw = Some(error);
                    }
                    outcome
                }
                (None, _) => outcome,
            };
            // `finally` always runs. A throwing or faulting finally replaces the
            // pending outcome; a normal one is cleanup and the outcome proceeds.
            // (The checker forbids return/break/continue in `finally`.)
            match finally {
                Some(block) => {
                    // Take any throw `handled` left pending so it cannot leak past
                    // `finally`. A clean `finally` restores it (the outcome
                    // proceeds); a `finally` that throws or faults replaces the
                    // outcome, so the stashed throw is dropped and `finally`'s own
                    // pending throw (set by a call it made) stands.
                    let pending = env.pending_throw.take();
                    match eval_block(block, env) {
                        Ok(Flow::Throw(error)) => Ok(Flow::Throw(error)),
                        Err(error) => Err(error),
                        Ok(_) => {
                            env.pending_throw = pending;
                            handled
                        }
                    }
                }
                None => handled,
            }
        }
        Statement::Lock { body, .. } => {
            // A single-writer capability profile holds no contended lock, so there
            // is no lock state to acquire or release: `lock` is just its body. The
            // body runs in `eval_block`, which pops its scope on every exit
            // (including errors). The target path only matters for coordinating
            // concurrent writers, so it is not read here.
            eval_block(body, env)
        }
        other => Err(unsupported("this statement", other.span())),
    }
}

/// How a loop body's resulting flow affects a loop labelled `label`.
enum LoopStep {
    /// Run the next iteration (the body fell through, or `continue`d this loop).
    Iterate,
    /// Stop the loop (a `break` targeting this loop).
    Stop,
    /// Leave the loop carrying an outward jump: a `return`, or a `break` /
    /// `continue` aimed at an enclosing loop.
    Propagate(Flow),
}

/// Classify a loop body's flow for a loop labelled `label`.
fn classify(flow: Flow, label: &Option<String>) -> LoopStep {
    match flow {
        Flow::Normal => LoopStep::Iterate,
        Flow::Continue(ref target) if targets_this_loop(target, label) => LoopStep::Iterate,
        Flow::Break(ref target) if targets_this_loop(target, label) => LoopStep::Stop,
        other => LoopStep::Propagate(other),
    }
}

/// Whether a `break`/`continue` carrying `jump_label` targets a loop labelled
/// `loop_label`: an unlabelled jump targets the innermost (this) loop; a
/// labelled jump targets only the loop with the matching label.
fn targets_this_loop(jump_label: &Option<String>, loop_label: &Option<String>) -> bool {
    match jump_label {
        None => true,
        Some(name) => loop_label.as_deref() == Some(name.as_str()),
    }
}

fn eval_while(
    label: &Option<String>,
    condition: &Expression,
    body: &Block,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    while eval_bool(condition, env)? {
        match classify(eval_block(body, env)?, label) {
            LoopStep::Iterate => {}
            LoopStep::Stop => break,
            LoopStep::Propagate(flow) => return Ok(flow),
        }
    }
    Ok(Flow::Normal)
}

/// Run `loop_body` with `prefix` marked as an actively-traversed saved layer (if
/// any), popping it afterward whatever the body returns, so a self-mutating write
/// inside the loop is caught by [`Env::guard_traversed_layer`] and the guard never
/// outlives the loop.
fn iterate_saved_layer(
    prefix: Option<Vec<PathSegment>>,
    env: &mut Env<'_>,
    loop_body: impl FnOnce(&mut Env<'_>) -> Result<Flow, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    let pushed = prefix.is_some();
    if let Some(prefix) = prefix {
        env.traversed_layers.push(encode_path(&prefix));
    }
    let result = loop_body(env);
    if pushed {
        env.traversed_layers.pop();
    }
    result
}

fn eval_for(
    label: &Option<String>,
    binding: &ForBinding,
    iterable: &Expression,
    body: &Block,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    // A two-name binding (`for k, v in entries(...)`) iterates `[key, value]`
    // pairs; ranges have no second name to bind.
    if let Some(second) = &binding.second {
        if matches!(
            iterable,
            Expression::Binary {
                op: BinaryOp::RangeExclusive | BinaryOp::RangeInclusive,
                ..
            }
        ) {
            return Err(unsupported("a two-name binding over a range", span));
        }
        let entries = eval_collection(iterable, env)?;
        let prefix = traversed_layer_prefix(iterable, env)?;
        return iterate_saved_layer(prefix, env, |env| {
            for entry in entries {
                let Value::Sequence(pair) = entry else {
                    return Err(unsupported(
                        "a two-name binding over a non-pair iterable (use entries(...))",
                        span,
                    ));
                };
                let [key, value] = <[Value; 2]>::try_from(pair).map_err(|_| {
                    unsupported(
                        "a two-name binding over a non-pair iterable (use entries(...))",
                        span,
                    )
                })?;
                env.push_scope();
                env.bind(binding.first.clone(), key, false);
                env.bind(second.clone(), value, false);
                let flow = eval_block(body, env);
                env.pop_scope();
                match classify(flow?, label) {
                    LoopStep::Iterate => {}
                    LoopStep::Stop => break,
                    LoopStep::Propagate(flow) => return Ok(flow),
                }
            }
            Ok(Flow::Normal)
        });
    }
    // A non-range iterable (e.g. `keys(^books.byShelf("x"))`) materializes to a
    // sequence of values, which the loop binds one at a time.
    if !matches!(
        iterable,
        Expression::Binary {
            op: BinaryOp::RangeExclusive | BinaryOp::RangeInclusive,
            ..
        }
    ) {
        let values = eval_collection(iterable, env)?;
        let prefix = traversed_layer_prefix(iterable, env)?;
        return iterate_saved_layer(prefix, env, |env| {
            for value in values {
                env.push_scope();
                env.bind(binding.first.clone(), value, false);
                let flow = eval_block(body, env);
                env.pop_scope();
                match classify(flow?, label) {
                    LoopStep::Iterate => {}
                    LoopStep::Stop => break,
                    LoopStep::Propagate(flow) => return Ok(flow),
                }
            }
            Ok(Flow::Normal)
        });
    }
    let (start, end, inclusive) = range_bounds(iterable, env)?;
    let mut current = start;
    while if inclusive {
        current <= end
    } else {
        current < end
    } {
        // Each iteration binds the loop variable in a fresh scope.
        env.push_scope();
        env.bind(binding.first.clone(), Value::Int(current), false);
        let flow = eval_block(body, env);
        env.pop_scope();
        match classify(flow?, label) {
            LoopStep::Iterate => {}
            LoopStep::Stop => break,
            LoopStep::Propagate(flow) => return Ok(flow),
        }
        // Stop rather than overflow when the endpoint reaches `i64::MAX`.
        match current.checked_add(1) {
            Some(next) => current = next,
            None => break,
        }
    }
    Ok(Flow::Normal)
}

/// The `(start, end, inclusive)` bounds of a range iterable. Only integer ranges
/// (`a..b`, `a..=b`) are iterable in this slice; other iterables are unsupported.
fn range_bounds(
    iterable: &Expression,
    env: &mut Env<'_>,
) -> Result<(i64, i64, bool), RuntimeError> {
    match iterable {
        Expression::Binary {
            op: BinaryOp::RangeExclusive,
            left,
            right,
            ..
        } => Ok((eval_int(left, env)?, eval_int(right, env)?, false)),
        Expression::Binary {
            op: BinaryOp::RangeInclusive,
            left,
            right,
            ..
        } => Ok((eval_int(left, env)?, eval_int(right, env)?, true)),
        other => Err(unsupported("iterating this value", other.span())),
    }
}

/// Materialize a non-range `for` iterable to a sequence of values. A saved-layer
/// path — a primary root `^books`, an index branch `^books.byShelf("x")`, or a
/// keyed/sequence child layer `^books(id).tags` — and `keys(<layer>)` of one both
/// enumerate the layer's child keys through [`enumerate_layer`]. Every other
/// iterable must evaluate to an in-memory sequence (e.g. `std::text::split(...)`).
fn eval_collection(iterable: &Expression, env: &mut Env<'_>) -> Result<Vec<Value>, RuntimeError> {
    if let Some(path) = keys_argument(iterable) {
        return enumerate_layer(path, env);
    }
    if is_saved_path(iterable) {
        return enumerate_layer(iterable, env);
    }
    match eval_expr(iterable, env)? {
        Value::Sequence(items) => Ok(items),
        _ => Err(unsupported("iterating this value", iterable.span())),
    }
}

/// The encoded path prefix of the saved layer a `for` iterable traverses, or
/// `None` for a range or a local value (which traverse no saved layer). A saved
/// layer is traversed only when the iterable is a saved path directly or wrapped
/// in `keys`/`values`/`entries`; iterating a local — the "collect keys first"
/// pattern — has no saved layer to guard. The prefix is the path whose child keys
/// the loop walks: `[Root]` for a primary root, `[Root, Index, IndexKey…]` for an
/// index branch, `[Root, RecordKey…, ChildLayer]` for a keyed/sequence layer. It
/// matches the prefix [`enumerate_layer`] reads children under, so a mutation that
/// changes that layer is caught by [`Env::guard_traversed_layer`].
fn traversed_layer_prefix(
    iterable: &Expression,
    env: &mut Env<'_>,
) -> Result<Option<Vec<PathSegment>>, RuntimeError> {
    let path = traversal_argument(iterable).unwrap_or(iterable);
    if !is_saved_path(path) {
        return Ok(None);
    }
    match path {
        Expression::SavedRoot { name, .. } => Ok(Some(vec![PathSegment::Root(name.clone())])),
        // An index branch `^root.index(args…)`: the prefix is the root, index name,
        // and the supplied index-key args (the levels below are reconstructed
        // identities, so the traversed layer is the branch the args reach).
        Expression::Call { callee, args, span } if matches!(callee.as_ref(), Expression::Field { base, .. } if matches!(base.as_ref(), Expression::SavedRoot { .. })) =>
        {
            let Expression::Field {
                base, name: index, ..
            } = callee.as_ref()
            else {
                return Ok(None);
            };
            let Expression::SavedRoot { name: root, .. } = base.as_ref() else {
                return Ok(None);
            };
            let mut prefix = vec![
                PathSegment::Root(root.clone()),
                PathSegment::Index(index.clone()),
            ];
            for arg in args {
                prefix.push(PathSegment::IndexKey(
                    value_to_key(eval_expr(&arg.value, env)?)
                        .ok_or_else(|| unsupported("an index key of this type", *span))?,
                ));
            }
            Ok(Some(prefix))
        }
        // A keyed/sequence child layer `^root(id…).layer`.
        Expression::Field {
            base, name: layer, ..
        } => {
            let (root, identity) = lower_record_identity(base, env)?;
            let mut prefix = vec![PathSegment::Root(root)];
            prefix.extend(identity.into_iter().map(PathSegment::RecordKey));
            prefix.push(PathSegment::ChildLayer(layer.clone()));
            Ok(Some(prefix))
        }
        _ => Ok(None),
    }
}

/// The sole argument of a `keys`/`values`/`entries` call, or `None` for any other
/// expression. These wrap a saved layer without changing which layer is traversed.
fn traversal_argument(expr: &Expression) -> Option<&Expression> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.len() != 1 || !matches!(segments[0].as_str(), "keys" | "values" | "entries") {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// The single argument of a `keys(<path>)` call, or `None` for any other
/// expression. Shared by the loop materializer and the standalone `keys` builtin.
fn keys_argument(expr: &Expression) -> Option<&Expression> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.len() != 1 || segments[0] != "keys" {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// Enumerate the child keys of a saved layer as the values a `for` loop binds or
/// `keys(...)` materializes. Classifies the path once and descends one shared
/// key-collector ([`collect_child_identities`]):
///
/// - `^root` (a keyed primary root) yields its record identities — a bare key
///   value for a single-key identity, a [`Value::Identity`] for a composite one;
///   a keyless singleton has no identities to iterate (a type error).
/// - `^root.index(args…)` yields the identities in that index branch.
/// - `^root(id…).layer` yields the keyed/sequence layer's child keys.
fn enumerate_layer(path: &Expression, env: &mut Env<'_>) -> Result<Vec<Value>, RuntimeError> {
    match path {
        // A primary keyed root: its immediate children are the record-key segments
        // of the (possibly composite) identity. A keyless singleton has none.
        Expression::SavedRoot { name, span } => {
            let arity = match root_identity_arity(env.program, name) {
                Some(0) => {
                    return Err(type_error(
                        &format!("`^{name}` is a singleton with no identities to iterate"),
                        *span,
                    ));
                }
                Some(arity) => arity,
                None => return Err(unsupported("iterating this saved path", *span)),
            };
            let prefix = vec![PathSegment::Root(name.clone())];
            collect_child_identities(&prefix, arity, &[], PathSegment::RecordKey, *span, env)
        }
        // An index branch `^root.index(args…)` (a `Call` whose callee is a `.index`
        // off a saved root) or a keyed/sequence child layer `^root(id…).layer`.
        Expression::Call { callee, args, span } if matches!(callee.as_ref(), Expression::Field { base, .. } if matches!(base.as_ref(), Expression::SavedRoot { .. })) => {
            enumerate_index_branch(callee, args, *span, env)
        }
        Expression::Field { .. } => enumerate_child_layer(path, env),
        other => Err(unsupported("iterating this saved path", other.span())),
    }
}

/// Enumerate the identities in a declared index branch `^root.index(args…)`. A
/// non-unique index ends with all identity keys, so the levels below the supplied
/// query args are the entry's remaining identity-key segments; descend them per
/// entry to reconstruct the full identity rather than only its first key component.
fn enumerate_index_branch(
    callee: &Expression,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let Expression::Field {
        base, name: index, ..
    } = callee
    else {
        return Err(unsupported("iterating this saved path", span));
    };
    let Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return Err(unsupported("iterating this saved path", span));
    };
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "an index lookup with named or out arguments",
            span,
        ));
    }
    let mut prefix = vec![
        PathSegment::Root(root.clone()),
        PathSegment::Index(index.clone()),
    ];
    for arg in args {
        prefix.push(PathSegment::IndexKey(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", span))?,
        ));
    }
    let schema = find_resource(env.program, root)
        .and_then(|resource| resource.indexes.iter().find(|i| &i.name == index))
        .ok_or_else(|| unsupported("iterating this saved path", span))?;
    let depth = schema.args.len().saturating_sub(args.len());
    collect_child_identities(&prefix, depth, &[], PathSegment::IndexKey, span, env)
}

/// Enumerate the child keys of a keyed/sequence child layer `^root(id…).layer`.
/// The layer's keys are single-key (`pos: int` for a sequence, `playerId: string`
/// for a keyed tree), so each child key is a bare value.
fn enumerate_child_layer(path: &Expression, env: &mut Env<'_>) -> Result<Vec<Value>, RuntimeError> {
    let Expression::Field {
        base, name: layer, ..
    } = path
    else {
        return Err(unsupported("iterating this saved path", path.span()));
    };
    let span = path.span();
    let (root, identity) = lower_record_identity(base, env)?;
    let mut prefix = vec![PathSegment::Root(root)];
    prefix.extend(identity.into_iter().map(PathSegment::RecordKey));
    prefix.push(PathSegment::ChildLayer(layer.clone()));
    collect_child_identities(&prefix, 1, &[], PathSegment::IndexKey, span, env)
}

/// Collect the identities reachable below `prefix`, descending `depth` remaining
/// key levels. `make_segment` builds the [`PathSegment`] for each descent step —
/// `RecordKey` below a primary root, `IndexKey` below an index branch or child
/// layer. `keys` accumulates the key segments gathered so far. At the final level
/// each entry yields one identity: a single key value (renderable, addresses
/// `^root(key)`) for a single-key identity, or a [`Value::Identity`] for a
/// composite one.
fn collect_child_identities(
    prefix: &[PathSegment],
    depth: usize,
    keys: &[SavedKey],
    make_segment: fn(SavedKey) -> PathSegment,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let children = {
        let store = env.store.borrow();
        store
            .child_keys(&encode_path(prefix))
            .map_err(|_| RuntimeError {
                code: RUN_STORE,
                message: "could not read the keys at this path".into(),
                span,
            })?
    };
    let mut values = Vec::new();
    for child in children {
        let ChildSegment::Key(key) = child else {
            continue;
        };
        let mut keys = keys.to_vec();
        keys.push(key.clone());
        if depth <= 1 {
            // The last key level: a single-key identity stays a raw key value; a
            // composite one reconstructs its full `Value::Identity`.
            values.push(if keys.len() == 1 {
                saved_key_to_value(key)
                    .ok_or_else(|| unsupported("iterating keys of this type", span))?
            } else {
                Value::Identity(keys)
            });
        } else {
            let mut prefix = prefix.to_vec();
            prefix.push(make_segment(key));
            values.extend(collect_child_identities(
                &prefix,
                depth - 1,
                &keys,
                make_segment,
                span,
                env,
            )?);
        }
    }
    Ok(values)
}

/// Convert a child key to a runtime value, or `None` for a key type the runtime
/// does not yet represent.
fn saved_key_to_value(key: SavedKey) -> Option<Value> {
    match key {
        SavedKey::Int(n) => Some(Value::Int(n)),
        SavedKey::Bool(b) => Some(Value::Bool(b)),
        SavedKey::Str(s) => Some(Value::Str(s)),
        SavedKey::Bytes(b) => Some(Value::Bytes(b)),
        _ => None,
    }
}

/// The single local name an assignment targets, or an "unsupported" error for a
/// saved path or qualified name (those arrive with later slices).
fn local_target(target: &Expression, span: SourceSpan) -> Result<&str, RuntimeError> {
    match target {
        Expression::Name { segments, .. } if segments.len() == 1 => Ok(&segments[0]),
        _ => Err(unsupported("assignment to this target", span)),
    }
}

fn eval_expr(expr: &Expression, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
    match expr {
        Expression::Literal { kind, text, span } => eval_literal(*kind, text, *span),
        Expression::Name { segments, span } => {
            if segments.len() != 1 {
                return Err(unsupported("a qualified name", *span));
            }
            env.lookup(&segments[0])
                .cloned()
                .ok_or_else(|| RuntimeError {
                    code: RUN_UNBOUND_NAME,
                    message: format!("`{}` is not bound", segments[0]),
                    span: *span,
                })
        }
        Expression::Unary { op, operand, span } => eval_unary(*op, operand, *span, env),
        Expression::Binary {
            op,
            left,
            right,
            span,
        } => eval_binary(*op, left, right, *span, env),
        Expression::Call { callee, args, span } => match eval_call(callee, args, *span, env)? {
            Some(value) => Ok(value),
            None => Err(RuntimeError {
                code: RUN_NO_VALUE,
                message: "a call to a function that returns no value cannot be used as a value"
                    .into(),
                span: *span,
            }),
        },
        Expression::Interpolation { parts, span } => eval_interpolation(parts, *span, env),
        // A dotted field read: off a saved root (`^books(id).title`) it is a
        // saved read; off a local it reads the resource value's field.
        Expression::Field {
            base, name, span, ..
        } => {
            if is_saved_path(base) {
                eval_saved_field(expr, env)
            } else {
                eval_local_field_get(base, name, *span, env)
            }
        }
        // A bare saved root read (`^settings`) is a whole-resource read of a
        // keyless singleton; a keyed root needs a `^root(key…)` call.
        Expression::SavedRoot { name, span, .. } => read_resource(name, &[], *span, env),
        other => Err(unsupported("this expression", other.span())),
    }
}

/// Read a scalar field off a saved record, e.g. `^books(id).title`. Lowers the
/// path to encoded segments, reads the store, and decodes the bytes with the
/// field's declared type from the resource schema. A group-entry target
/// `^root(key…).layer(key…).field` is dispatched to [`eval_group_field_read`].
/// An unpopulated element is an absent-element error.
fn eval_saved_field(expr: &Expression, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
    let Expression::Field { base, name, .. } = expr else {
        return Err(unsupported("this read", expr.span()));
    };
    // A field reached through one or more group layers reads inside that group:
    // a keyed GROUP entry `^root(id…).layer(key…)….field` (a layer call whose
    // callee is a `.layer` access), or an unkeyed group `^root(id…).name.field`
    // (a `.field` off a `.field` of the record). A plain `^root(id…).field` base
    // is a top-level field read.
    if is_group_base(base) {
        return eval_group_field_read(base, name, expr.span(), env);
    }
    let (root, identity) = lower_record_identity(base, env)?;
    let value = read_saved_field(&root, &identity, name, expr.span(), env);
    catchable_read(value, env)
}

/// Read a top-level saved scalar field from a pre-lowered identity, decoding it
/// with the field's declared type. An unpopulated element is an absent-element
/// error. Shared by [`eval_saved_field`] and `out`/`inout` place reads.
fn read_saved_field(
    root: &str,
    identity: &[SavedKey],
    field: &str,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut segments = vec![PathSegment::Root(root.to_string())];
    segments.extend(identity.iter().cloned().map(PathSegment::RecordKey));
    segments.push(PathSegment::Field(field.to_string()));
    let field_type = resource_field_type(env.program, root, field)
        .ok_or_else(|| unsupported("reading this field", span))?;
    let store = env.store.borrow();
    let Some(bytes) = store
        .read(&encode_path(&segments))
        .map_err(|error| store_error(error, span))?
    else {
        return Err(RuntimeError {
            code: RUN_ABSENT,
            message: format!("`{field}` is absent"),
            span,
        });
    };
    decode_value(&bytes, field_type)
        .and_then(saved_value_to_value)
        .ok_or_else(|| RuntimeError {
            code: RUN_TYPE,
            message: format!("stored value for `{field}` did not decode to a runtime value"),
            span,
        })
}

/// Read a field inside a keyed GROUP entry at any nesting depth, e.g.
/// `^books(id).versions(v).comments(c).text`. `base` is the group-entry path; it
/// is lowered to the record identity and the chain of layer levels, the store is
/// read, and the value is decoded with the innermost member's declared type; an
/// absent entry is an absent-element error.
fn eval_group_field_read(
    base: &Expression,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let (root, identity, layers) = lower_layer_path(base, env)?;
    let value = read_nested_field(&root, &identity, &layers, field, span, env);
    catchable_read(value, env)
}

/// Read a scalar field inside a (possibly nested) keyed group entry from already-
/// lowered path parts. Shared by [`eval_group_field_read`] and an `inout` place
/// read; the value decodes with the innermost member's declared type, and an
/// unpopulated entry is an absent-element error.
fn read_nested_field(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    field: &str,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    let layer_names: Vec<&str> = layers.iter().map(|(name, _)| name.as_str()).collect();
    let member_type = resource_nested_member_type(env.program, root, &layer_names, field)
        .ok_or_else(|| unsupported("reading this group field", span))?;
    let mut segments = vec![PathSegment::Root(root.to_string())];
    segments.extend(identity.iter().cloned().map(PathSegment::RecordKey));
    for (name, keys) in layers {
        segments.push(PathSegment::ChildLayer(name.clone()));
        segments.extend(keys.iter().cloned().map(PathSegment::IndexKey));
    }
    segments.push(PathSegment::Field(field.to_string()));
    let store = env.store.borrow();
    let Some(bytes) = store
        .read(&encode_path(&segments))
        .map_err(|error| store_error(error, span))?
    else {
        return Err(RuntimeError {
            code: RUN_ABSENT,
            message: format!("`{field}` entry is absent"),
            span,
        });
    };
    decode_value(&bytes, member_type)
        .and_then(saved_value_to_value)
        .ok_or_else(|| RuntimeError {
            code: RUN_TYPE,
            message: format!("stored value for `{field}` did not decode to a runtime value"),
            span,
        })
}

/// Read a resource identity from a declared index lookup `^root.index(args…)`.
/// A unique index stores the owning identity at the lookup path, so reading it
/// decodes back to a [`Value::Identity`]. A non-unique index has no single
/// identity to yield in value position; iterate it with `keys(...)` instead.
fn eval_index_lookup(
    resource: &ResourceSchema,
    index: &IndexSchema,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if !index.unique {
        return Err(RuntimeError {
            code: RUN_UNSUPPORTED,
            message: format!(
                "non-unique index `{}` has no single identity in value position; \
                 iterate it with `keys(...)`",
                index.name
            ),
            span,
        });
    }
    // A unique index points to one resource, so `decode_identity` needs the
    // resource's saved root to know the identity arity.
    let root = resource
        .saved_root
        .as_ref()
        .ok_or_else(|| unsupported("an index on a resource with no saved root", span))?;
    let mut segments = vec![
        PathSegment::Root(root.root.clone()),
        PathSegment::Index(index.name.clone()),
    ];
    for arg in args {
        if arg.mode.is_some() || arg.name.is_some() {
            return Err(unsupported(
                "an index lookup with named or out arguments",
                span,
            ));
        }
        segments.push(PathSegment::IndexKey(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", span))?,
        ));
    }
    let store = env.store.borrow();
    let bytes = store
        .read(&encode_path(&segments))
        .map_err(|error| store_error(error, span))?
        .ok_or_else(|| RuntimeError {
            code: RUN_ABSENT,
            message: format!("`{}` has no entry for that key", index.name),
            span,
        })?;
    decode_identity(&bytes, root)
        .map(Value::Identity)
        .ok_or_else(|| RuntimeError {
            code: RUN_TYPE,
            message: format!(
                "the `{}` index entry did not decode to an identity",
                index.name
            ),
            span,
        })
}

/// Read a keyed-layer entry off a saved record. A leaf layer
/// (`^books(id).tags(pos)`) reads its single value; a group layer
/// (`^books(id).versions(v)`) materializes the whole entry. The `callee` is the
/// layer field `^books(id).<layer>` and `keys` are the layer key arguments.
fn eval_saved_layer_read(
    callee: &Expression,
    keys: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Expression::Field {
        base, name: layer, ..
    } = callee
    else {
        return Err(unsupported("this read", span));
    };
    let (root, identity) = lower_record_identity(base, env)?;
    let layer_keys = lower_layer_keys(keys, span, env)?;
    let value = read_layer_entry(&root, &identity, layer, &layer_keys, span, env);
    catchable_read(value, env)
}

/// Read one keyed-layer entry from a lowered record identity and layer keys. A
/// leaf layer reads its single decoded value; a group layer materializes its
/// entry as a [`Value::Resource`]. Shared by [`eval_saved_layer_read`] and the
/// `values`/`entries` materializer.
fn read_layer_entry(
    root: &str,
    identity: &[SavedKey],
    layer: &str,
    layer_keys: &[SavedKey],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut entry = vec![PathSegment::Root(root.to_string())];
    entry.extend(identity.iter().cloned().map(PathSegment::RecordKey));
    entry.push(PathSegment::ChildLayer(layer.to_string()));
    entry.extend(layer_keys.iter().cloned().map(PathSegment::IndexKey));

    // A leaf layer reads one value; a group layer materializes its entry.
    let Some(leaf_type) = resource_layer_leaf_type(env.program, root, layer) else {
        return read_group_entry(root, layer, &entry, span, env);
    };
    let store = env.store.borrow();
    let Some(bytes) = store
        .read(&encode_path(&entry))
        .map_err(|error| store_error(error, span))?
    else {
        return Err(RuntimeError {
            code: RUN_ABSENT,
            message: format!("`{layer}` entry is absent"),
            span,
        });
    };
    decode_value(&bytes, leaf_type)
        .and_then(saved_value_to_value)
        .ok_or_else(|| RuntimeError {
            code: RUN_TYPE,
            message: format!("stored value in `{layer}` did not decode to a runtime value"),
            span,
        })
}

/// Materialize a keyed GROUP entry `^root(key…).layer(key…)` (its path already
/// lowered into `entry`) as a [`Value::Resource`]: each present member field, in
/// declaration order, decoded by its type; sparse members are omitted. Mirrors a
/// whole-resource read scoped to one group entry.
fn read_group_entry(
    root: &str,
    layer: &str,
    entry: &[PathSegment],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let members = resource_group_members(env.program, root, layer)
        .ok_or_else(|| unsupported("reading this layer", span))?;
    let store = env.store.borrow();
    let mut fields = Vec::new();
    for (name, value_type) in members {
        let mut segments = entry.to_vec();
        segments.push(PathSegment::Field(name.clone()));
        let Some(bytes) = store
            .read(&encode_path(&segments))
            .map_err(|error| store_error(error, span))?
        else {
            continue;
        };
        let value = decode_value(&bytes, value_type)
            .and_then(saved_value_to_value)
            .ok_or_else(|| RuntimeError {
                code: RUN_TYPE,
                message: format!("stored value for `{name}` did not decode to a runtime value"),
                span,
            })?;
        fields.push((name, value));
    }
    Ok(Value::Resource(fields))
}

/// Read a whole resource `^root(key…)` into a materialized [`Value::Resource`]:
/// each present top-level field, in schema order, decoded by its declared type.
/// Absent (sparse) fields are simply omitted.
fn eval_resource_read(
    callee: &Expression,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Expression::SavedRoot { name: root, .. } = callee else {
        return Err(unsupported("this read", span));
    };
    let identity = lower_identity_args(args, span, env)?;
    read_resource(root, &identity, span, env)
}

/// Read a whole resource from a pre-lowered identity into a materialized
/// [`Value::Resource`]: each present top-level field in schema order, decoded by
/// its type; sparse fields omitted. Shared by [`eval_resource_read`] and
/// `out`/`inout` place reads.
fn read_resource(
    root: &str,
    identity: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("reading this saved root", span))?;
    let arity = resource
        .saved_root
        .as_ref()
        .map_or(0, |saved| saved.identity_keys.len());
    if identity.len() != arity {
        // A whole-resource read needs the root's full identity: a keyed root such
        // as `^books` is a collection of records, not a readable value on its own.
        return Err(type_error(
            &format!(
                "`^{root}` expects {arity} identity key(s), got {}",
                identity.len()
            ),
            span,
        ));
    }
    if declares_unkeyed_group(resource) {
        return Err(unsupported(
            "a whole-resource read of a resource with an unkeyed nested group \
             (it would silently omit the group's fields)",
            span,
        ));
    }
    let mut prefix = vec![PathSegment::Root(root.to_string())];
    prefix.extend(identity.iter().cloned().map(PathSegment::RecordKey));

    let store = env.store.borrow();
    let mut fields = Vec::new();
    for field in &resource.fields {
        let mut segments = prefix.clone();
        segments.push(PathSegment::Field(field.name.clone()));
        let Some(bytes) = store
            .read(&encode_path(&segments))
            .map_err(|error| store_error(error, span))?
        else {
            continue;
        };
        let value_type = ValueType::from_scalar_name(&field.ty.text)
            .ok_or_else(|| unsupported("reading this field type", span))?;
        let value = decode_value(&bytes, value_type)
            .and_then(saved_value_to_value)
            .ok_or_else(|| RuntimeError {
                code: RUN_TYPE,
                message: format!("stored value for `{}` did not decode", field.name),
                span,
            })?;
        fields.push((field.name.clone(), value));
    }
    Ok(Value::Resource(fields))
}

/// Apply a managed field write `^root(key…).field = value`. Lowers the identity,
/// evaluates the value, and drives [`marrow_write::plan_field_write`] — which
/// validates the field and value and keeps generated indexes coherent — then
/// commits the plan to the store. A planning failure surfaces with its `write.*`
/// code. A group-entry target `^root(key…).layer(key…).field = value` is
/// dispatched to [`eval_group_field_write`].
fn eval_saved_field_write(
    base: &Expression,
    field: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    // A field reached through one or more group layers writes inside that group:
    // a keyed GROUP entry `^root(id…).layer(key…)….field = v`, or an unkeyed group
    // `^root(id…).name.field = v`. A plain `^root(id…).field` base is a top-level
    // field write.
    if is_group_base(base) {
        return eval_group_field_write(base, field, value, span, env);
    }
    let (root, identity) = lower_record_identity(base, env)?;
    let value = eval_expr(value, env)?;
    write_saved_field(&root, &identity, field, value, span, env)
}

/// Apply a managed top-level field write from a pre-lowered identity and an
/// already-evaluated value, driving [`marrow_write::plan_field_write`] and
/// committing. Shared by [`eval_saved_field_write`] and `out`/`inout` write-back.
fn write_saved_field(
    root: &str,
    identity: &[SavedKey],
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("writing to this saved root", span))?;
    let saved = value_to_saved(value)
        .ok_or_else(|| unsupported("writing a resource value to a field", span))?;
    let plan = {
        let store = env.store.borrow();
        plan_field_write(resource, identity, field, &saved, &*store)
    };
    let plan = plan.map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a managed group-entry field write
/// `^root(key…).layer(key…)….field = value`: a single-field update inside a keyed
/// GROUP entry at any nesting depth (e.g. `^books(id).versions(v).comments(c).text`),
/// leaving the entry's other members in place. `base` is the group-entry path; it
/// is lowered to the record identity and the chain of layer levels, then drives
/// [`marrow_write::plan_nested_field_write`] and commits. Generated indexes do not
/// span keyed child layers, so there is no index interaction.
fn eval_group_field_write(
    base: &Expression,
    field: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity, layers) = lower_layer_path(base, env)?;
    let value = eval_expr(value, env)?;
    write_nested_field(&root, &identity, &layers, field, value, span, env)
}

/// Write `value` to a scalar field inside a (possibly nested) keyed group entry
/// from already-lowered path parts. Shared by [`eval_group_field_write`] and an
/// `out`/`inout` place write. Groups carry no generated indexes, so this is a
/// plain replace-in-place write.
fn write_nested_field(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("writing to this saved root", span))?;
    let saved = value_to_saved(value)
        .ok_or_else(|| unsupported("writing a resource value to a field", span))?;
    let layer_refs: Vec<(&str, &[SavedKey])> = layers
        .iter()
        .map(|(name, keys)| (name.as_str(), keys.as_slice()))
        .collect();
    let plan = plan_nested_field_write(resource, identity, &layer_refs, field, &saved)
        .map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a whole keyed-group-entry write `^root(key…).layer(key…) = value`, where
/// `value` is a materialized [`Value::Resource`]. Lowers its fields to a
/// `ResourceValue` and drives [`marrow_write::plan_layer_group_write`] (replace
/// semantics for the one entry), then commits. Groups carry no generated indexes.
fn eval_group_entry_write(
    record: &Expression,
    layer: &str,
    keys: &[Argument],
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity) = lower_record_identity(record, env)?;
    // A keyed-entry write adds/replaces a key in this layer's key set.
    env.guard_traversed_layer(&layer_prefix(&root, &identity, layer), span)?;
    // A declared keyed LEAF (e.g. `tags(pos: int): string`) takes a scalar value
    // written at the keyed path, sharing marrow-write's keyed-leaf write path with
    // `append`. A keyed GROUP takes a whole-entry resource value.
    if resource_layer_leaf_type(env.program, &root, layer).is_some() {
        let saved = value_to_saved(eval_expr(value, env)?)
            .ok_or_else(|| unsupported("writing a resource value to a keyed leaf", span))?;
        let resource = find_resource(env.program, &root)
            .ok_or_else(|| unsupported("writing to this saved root", span))?;
        let layer_keys = lower_layer_keys(keys, span, env)?;
        let plan = plan_layer_leaf_write(resource, &identity, layer, &layer_keys, &saved)
            .map_err(|error| write_fault(error, span, env))?;
        plan.commit(&mut *env.store.borrow_mut())
            .map_err(|error| store_error(error, span))?;
        return Ok(());
    }
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported(
            "assigning a non-resource value to a group entry",
            span,
        ));
    };
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("writing to this saved root", span))?;
    let layer_keys = lower_layer_keys(keys, span, env)?;
    let value = resource_value_of(fields, span)?;
    let plan = plan_layer_group_write(resource, &identity, layer, &layer_keys, &value)
        .map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a whole-resource write `^root(key…) = value`, where `value` is a
/// materialized [`Value::Resource`]. Lowers its present fields to a
/// `ResourceValue` and drives [`marrow_write::plan_resource_write`] (replace
/// semantics, keeping generated indexes coherent), then commits.
fn eval_resource_write(
    target: &Expression,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity) = lower_record_identity(target, env)?;
    let value = eval_expr(value, env)?;
    write_resource(&root, &identity, value, span, env)
}

/// Apply a whole-resource write from a pre-lowered identity and an
/// already-evaluated [`Value::Resource`], driving
/// [`marrow_write::plan_resource_write`] (replace semantics) and committing.
/// Shared by [`eval_resource_write`] and `out`/`inout` write-back.
fn write_resource(
    root: &str,
    identity: &[SavedKey],
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Value::Resource(fields) = value else {
        return Err(unsupported(
            "assigning a non-resource value to a saved record",
            span,
        ));
    };
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("writing this saved root", span))?;
    if declares_unkeyed_group(resource) {
        return Err(unsupported(
            "a whole-resource write of a resource with an unkeyed nested group \
             (it would silently delete the group's data)",
            span,
        ));
    }
    // A whole-record write adds/replaces a key in the root's identity layer.
    env.guard_traversed_layer(&[PathSegment::Root(root.into())], span)?;
    let value = resource_value_of(fields, span)?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_write(resource, identity, &value, &*store)
    };
    let plan = plan.map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a managed merge `merge ^root(key…) = value`: drives
/// [`marrow_write::plan_resource_merge`] (copy supplied fields, keep absent ones)
/// and commits. When the source is another saved record of the same root
/// (`merge ^root(to) = ^root(from)`), this is a tree-shaped merge: its child-layer
/// subtrees are copied too, so the source identity is lowered and passed through.
/// A local-value source (`merge ^root(id) = patch`) carries only top-level fields.
fn eval_resource_merge(
    target: &Expression,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity) = lower_record_identity(target, env)?;
    // A saved-record source contributes its child-layer subtrees; lower its
    // identity (rejecting a cross-root merge) before reading its scalar fields.
    let source = if is_saved_path(value) {
        let (source_root, source_identity) = lower_record_identity(value, env)?;
        if source_root != root {
            return Err(unsupported("merging across saved roots", span));
        }
        Some(source_identity)
    } else {
        None
    };
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported("merging a non-resource value", span));
    };
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("merging this saved root", span))?;
    // A whole-record merge can create a new identity in the root's identity layer.
    env.guard_traversed_layer(&[PathSegment::Root(root.clone())], span)?;
    let value = resource_value_of(fields, span)?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_merge(resource, &identity, &value, source.as_deref(), &*store)
    };
    let plan = plan.map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a merge into a local resource var `merge draft = source`: overlay each
/// populated source field onto the local binding, leaving the local's other
/// fields in place (docs/language `resources-and-storage.md` — a `merge`
/// preserves fields the source does not supply). The local is ordinary program
/// state, so this is a sequence of local-field writes, not a managed saved write.
fn eval_local_merge(
    target: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported("merging a non-resource value", span));
    };
    if env.lookup(target).is_none() {
        return Err(unsupported("merging into an unbound local", span));
    }
    for (field, value) in fields {
        write_local_field(target, &field, value, span, env)?;
    }
    Ok(())
}

/// Apply a keyed-layer merge `merge ^root(to).layer = ^root(from).layer`: copy
/// the source layer's entries over the target layer (an overlay, leaving target
/// entries the source does not cover in place). Both sides must name the same
/// layer of the same saved root. Drives [`marrow_write::plan_layer_merge`], which
/// reads the source subtree, then commits.
fn eval_layer_merge(
    target_record: &Expression,
    layer: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    // The source is a saved layer path `^root(from).layer` naming the same root
    // and layer as the target.
    let Expression::Field {
        base: source_record,
        name: source_layer,
        ..
    } = value
    else {
        return Err(unsupported("merging this value into a layer", span));
    };
    if source_layer.as_str() != layer {
        return Err(unsupported(
            "merging between differently named layers",
            span,
        ));
    }
    let (to_root, to_identity) = lower_record_identity(target_record, env)?;
    let (from_root, from_identity) = lower_record_identity(source_record, env)?;
    if from_root != to_root {
        return Err(unsupported("merging a layer across saved roots", span));
    }
    let resource = find_resource(env.program, &to_root)
        .ok_or_else(|| unsupported("merging into this saved root", span))?;
    // A layer merge overlays entries into the target layer's key set.
    env.guard_traversed_layer(&layer_prefix(&to_root, &to_identity, layer), span)?;
    let plan = {
        let store = env.store.borrow();
        plan_layer_merge(resource, &from_identity, &to_identity, layer, &*store)
    };
    let plan = plan.map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// The encoded-path prefix of a keyed child layer `^root(identity…).layer` — the
/// layer whose child keys an entry write, append, or layer merge changes. Matches
/// the prefix [`traversed_layer_prefix`] produces for a loop over that layer, so
/// [`Env::guard_traversed_layer`] can compare them.
fn layer_prefix(root: &str, identity: &[SavedKey], layer: &str) -> Vec<PathSegment> {
    let mut prefix = vec![PathSegment::Root(root.into())];
    prefix.extend(identity.iter().cloned().map(PathSegment::RecordKey));
    prefix.push(PathSegment::ChildLayer(layer.into()));
    prefix
}

/// Lower a materialized resource value's present fields to a `ResourceValue` for
/// the managed-write planners. A nested resource field is unsupported.
fn resource_value_of(
    fields: Vec<(String, Value)>,
    span: SourceSpan,
) -> Result<ResourceValue, RuntimeError> {
    let mut resource_fields = Vec::with_capacity(fields.len());
    for (name, value) in fields {
        let saved =
            value_to_saved(value).ok_or_else(|| unsupported("a nested resource field", span))?;
        resource_fields.push((name, FieldValue::Saved(saved)));
    }
    Ok(ResourceValue {
        fields: resource_fields,
    })
}

/// Apply a whole-resource delete `delete ^root(key…)`, driving
/// [`marrow_write::plan_resource_delete`] (which removes the record and tears
/// down its generated index entries) and committing it. Field and layer deletes
/// are not yet supported.
fn eval_delete(path: &Expression, span: SourceSpan, env: &mut Env<'_>) -> Result<(), RuntimeError> {
    // Read the target shape to dispatch, mirroring the merge target-shape pattern:
    // a `.field` off a saved record is a field delete (top-level, or a group-entry
    // field via `is_group_base`); a `.layer(key…)` call off a saved record is a
    // keyed-entry subtree delete; anything else (`^root(key…)` or a singleton
    // `^settings`) is the whole-record delete handled below.
    if let Expression::Field { base, name, .. } = path
        && is_saved_path(base)
    {
        return eval_field_delete(base, name, span, env);
    }
    if let Expression::Call { callee, args, .. } = path
        && let Expression::Field { base, name, .. } = callee.as_ref()
        && is_saved_path(base)
    {
        return eval_layer_entry_delete(base, name, args, span, env);
    }
    let (root, identity) = lower_record_identity(path, env)?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("deleting from this saved root", span))?;
    // Deleting a record removes a key from the root's identity layer.
    env.guard_traversed_layer(&[PathSegment::Root(root.clone())], span)?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_delete(resource, &identity, &*store)
    };
    let plan = plan.map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a managed field delete `delete ^root(key…).field`. A top-level field
/// (`^books(id).subtitle`) drives [`marrow_write::plan_field_delete`] — removing
/// the field path and tearing down any index it feeds — after the required-field
/// guard. A group-entry field (`^books(id).versions(v).text`) is a plain subtree
/// delete of that one path (groups carry no generated indexes, matching
/// [`eval_group_field_write`]'s comment). A top-level field delete does not change
/// any traversed layer's key set, so it is not guarded against the identity layer.
fn eval_field_delete(
    base: &Expression,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    // A field reached through one or more group layers deletes inside that group
    // entry, with no index interaction.
    if is_group_base(base) {
        let (root, identity, layers) = lower_layer_path(base, env)?;
        return delete_nested_field(&root, &identity, &layers, field, span, env);
    }
    let (root, identity) = lower_record_identity(base, env)?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("deleting from this saved root", span))?;
    // Deleting a required field on its own would leave the resource invalid; it is
    // only allowed when the surrounding entry or whole resource is deleted.
    if resource
        .fields
        .iter()
        .any(|declared| declared.name == field && declared.required)
    {
        return Err(raise_fault(
            WRITE_REQUIRED_FIELD,
            format!("cannot delete required field `{field}`; delete the whole record instead"),
            span,
            env,
        ));
    }
    let plan = {
        let store = env.store.borrow();
        plan_field_delete(resource, &identity, field, &*store)
    };
    let plan = plan.map_err(|error| write_fault(error, span, env))?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Delete a scalar field inside a (possibly nested) keyed group entry,
/// `delete ^root(key…).layer(key…)….field`. Groups carry no generated indexes, so
/// this is a plain subtree delete of the one field path. The innermost layer must
/// declare `field` as a scalar member.
fn delete_nested_field(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let layer_names: Vec<&str> = layers.iter().map(|(name, _)| name.as_str()).collect();
    if resource_nested_member_type(env.program, root, &layer_names, field).is_none() {
        return Err(unsupported("deleting this group field", span));
    }
    let mut path = vec![PathSegment::Root(root.into())];
    path.extend(identity.iter().cloned().map(PathSegment::RecordKey));
    for (layer, keys) in layers {
        path.push(PathSegment::ChildLayer(layer.clone()));
        path.extend(keys.iter().cloned().map(PathSegment::IndexKey));
    }
    path.push(PathSegment::Field(field.into()));
    env.store
        .borrow_mut()
        .delete(&encode_path(&path))
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a keyed-entry subtree delete `delete ^root(key…).layer(entryKey…)`. The
/// backend `delete` is a subtree delete, so one delete of the entry prefix removes
/// the whole entry (a keyed leaf value, or a group entry with all its members and
/// nested layers). Child layers feed no generated index, so there is no index
/// maintenance. The guard fires against the layer prefix so a self-mutating
/// traversal of that layer is still caught.
fn eval_layer_entry_delete(
    record: &Expression,
    layer: &str,
    keys: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity, chain) = lower_layer_path(record, env)?;
    let entry_keys = lower_layer_keys(keys, span, env)?;
    // The full layer chain the delete targets must be declared on the resource.
    let layer_names: Vec<&str> = chain
        .iter()
        .map(|(name, _)| name.as_str())
        .chain(std::iter::once(layer))
        .collect();
    if !resource_layer_chain_exists(env.program, &root, &layer_names) {
        return Err(unsupported("deleting this layer entry", span));
    }
    // Deleting an entry changes the innermost layer's key set, so guard against
    // that layer's prefix. A direct layer of the record (empty chain) uses the
    // record-level prefix; a nested layer is not reachable by a top-level loop, so
    // the guard there is moot.
    if chain.is_empty() {
        env.guard_traversed_layer(&layer_prefix(&root, &identity, layer), span)?;
    }
    let mut path = vec![PathSegment::Root(root.clone())];
    path.extend(identity.iter().cloned().map(PathSegment::RecordKey));
    for (name, level_keys) in &chain {
        path.push(PathSegment::ChildLayer(name.clone()));
        path.extend(level_keys.iter().cloned().map(PathSegment::IndexKey));
    }
    path.push(PathSegment::ChildLayer(layer.into()));
    path.extend(entry_keys.into_iter().map(PathSegment::IndexKey));
    env.store
        .borrow_mut()
        .delete(&encode_path(&path))
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Whether the chain of layer names (outermost to innermost) is fully declared on
/// the resource at `root`: the first is a direct layer of the resource, each
/// deeper one a nested layer of the one before it. Used to reject a delete of an
/// undeclared layer entry before touching the store.
fn resource_layer_chain_exists(program: &CheckedProgram, root: &str, layers: &[&str]) -> bool {
    let Some(resource) = find_resource(program, root) else {
        return false;
    };
    let Some((first, rest)) = layers.split_first() else {
        return false;
    };
    let Some(mut current) = resource.layers.iter().find(|layer| &layer.name == first) else {
        return false;
    };
    for name in rest {
        let next = current.members.iter().find_map(|member| match member {
            LayerMember::Layer(layer) if &layer.name == name => Some(layer),
            _ => None,
        });
        match next {
            Some(layer) => current = layer,
            None => return false,
        }
    }
    true
}

/// The resource schema attached to a saved root, by root name.
fn find_resource<'p>(program: &'p CheckedProgram, root: &str) -> Option<&'p ResourceSchema> {
    program
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .find(|resource| {
            resource
                .saved_root
                .as_ref()
                .is_some_and(|saved| saved.root == root)
        })
}

/// The number of declared identity keys for the resource at saved root `name`,
/// or `None` when `name` is not a managed saved root. A keyless singleton has
/// arity 0; a keyed root such as `^books` has a positive arity, so it cannot be
/// read or addressed without an identity.
fn root_identity_arity(program: &CheckedProgram, name: &str) -> Option<usize> {
    find_resource(program, name)
        .and_then(|resource| resource.saved_root.as_ref())
        .map(|root| root.identity_keys.len())
}

/// The resource schema declared with `name`, for an identity constructor
/// `Name::Id(...)`. Keyed on the resource name (not its saved root), since the
/// constructor names the resource.
fn find_resource_by_name<'p>(
    program: &'p CheckedProgram,
    name: &str,
) -> Option<&'p ResourceSchema> {
    program
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .find(|resource| resource.name == name)
}

/// Build a resource identity value from a `Resource::Id(...)` constructor: its
/// keys lowered in declared identity-key order. Positional args lower in order;
/// named args (composite keys) match by key name. A singleton (keyless) resource
/// has no identity type, and an arity or name mismatch is a type error.
fn eval_identity_constructor(
    resource: &ResourceSchema,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let root = resource
        .saved_root
        .as_ref()
        .filter(|saved| !saved.identity_keys.is_empty());
    let Some(root) = root else {
        return Err(unsupported(
            "an identity for a resource with no identity keys",
            span,
        ));
    };
    if args.iter().any(|arg| arg.mode.is_some()) {
        return Err(type_error(
            "an identity key cannot be an out argument",
            span,
        ));
    }
    if args.len() != root.identity_keys.len() {
        return Err(type_error(
            &format!(
                "`{}::Id` takes {} key(s), got {}",
                resource.name,
                root.identity_keys.len(),
                args.len()
            ),
            span,
        ));
    }
    // Mixed positional and named arguments are ambiguous; require one shape.
    let named = args.iter().filter(|arg| arg.name.is_some()).count();
    if named != 0 && named != args.len() {
        return Err(type_error(
            "an identity takes either positional or named keys, not both",
            span,
        ));
    }
    let mut keys = Vec::with_capacity(root.identity_keys.len());
    if named == 0 {
        // Positional: each argument lowers to the key at the same position.
        for arg in args {
            keys.push(
                value_to_key(eval_expr(&arg.value, env)?)
                    .ok_or_else(|| type_error("a key of this type", span))?,
            );
        }
    } else {
        // Named (composite): for each declared key, find the matching argument,
        // so keys land in declared order regardless of argument order.
        for key in &root.identity_keys {
            let arg = args
                .iter()
                .find(|arg| arg.name.as_deref() == Some(key.name.as_str()))
                .ok_or_else(|| {
                    type_error(&format!("identity key `{}` is missing", key.name), span)
                })?;
            keys.push(
                value_to_key(eval_expr(&arg.value, env)?)
                    .ok_or_else(|| type_error("a key of this type", span))?,
            );
        }
    }
    Ok(Value::Identity(keys))
}

/// Whether the resource declares an unkeyed nested group, which a whole-resource
/// value owns but the runtime cannot yet materialize. A group layer has no key
/// params (a keyed leaf or keyed group always does), so any such layer is an
/// unkeyed group the whole-resource read would silently omit and the
/// whole-resource write would silently delete (review F15, interim).
fn declares_unkeyed_group(resource: &ResourceSchema) -> bool {
    resource
        .layers
        .iter()
        .any(|layer| layer.key_params.is_empty())
}

/// Whether `name` is a resource type declared in the program (for an
/// uninitialized `var book: Book` to start as an empty resource value).
fn is_resource_type(program: &CheckedProgram, name: &str) -> bool {
    program
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .any(|resource| resource.name == name)
}

/// Whether an expression denotes a saved path (rooted at a `^root`), as opposed
/// to a local value. Field access and key lookups on a saved path are saved
/// reads; on a local resource value they read its materialized fields.
fn is_saved_path(expr: &Expression) -> bool {
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Call { callee, .. } => is_saved_path(callee),
        Expression::Field { base, .. } => is_saved_path(base),
        _ => false,
    }
}

/// Whether a field-read/write base reaches its field through a group layer (so the
/// nested-field reader/writer handles it): a keyed GROUP entry `^root(id…).layer(key…)`
/// (a layer call whose callee is a `.layer` access), or an unkeyed group hop
/// `^root(id…).name` (a `.field` off a saved path). A plain record base
/// `^root(id…)` or singleton `^root` is a top-level field, not a group base.
fn is_group_base(base: &Expression) -> bool {
    match base {
        Expression::Call { callee, .. } => matches!(callee.as_ref(), Expression::Field { .. }),
        Expression::Field { base, .. } => is_saved_path(base),
        _ => false,
    }
}

/// Read a field of a local resource value, e.g. `book.shelf`. An unpopulated
/// field is an absent-element error.
fn eval_local_field_get(
    base: &Expression,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Value::Resource(fields) = eval_expr(base, env)? else {
        return Err(unsupported("a field of a non-resource value", span));
    };
    match fields.into_iter().find(|(name, _)| name == field) {
        Some((_, value)) => Ok(value),
        None => Err(raise_fault(
            RUN_ABSENT,
            format!("`{field}` is absent"),
            span,
            env,
        )),
    }
}

/// Set a field of a local resource variable, e.g. `book.title = t`. The base
/// must be a mutable local bound to a resource value; the field is updated (or
/// inserted) and the variable rebound.
fn eval_local_field_set(
    base: &Expression,
    field: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Expression::Name { segments, .. } = base else {
        return Err(unsupported("setting a field of this value", span));
    };
    let [name] = segments.as_slice() else {
        return Err(unsupported("setting a field of this value", span));
    };
    let new_value = eval_expr(value, env)?;
    write_local_field(name, field, new_value, span, env)
}

/// Read a field of the local resource bound to `base`, from a pre-resolved base
/// name. Shared by `out`/`inout` place reads.
fn read_local_field(
    base: &str,
    field: &str,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    let Some(Value::Resource(fields)) = env.lookup(base) else {
        return Err(unsupported("a field of a non-resource local", span));
    };
    fields
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| value.clone())
        .ok_or_else(|| RuntimeError {
            code: RUN_ABSENT,
            message: format!("`{field}` is absent"),
            span,
        })
}

/// Update (or insert) `field` of the local resource bound to `base` with an
/// already-evaluated value, rebinding the variable. Shared by
/// [`eval_local_field_set`] and `out`/`inout` write-back.
fn write_local_field(
    base: &str,
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Some(Value::Resource(mut fields)) = env.lookup(base).cloned() else {
        return Err(unsupported("setting a field of a non-resource local", span));
    };
    match fields.iter().position(|(existing, _)| existing == field) {
        Some(index) => fields[index].1 = value,
        None => fields.push((field.to_string(), value)),
    }
    env.assign(base, Value::Resource(fields))
        .map_err(|error| assign_error(base, error, span))
}

/// Convert a runtime value to the saved value a managed write stores. Total over
/// the scalar values this slice supports; the write planner checks the value
/// against the field's declared type.
fn value_to_saved(value: Value) -> Option<SavedValue> {
    Some(match value {
        Value::Int(n) => SavedValue::Int(n),
        Value::Bool(b) => SavedValue::Bool(b),
        Value::Str(s) => SavedValue::Str(s),
        Value::Instant(n) => SavedValue::Instant(n),
        Value::Date(d) => SavedValue::Date(d),
        Value::Duration(n) => SavedValue::Duration(n),
        Value::Decimal(d) => SavedValue::Decimal {
            coefficient: d.coefficient(),
            scale: d.scale(),
        },
        Value::Bytes(b) => SavedValue::Bytes(b),
        // A whole sequence or resource is a tree, not a scalar saved value; an
        // identity is opaque and is not stored as a field value in this wave.
        Value::Sequence(_) | Value::Resource(_) | Value::Identity(_) => return None,
    })
}

/// Lower a record path to its saved root name and identity key values: a keyed
/// lookup `^root(key…)`, or a bare singleton root `^root` (a zero-key identity).
fn lower_record_identity(
    expr: &Expression,
    env: &mut Env<'_>,
) -> Result<(String, Vec<SavedKey>), RuntimeError> {
    // A bare saved root is a whole-resource address only for a keyless singleton
    // (`Settings at ^settings`). For a keyed root such as `^books` it is not a
    // record — addressing or reading it without an identity is a type error, not
    // a silent read of the identity-less path.
    if let Expression::SavedRoot { name, span } = expr {
        return match root_identity_arity(env.program, name) {
            Some(0) => Ok((name.clone(), Vec::new())),
            Some(arity) => Err(type_error(
                &format!(
                    "`^{name}` expects {arity} identity key(s), got 0; address a record with `^{name}(id)`"
                ),
                *span,
            )),
            None => Err(unsupported("this saved path", *span)),
        };
    }
    let Expression::Call { callee, args, span } = expr else {
        return Err(unsupported("this saved path", expr.span()));
    };
    let Expression::SavedRoot { name, .. } = callee.as_ref() else {
        return Err(unsupported("this saved path", *span));
    };
    let keys = lower_identity_args(args, *span, env)?;
    Ok((name.clone(), keys))
}

/// Evaluate a keyed lookup's arguments to identity key segments. A sole
/// identity-valued argument (`^root(id)` where `id: Resource::Id`) splices its
/// lowered keys in as the full identity; otherwise each argument is one raw key
/// (the `^root(17)`/`nextId` flow). Named/out arguments are rejected, and an
/// identity argument cannot be mixed with raw keys.
fn lower_identity_args(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "a keyed lookup with named or out arguments",
            span,
        ));
    }
    let mut keys = Vec::with_capacity(args.len());
    for arg in args {
        match eval_expr(&arg.value, env)? {
            // An identity is the whole lookup key only as the sole argument; it
            // cannot be one component among raw keys.
            Value::Identity(identity) if args.len() == 1 => return Ok(identity),
            Value::Identity(_) => {
                return Err(unsupported("an identity mixed with other keys", span));
            }
            value => keys
                .push(value_to_key(value).ok_or_else(|| unsupported("a key of this type", span))?),
        }
    }
    Ok(keys)
}

/// A lowered keyed group-entry path: the saved root name, the record identity
/// keys, and the chain of `(layer, key…)` levels from outermost to innermost.
type LayerPath = (String, Vec<SavedKey>, Vec<(String, Vec<SavedKey>)>);

/// Lower a (possibly nested) keyed group-entry path to its saved root, record
/// identity, and the chain of `(layer, key…)` levels from outermost to innermost.
/// `^root(id…)` lowers to an empty chain; each `….layer(key…)` wrapper appends one
/// level, so `^books(id).versions(v).comments(c)` yields two chain entries.
fn lower_layer_path(expr: &Expression, env: &mut Env<'_>) -> Result<LayerPath, RuntimeError> {
    if let Expression::Call { callee, args, span } = expr
        && let Expression::Field { base, name, .. } = callee.as_ref()
    {
        let (root, identity, mut chain) = lower_layer_path(base, env)?;
        let keys = lower_layer_keys(args, *span, env)?;
        chain.push((name.clone(), keys));
        return Ok((root, identity, chain));
    }
    // An unkeyed group hop `….name` (a `.field` off a saved path, not a call)
    // appends a zero-key layer level, so `^patients(id).name` descends into the
    // group `name`. The record base is handled by the terminal arm below.
    if let Expression::Field { base, name, .. } = expr
        && is_saved_path(base)
    {
        let (root, identity, mut chain) = lower_layer_path(base, env)?;
        chain.push((name.clone(), Vec::new()));
        return Ok((root, identity, chain));
    }
    let (root, identity) = lower_record_identity(expr, env)?;
    Ok((root, identity, Vec::new()))
}

/// Evaluate keyed-lookup arguments to saved keys, rejecting named/out arguments.
/// Shared by keyed-leaf reads and group-entry field writes.
fn lower_layer_keys(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "a keyed lookup with named or out arguments",
            span,
        ));
    }
    let mut keys = Vec::with_capacity(args.len());
    for arg in args {
        keys.push(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("a key of this type", span))?,
        );
    }
    Ok(keys)
}

/// Lower any saved path expression — `^root`, `^root(key…)`, or a `.field` off
/// one — to its encoded segments. Used by `exists`, which needs only the path,
/// not the resource schema.
fn lower_saved_path(
    expr: &Expression,
    env: &mut Env<'_>,
) -> Result<Vec<PathSegment>, RuntimeError> {
    match expr {
        Expression::SavedRoot { name, .. } => Ok(vec![PathSegment::Root(name.clone())]),
        Expression::Call { callee, args, span } => {
            let mut segments = lower_saved_path(callee, env)?;
            let keys = lower_identity_args(args, *span, env)?;
            segments.extend(keys.into_iter().map(PathSegment::RecordKey));
            Ok(segments)
        }
        Expression::Field { base, name, .. } => {
            let mut segments = lower_saved_path(base, env)?;
            segments.push(PathSegment::Field(name.clone()));
            Ok(segments)
        }
        other => Err(unsupported("this saved path", other.span())),
    }
}

/// The declared scalar type of a saved root's top-level field, found by matching
/// the root name against the program's resource schemas.
fn resource_field_type(program: &CheckedProgram, root: &str, field: &str) -> Option<ValueType> {
    let resource = program
        .modules
        .iter()
        .flat_map(|module| &module.resources)
        .find(|resource| {
            resource
                .saved_root
                .as_ref()
                .is_some_and(|saved| saved.root == root)
        })?;
    let field = resource.fields.iter().find(|field_| field_.name == field)?;
    ValueType::from_scalar_name(&field.ty.text)
}

/// The declared leaf type of a keyed-leaf layer on a saved root (e.g. the
/// `string` of `tags(pos: int): string`).
fn resource_layer_leaf_type(
    program: &CheckedProgram,
    root: &str,
    layer: &str,
) -> Option<ValueType> {
    let resource = find_resource(program, root)?;
    let layer = resource
        .layers
        .iter()
        .find(|declared| declared.name == layer)?;
    ValueType::from_scalar_name(&layer.leaf_type.as_ref()?.text)
}

/// The declared type of a scalar member field inside a saved root's GROUP layer,
/// at any nesting depth (e.g. the `string` of
/// `versions(version: int).comments(pos: int).text`). `layers` names the group
/// layers from outermost to innermost; descending follows nested layer members.
fn resource_nested_member_type(
    program: &CheckedProgram,
    root: &str,
    layers: &[&str],
    field: &str,
) -> Option<ValueType> {
    let resource = find_resource(program, root)?;
    let (first, rest) = layers.split_first()?;
    let mut current = resource.layers.iter().find(|layer| &layer.name == first)?;
    for name in rest {
        current = current.members.iter().find_map(|member| match member {
            LayerMember::Layer(layer) if &layer.name == name => Some(layer),
            _ => None,
        })?;
    }
    let member = current.members.iter().find_map(|member| match member {
        LayerMember::Field(member) if member.name == field => Some(member),
        _ => None,
    })?;
    ValueType::from_scalar_name(&member.ty.text)
}

/// The scalar Field members of a saved root's GROUP layer, as `(name, value type)`
/// in declaration order, for materializing a whole group entry. `None` if the
/// layer is unknown.
fn resource_group_members(
    program: &CheckedProgram,
    root: &str,
    layer: &str,
) -> Option<Vec<(String, ValueType)>> {
    let resource = find_resource(program, root)?;
    let layer = resource
        .layers
        .iter()
        .find(|declared| declared.name == layer)?;
    let members = layer
        .members
        .iter()
        .filter_map(|member| match member {
            LayerMember::Field(field) => Some((
                field.name.clone(),
                ValueType::from_scalar_name(&field.ty.text)?,
            )),
            _ => None,
        })
        .collect();
    Some(members)
}

/// Convert a record-key value to a [`SavedKey`], or `None` for a type that is not
/// a key (only int/bool/string are runtime values this slice can key on).
fn value_to_key(value: Value) -> Option<SavedKey> {
    match value {
        Value::Int(n) => Some(SavedKey::Int(n)),
        Value::Bool(b) => Some(SavedKey::Bool(b)),
        Value::Str(s) => Some(SavedKey::Str(s)),
        Value::Instant(n) => Some(SavedKey::Instant(n)),
        Value::Date(d) => Some(SavedKey::Date(d)),
        Value::Duration(n) => Some(SavedKey::Duration(n)),
        Value::Bytes(b) => Some(SavedKey::Bytes(b)),
        // Decimal keys are deferred; sequences and resources are not scalar keys.
        // An identity is not a single key — lowering splices its segments in
        // before reaching here.
        Value::Decimal(_) | Value::Sequence(_) | Value::Resource(_) | Value::Identity(_) => None,
    }
}

/// Convert a decoded saved value to a runtime value, or `None` for a scalar type
/// the runtime does not yet represent (date, decimal, and so on).
fn saved_value_to_value(value: SavedValue) -> Option<Value> {
    match value {
        SavedValue::Int(n) => Some(Value::Int(n)),
        SavedValue::Bool(b) => Some(Value::Bool(b)),
        SavedValue::Str(s) => Some(Value::Str(s)),
        SavedValue::Instant(n) => Some(Value::Instant(n)),
        SavedValue::Date(d) => Some(Value::Date(d)),
        SavedValue::Duration(n) => Some(Value::Duration(n)),
        SavedValue::Decimal { coefficient, scale } => {
            Decimal::from_parts(coefficient, scale).map(Value::Decimal)
        }
        SavedValue::Bytes(b) => Some(Value::Bytes(b)),
        _ => None,
    }
}

/// Evaluate an interpolated string `$"...{expr}..."` to a string value: literal
/// segments contribute their text (with `{{`/`}}` unescaped to single braces),
/// and embedded expressions are rendered to text.
fn eval_interpolation(
    parts: &[InterpolationPart],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut result = String::new();
    for part in parts {
        match part {
            InterpolationPart::Text { text, .. } => {
                // Backslash escapes are not yet decoded (as for plain strings).
                if text.contains('\\') {
                    return Err(unsupported("string escape sequences", span));
                }
                result.push_str(&text.replace("{{", "{").replace("}}", "}"));
            }
            InterpolationPart::Expr(expr) => result.push_str(&render(eval_expr(expr, env)?, span)?),
        }
    }
    Ok(Value::Str(result))
}

/// Render a scalar value as text: integers in decimal, booleans as
/// `true`/`false`, strings as themselves. Resource values have no text form, and
/// an instant is rendered through `std::clock::formatInstant`, not directly.
fn render(value: Value, span: SourceSpan) -> Result<String, RuntimeError> {
    Ok(match value {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => s,
        Value::Decimal(d) => d.to_text(),
        Value::Bytes(_) => return Err(unsupported("rendering a bytes value", span)),
        Value::Sequence(_) => return Err(unsupported("rendering a sequence value", span)),
        Value::Instant(_) => return Err(unsupported("rendering an instant value", span)),
        Value::Date(_) => return Err(unsupported("rendering a date value", span)),
        Value::Duration(_) => return Err(unsupported("rendering a duration value", span)),
        Value::Resource(_) => return Err(unsupported("rendering a resource value", span)),
        Value::Identity(_) => return Err(unsupported("rendering an identity value", span)),
    })
}

fn eval_literal(kind: LiteralKind, text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    match kind {
        LiteralKind::Integer => text
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| RuntimeError {
                code: RUN_OVERFLOW,
                message: format!("integer literal `{text}` is out of range"),
                span,
            }),
        LiteralKind::Bool => Ok(Value::Bool(text == "true")),
        LiteralKind::String => eval_string_literal(text, span),
        LiteralKind::Decimal => {
            Decimal::parse(text)
                .map(Value::Decimal)
                .ok_or_else(|| RuntimeError {
                    code: RUN_OVERFLOW,
                    message: format!("decimal literal `{text}` is out of range"),
                    span,
                })
        }
        LiteralKind::Bytes => eval_bytes_literal(text, span),
    }
}

/// Decode a string literal's value. The literal `text` is the raw source,
/// including the surrounding quotes; escape sequences are not yet decoded, so a
/// literal containing a backslash is reported as unsupported rather than guessed.
fn eval_string_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| unsupported("this string literal", span))?;
    if inner.contains('\\') {
        return Err(unsupported("string escape sequences", span));
    }
    Ok(Value::Str(inner.to_string()))
}

/// Decode a bytes literal `b"..."` to its raw bytes (the content's UTF-8). Like
/// string literals, escape sequences are not yet decoded.
fn eval_bytes_literal(text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    let inner = text
        .strip_prefix("b\"")
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| unsupported("this bytes literal", span))?;
    if inner.contains('\\') {
        return Err(unsupported("bytes escape sequences", span));
    }
    Ok(Value::Bytes(inner.as_bytes().to_vec()))
}

fn eval_unary(
    op: UnaryOp,
    operand: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match (op, eval_expr(operand, env)?) {
        (UnaryOp::Neg, Value::Int(n)) => n
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| overflow(span)),
        (UnaryOp::Neg, Value::Decimal(d)) => Decimal::from_parts(-d.coefficient(), d.scale())
            .map(Value::Decimal)
            .ok_or_else(|| overflow(span)),
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, _) => Err(type_error("negation expects a number", span)),
        (UnaryOp::Not, _) => Err(type_error("`not` expects a boolean", span)),
    }
}

fn eval_binary(
    op: BinaryOp,
    left: &Expression,
    right: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    match op {
        // Logical operators short-circuit: the right side is evaluated only when
        // the left does not already decide the result.
        BinaryOp::And => Ok(Value::Bool(eval_bool(left, env)? && eval_bool(right, env)?)),
        BinaryOp::Or => Ok(Value::Bool(eval_bool(left, env)? || eval_bool(right, env)?)),
        BinaryOp::Add => numeric_op(
            left,
            right,
            env,
            span,
            i64::checked_add,
            Decimal::checked_add,
        ),
        BinaryOp::Subtract => numeric_op(
            left,
            right,
            env,
            span,
            i64::checked_sub,
            Decimal::checked_sub,
        ),
        BinaryOp::Multiply => numeric_op(
            left,
            right,
            env,
            span,
            i64::checked_mul,
            Decimal::checked_mul,
        ),
        // `/` always yields a decimal (docs/language/syntax.md), so integer
        // operands divide as decimals too: `1 / 2` is `0.5`.
        BinaryOp::Divide => decimal_div(left, right, env, span),
        BinaryOp::Remainder => int_remainder_op(left, right, env, span),
        BinaryOp::Less => compare_values(left, right, env, span, |o| o == Ordering::Less),
        BinaryOp::LessEqual => compare_values(left, right, env, span, |o| o != Ordering::Greater),
        BinaryOp::Greater => compare_values(left, right, env, span, |o| o == Ordering::Greater),
        BinaryOp::GreaterEqual => compare_values(left, right, env, span, |o| o != Ordering::Less),
        BinaryOp::Equal => Ok(Value::Bool(values_equal(left, right, env, span)?)),
        BinaryOp::NotEqual => Ok(Value::Bool(!values_equal(left, right, env, span)?)),
        BinaryOp::Concat => concat(left, right, env, span),
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => {
            Err(unsupported("this operator", span))
        }
    }
}

/// Apply a checked numeric operation to two operands of the same numeric type —
/// both integers or both decimals — mapping overflow to `run.overflow`. The
/// checker rejects mixed int/decimal operands, so a mismatch here is a type error.
fn numeric_op(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
    int_op: fn(i64, i64) -> Option<i64>,
    decimal_op: fn(Decimal, Decimal) -> Option<Decimal>,
) -> Result<Value, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
        (Value::Int(a), Value::Int(b)) => {
            int_op(a, b).map(Value::Int).ok_or_else(|| overflow(span))
        }
        (Value::Decimal(a), Value::Decimal(b)) => decimal_op(a, b)
            .map(Value::Decimal)
            .ok_or_else(|| overflow(span)),
        _ => Err(type_error(
            "arithmetic expects two operands of the same numeric type",
            span,
        )),
    }
}

/// Divide two numeric operands as decimals (`/` always yields a decimal). A zero
/// divisor is `run.divide_by_zero`; a result outside the decimal envelope is
/// `run.overflow`.
fn decimal_div(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    let dividend = to_decimal(eval_expr(left, env)?, span)?;
    let divisor = to_decimal(eval_expr(right, env)?, span)?;
    if divisor.is_zero() {
        return Err(RuntimeError {
            code: RUN_DIVIDE_BY_ZERO,
            message: "division by zero".into(),
            span,
        });
    }
    dividend
        .checked_div(divisor)
        .map(Value::Decimal)
        .ok_or_else(|| overflow(span))
}

/// Coerce a numeric value to a decimal: an integer becomes an exact decimal, a
/// decimal is itself. Any other type is a runtime type error.
fn to_decimal(value: Value, span: SourceSpan) -> Result<Decimal, RuntimeError> {
    match value {
        Value::Decimal(decimal) => Ok(decimal),
        Value::Int(n) => Decimal::from_parts(i128::from(n), 0)
            .ok_or_else(|| type_error("an integer that is not a valid decimal", span)),
        _ => Err(type_error("division expects numeric operands", span)),
    }
}

/// Evaluate the integer remainder operator (`%`) over two operands. The `/`
/// operator yields a decimal and uses `decimal_div`, so `%` is the only integer
/// division-family operator; it shares the one integer-remainder path (and its
/// "integer remainder by zero" message) with `std::math::remainder`.
fn int_remainder_op(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    let a = eval_int(left, env)?;
    let b = eval_int(right, env)?;
    int_remainder(a, b, span).map(Value::Int)
}

/// Compare two values of the same orderable type — integers or strings — and
/// test the resulting ordering. Booleans and mismatched types are not orderable.
fn compare_values(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
    want: fn(Ordering) -> bool,
) -> Result<Value, RuntimeError> {
    let ordering = match (eval_expr(left, env)?, eval_expr(right, env)?) {
        (Value::Int(a), Value::Int(b)) => a.cmp(&b),
        (Value::Str(a), Value::Str(b)) => a.cmp(&b),
        (Value::Decimal(a), Value::Decimal(b)) => a.cmp(&b),
        (Value::Bytes(a), Value::Bytes(b)) => a.cmp(&b),
        // Temporal values order by their underlying instant/day/nanosecond count.
        (Value::Instant(a), Value::Instant(b)) => a.cmp(&b),
        (Value::Date(a), Value::Date(b)) => a.cmp(&b),
        (Value::Duration(a), Value::Duration(b)) => a.cmp(&b),
        _ => {
            return Err(type_error(
                "cannot order values of different or unordered types",
                span,
            ));
        }
    };
    Ok(Value::Bool(want(ordering)))
}

/// Concatenate two strings with `++`.
fn concat(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
        (Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
        _ => Err(type_error("`++` concatenates two strings", span)),
    }
}

/// Whether two values are equal. They must share a scalar type; comparing across
/// types is a runtime type error (the checker rejects it statically).
fn values_equal(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
) -> Result<bool, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
        (Value::Int(a), Value::Int(b)) => Ok(a == b),
        (Value::Bool(a), Value::Bool(b)) => Ok(a == b),
        (Value::Str(a), Value::Str(b)) => Ok(a == b),
        (Value::Decimal(a), Value::Decimal(b)) => Ok(a == b),
        (Value::Bytes(a), Value::Bytes(b)) => Ok(a == b),
        (Value::Instant(a), Value::Instant(b)) => Ok(a == b),
        (Value::Date(a), Value::Date(b)) => Ok(a == b),
        (Value::Duration(a), Value::Duration(b)) => Ok(a == b),
        _ => Err(type_error("cannot compare values of different types", span)),
    }
}

fn eval_int(expr: &Expression, env: &mut Env<'_>) -> Result<i64, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Int(n) => Ok(n),
        _ => Err(type_error("expected an integer", expr.span())),
    }
}

fn eval_bool(expr: &Expression, env: &mut Env<'_>) -> Result<bool, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Bool(b) => Ok(b),
        _ => Err(type_error("expected a boolean", expr.span())),
    }
}

fn store_error(error: StoreError, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code: RUN_STORE,
        message: format!("a saved-data operation failed: {error:?}"),
        span,
    }
}

/// Surface a managed-write planning failure (`marrow_write::WriteError`) as a
/// catchable fault: the spec treats a rejected managed write — a unique conflict,
/// a missing required field, a type or identity mismatch, a value-range error, or
/// a store read error met while planning — as recoverable, so a `try`/`catch`
/// can bind it and a transaction can continue or roll back. The fault keeps the
/// `write.*` (or value-codec) code so an uncaught one surfaces unchanged. Call
/// after dropping any `env.store` borrow held while planning.
fn write_fault(error: WriteError, span: SourceSpan, env: &mut Env<'_>) -> RuntimeError {
    raise_fault(error.code, error.message, span, env)
}

/// Surface a value-encoding range error (e.g. a date/instant outside year
/// 0001-9999) as a runtime error, preserving the codec's stable dotted code.
fn value_error(error: ValueError, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code: error.code(),
        message: error.to_string(),
        span,
    }
}

fn unsupported(what: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code: RUN_UNSUPPORTED,
        message: format!("the runtime does not yet evaluate {what}"),
        span,
    }
}

fn type_error(message: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code: RUN_TYPE,
        message: message.to_string(),
        span,
    }
}

fn overflow(span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code: RUN_OVERFLOW,
        message: "integer arithmetic overflowed".into(),
        span,
    }
}
