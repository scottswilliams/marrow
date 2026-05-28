//! The Marrow runtime: evaluate checked `.mw` functions.
//!
//! The evaluator runs functions over scalar values (integers, booleans,
//! strings) with locals, arithmetic/comparison/logical/`_` operators,
//! conditionals, `while`/`for` loops, interpolation, and calls between
//! functions. It reads saved data (fields and keyed-leaf entries) and writes it
//! through the managed-write layer (`^books(id).field = …`, `delete`, `append`),
//! groups writes in a `transaction` (commit/rollback with read-your-writes), and
//! provides the `print`/`write`/`exists`/`get`/`nextId`/`append` builtins and the
//! `std::clock::now()` host capability. Whole-resource writes, `merge`, index
//! traversal, and structured errors build on this spine.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_check::{CheckedFunction, CheckedParam, CheckedProgram};
use marrow_schema::{LayerMember, ResourceSchema};
use marrow_store::backend::Backend;
use marrow_store::mem::{MemStore, Presence, StoreError};
use marrow_store::path::{ChildSegment, PathSegment, SavedKey, encode_path};
use marrow_store::value::{SavedValue, ValueType, decode_value};
use marrow_syntax::{
    Argument, BinaryOp, Block, Expression, ForBinding, FunctionDecl, InterpolationPart,
    LiteralKind, SourceSpan, Statement, UnaryOp,
};
use marrow_write::{
    FieldValue, ResourceValue, next_id, next_layer_pos, plan_field_write, plan_layer_field_write,
    plan_layer_leaf_write, plan_layer_merge, plan_resource_delete, plan_resource_merge,
    plan_resource_write,
};

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
    /// A materialized resource tree: its present top-level fields, in schema
    /// order. Produced by a whole-resource read and consumed by a whole-resource
    /// write or `merge`.
    Resource(Vec<(String, Value)>),
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
}

impl Host {
    /// A host that provides no capabilities.
    pub fn new() -> Self {
        Self::default()
    }

    /// A host whose clock reads the real system time, captured now.
    pub fn with_system_clock() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as i128)
            .unwrap_or(0);
        Self { clock: Some(nanos) }
    }

    /// A host whose clock returns a fixed instant (nanoseconds since the Unix
    /// epoch, UTC), for deterministic runs and tests.
    pub fn with_clock(nanos: i128) -> Self {
        Self { clock: Some(nanos) }
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
    invoke(ctx, output, &names, &function.body, function.span, args)
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
    let value = invoke(
        ctx,
        Rc::clone(&output),
        &names,
        &function.body,
        function.span,
        args,
    )?;
    Ok(RunOutput {
        value,
        output: output.borrow().clone(),
    })
}

/// Bind `args` to `param_names`, evaluate `body` in a fresh activation, and
/// surface its returned value. Shared by [`evaluate_function`], [`run_entry`],
/// and call evaluation.
fn invoke(
    ctx: Context<'_>,
    output: Rc<RefCell<String>>,
    param_names: &[&str],
    body: &Block,
    span: SourceSpan,
    args: &[Value],
) -> Result<Option<Value>, RuntimeError> {
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
        env.bind((*name).to_string(), arg.clone(), false);
    }
    let flow = eval_block(body, &mut env)?;
    env.pop_scope();
    match flow {
        Flow::Return(value) => Ok(value),
        Flow::Normal => Ok(None),
        Flow::Break(_) | Flow::Continue(_) => Err(RuntimeError {
            code: RUN_NO_ENCLOSING_LOOP,
            message: "`break` or `continue` outside a loop".into(),
            span,
        }),
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
/// exactly once. (`out`/`inout` modes are rejected before this point.)
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
        let index = match &arg.name {
            None => {
                // A positional argument after a named one would silently
                // back-fill an earlier parameter; named arguments come last.
                if seen_named {
                    return Err(type_error(
                        "a positional argument cannot follow a named argument",
                        span,
                    ));
                }
                let index = next_positional;
                next_positional += 1;
                index
            }
            Some(name) => {
                seen_named = true;
                params
                    .iter()
                    .position(|param| &param.name == name)
                    .ok_or_else(|| type_error(&format!("call has no parameter `{name}`"), span))?
            }
        };
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
        *slot = Some(eval_expr(&arg.value, env)?);
    }
    let mut values = Vec::with_capacity(params.len());
    for (slot, param) in slots.into_iter().zip(params) {
        values.push(
            slot.ok_or_else(|| {
                type_error(&format!("missing argument for `{}`", param.name), span)
            })?,
        );
    }
    Ok(values)
}

/// Evaluate a call to a program function, returning its returned value (or
/// `None` for a function that returns nothing). Arguments may be positional or
/// named; `out`/`inout` argument modes are not yet supported.
fn eval_call(
    callee: &Expression,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // A call whose callee is a saved field is a keyed-leaf read, e.g.
    // `^books(id).tags(pos)`, not a function call.
    if let Expression::Field { .. } = callee {
        return eval_saved_leaf_read(callee, args, span, env).map(Some);
    }
    // A call whose callee is a saved root is a whole-resource read, `^books(id)`.
    if let Expression::SavedRoot { .. } = callee {
        return eval_resource_read(callee, args, span, env).map(Some);
    }
    let Expression::Name { segments, .. } = callee else {
        return Err(unsupported("calling this expression", span));
    };
    // `out`/`inout` argument modes are deferred for every call.
    if args.iter().any(|arg| arg.mode.is_some()) {
        return Err(unsupported("out/inout arguments", span));
    }
    // Builtins and the host capability take positional arguments only; a call
    // carrying named arguments is a program-function call, handled last.
    let has_named = args.iter().any(|arg| arg.name.is_some());
    if !has_named {
        // Builtins are call-shaped but are not program functions.
        if let [name] = segments.as_slice() {
            match name.as_str() {
                "print" | "write" => return eval_output(name, args, span, env),
                "exists" => return eval_exists(args, span, env).map(Some),
                "get" => return eval_get(args, span, env).map(Some),
                "nextId" => return eval_next_id(args, span, env).map(Some),
                "append" => return eval_append(args, span, env).map(Some),
                _ => {}
            }
        }
        // `std::clock::now()` is a host-capability builtin, not a program function.
        if let [first, second, third] = segments.as_slice()
            && first == "std"
            && second == "clock"
            && third == "now"
        {
            return eval_clock_now(args, span, env).map(Some);
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
    let values = bind_arguments(&function.params, args, span, env)?;
    let names: Vec<&str> = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    invoke(
        ctx,
        Rc::clone(&env.output),
        &names,
        &function.body,
        function.span,
        &values,
    )
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
        Err(error) if error.code == RUN_ABSENT => eval_expr(&default.value, env),
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
    let store = env.store.borrow();
    let next = next_id(name, &*store).map_err(|error| RuntimeError {
        code: error.code,
        message: error.message,
        span,
    })?;
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
    let saved = value_to_saved(eval_expr(&value.value, env)?)
        .ok_or_else(|| unsupported("appending a resource value", span))?;
    let pos = {
        let store = env.store.borrow();
        next_layer_pos(resource, &identity, layer, &*store).map_err(|error| RuntimeError {
            code: error.code,
            message: error.message,
            span,
        })?
    };
    let plan = plan_layer_leaf_write(resource, &identity, layer, &[SavedKey::Int(pos)], &saved)
        .map_err(|error| RuntimeError {
            code: error.code,
            message: error.message,
            span,
        })?;
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(Value::Int(pos))
}

/// Evaluate `std::clock::now()`: the run's current UTC instant from the host's
/// clock capability. A run with no clock capability raises a typed capability
/// error rather than reading the wall clock implicitly.
fn eval_clock_now(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if !args.is_empty() {
        return Err(type_error("`std::clock::now` takes no arguments", span));
    }
    let nanos = env.host.clock.ok_or_else(|| RuntimeError {
        code: RUN_CAPABILITY,
        message: "this run provides no clock capability for `std::clock::now`".into(),
        span,
    })?;
    Ok(Value::Instant(nanos))
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
        }
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
                // An uninitialized var of a resource type starts as an empty
                // resource value, filled field by field before use.
                None => match ty {
                    Some(ty) if is_resource_type(env.program, &ty.text) => {
                        Value::Resource(Vec::new())
                    }
                    _ => return Err(unsupported("an uninitialized variable", *span)),
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
            // `^root(key…)` target is a whole-resource write; a bare name is a
            // local reassignment.
            if let Expression::Field { base, name, .. } = target {
                if is_saved_path(base) {
                    eval_saved_field_write(base, name, value, *span, env)?;
                } else {
                    eval_local_field_set(base, name, value, *span, env)?;
                }
            } else if let Expression::Call { .. } = target {
                eval_resource_write(target, value, *span, env)?;
            } else {
                let name = local_target(target, *span)?;
                let evaluated = eval_expr(value, env)?;
                env.assign(name, evaluated).map_err(|error| match error {
                    AssignError::Immutable => RuntimeError {
                        code: RUN_TYPE,
                        message: format!("cannot assign to immutable `{name}`"),
                        span: *span,
                    },
                    AssignError::Unbound => RuntimeError {
                        code: RUN_UNBOUND_NAME,
                        message: format!("`{name}` is not bound"),
                        span: *span,
                    },
                })?;
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
            // A `.layer` off a saved record is a keyed-layer merge; a `^root(key…)`
            // target is a whole-resource merge.
            if let Expression::Field { base, name, .. } = target
                && is_saved_path(base)
            {
                eval_layer_merge(base, name, value, *span, env)?;
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

fn eval_for(
    label: &Option<String>,
    binding: &ForBinding,
    iterable: &Expression,
    body: &Block,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Flow, RuntimeError> {
    if binding.second.is_some() {
        return Err(unsupported("a two-name loop binding", span));
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
        for value in eval_collection(iterable, env)? {
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
        return Ok(Flow::Normal);
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

/// Materialize a non-range `for` iterable to a sequence of values. Only
/// `keys(saved_path)` is supported: it yields the immediate child keys under the
/// path — e.g. `keys(^books.byShelf("fiction"))` yields the book ids on that
/// shelf, for index traversal.
fn eval_collection(iterable: &Expression, env: &mut Env<'_>) -> Result<Vec<Value>, RuntimeError> {
    let Expression::Call { callee, args, span } = iterable else {
        return Err(unsupported("iterating this value", iterable.span()));
    };
    let is_keys = matches!(
        callee.as_ref(),
        Expression::Name { segments, .. } if segments.len() == 1 && segments[0] == "keys"
    );
    if !is_keys {
        return Err(unsupported("iterating this value", *span));
    }
    let [path] = args.as_slice() else {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: "`keys` takes one argument".into(),
            span: *span,
        });
    };
    // The path is an index lookup `^root.index(key…)`: lower it to the index
    // prefix, whose immediate children are the matching record keys.
    let Expression::Call {
        callee: index_callee,
        args: index_args,
        ..
    } = &path.value
    else {
        return Err(unsupported("keys of this path", *span));
    };
    let Expression::Field {
        base, name: index, ..
    } = index_callee.as_ref()
    else {
        return Err(unsupported("keys of this path", *span));
    };
    let Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return Err(unsupported("keys of this path", *span));
    };
    if index_args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "an index lookup with named or out arguments",
            *span,
        ));
    }
    let mut segments = vec![
        PathSegment::Root(root.clone()),
        PathSegment::Index(index.clone()),
    ];
    for arg in index_args {
        segments.push(PathSegment::IndexKey(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", *span))?,
        ));
    }
    let children = {
        let store = env.store.borrow();
        store
            .child_keys(&encode_path(&segments))
            .map_err(|_| RuntimeError {
                code: RUN_STORE,
                message: "could not read the keys at this path".into(),
                span: *span,
            })?
    };
    let mut values = Vec::with_capacity(children.len());
    for child in children {
        if let ChildSegment::Key(key) = child {
            values.push(
                saved_key_to_value(key)
                    .ok_or_else(|| unsupported("iterating keys of this type", *span))?,
            );
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
    // `^root(id…).layer(key…).field` reads a field inside a keyed GROUP entry:
    // the base is a layer call whose callee is itself a `.layer` access.
    if let Expression::Call { callee, args, .. } = base.as_ref()
        && let Expression::Field {
            base: record,
            name: layer,
            ..
        } = callee.as_ref()
    {
        return eval_group_field_read(record, layer, args, name, expr.span(), env);
    }
    let (root, keys) = lower_record_identity(base, env)?;
    let mut segments = vec![PathSegment::Root(root.clone())];
    segments.extend(keys.into_iter().map(PathSegment::RecordKey));
    segments.push(PathSegment::Field(name.clone()));
    let field_type = resource_field_type(env.program, &root, name)
        .ok_or_else(|| unsupported("reading this field", expr.span()))?;
    let store = env.store.borrow();
    let Some(bytes) = store
        .read(&encode_path(&segments))
        .map_err(|error| store_error(error, expr.span()))?
    else {
        return Err(RuntimeError {
            code: RUN_ABSENT,
            message: format!("`{name}` is absent"),
            span: expr.span(),
        });
    };
    decode_value(&bytes, field_type)
        .and_then(saved_value_to_value)
        .ok_or_else(|| RuntimeError {
            code: RUN_TYPE,
            message: format!("stored value for `{name}` did not decode to a runtime value"),
            span: expr.span(),
        })
}

/// Read a field inside a keyed GROUP entry, e.g. `^books(id).versions(v).title`.
/// Lowers the record identity and layer keys, reads the store, and decodes with
/// the group member's declared type; an absent entry is an absent-element error.
fn eval_group_field_read(
    record: &Expression,
    layer: &str,
    keys: &[Argument],
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let (root, identity) = lower_record_identity(record, env)?;
    let layer_keys = lower_layer_keys(keys, span, env)?;
    let member_type = resource_group_member_type(env.program, &root, layer, field)
        .ok_or_else(|| unsupported("reading this group field", span))?;
    let mut segments = vec![PathSegment::Root(root)];
    segments.extend(identity.into_iter().map(PathSegment::RecordKey));
    segments.push(PathSegment::ChildLayer(layer.to_string()));
    segments.extend(layer_keys.into_iter().map(PathSegment::IndexKey));
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

/// Read a keyed-leaf entry off a saved record, e.g. `^books(id).tags(pos)`. The
/// `callee` is the layer field `^books(id).tags` and `keys` are the layer key
/// arguments. Lowers the path, reads the store, and decodes with the layer's
/// leaf type; an absent entry is an absent-element error.
fn eval_saved_leaf_read(
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
    let mut segments = vec![PathSegment::Root(root.clone())];
    segments.extend(identity.into_iter().map(PathSegment::RecordKey));
    segments.push(PathSegment::ChildLayer(layer.clone()));
    segments.extend(layer_keys.into_iter().map(PathSegment::IndexKey));
    let leaf_type = resource_layer_leaf_type(env.program, &root, layer)
        .ok_or_else(|| unsupported("reading this layer", span))?;
    let store = env.store.borrow();
    let Some(bytes) = store
        .read(&encode_path(&segments))
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
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "a keyed lookup with named or out arguments",
            span,
        ));
    }
    let mut identity = Vec::with_capacity(args.len());
    for arg in args {
        identity.push(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("a key of this type", span))?,
        );
    }
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("reading this saved root", span))?;
    let mut prefix = vec![PathSegment::Root(root.clone())];
    prefix.extend(identity.into_iter().map(PathSegment::RecordKey));

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
        let value_type = value_type_for(&field.ty.text)
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
    // `^root(id…).layer(key…).field = v` writes a field inside a keyed GROUP
    // entry: the base is a layer call whose callee is itself a `.layer` access
    // off the record. A plain `^root(id…).field` base is a top-level field write.
    if let Expression::Call { callee, args, .. } = base
        && let Expression::Field {
            base: record,
            name: layer,
            ..
        } = callee.as_ref()
    {
        return eval_group_field_write(record, layer, args, field, value, span, env);
    }
    let (root, identity) = lower_record_identity(base, env)?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("writing to this saved root", span))?;
    let saved = value_to_saved(eval_expr(value, env)?)
        .ok_or_else(|| unsupported("writing a resource value to a field", span))?;
    let plan = {
        let store = env.store.borrow();
        plan_field_write(resource, &identity, field, &saved, &*store).map_err(|error| {
            RuntimeError {
                code: error.code,
                message: error.message,
                span,
            }
        })?
    };
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a managed group-entry field write
/// `^root(key…).layer(key…).field = value`: a single-field update inside a keyed
/// GROUP entry (e.g. `^books(id).versions(v).title`), leaving the entry's other
/// members in place. Lowers the record identity and layer keys, then drives
/// [`marrow_write::plan_layer_field_write`] and commits. Generated indexes do not
/// span keyed child layers, so there is no index interaction.
fn eval_group_field_write(
    record: &Expression,
    layer: &str,
    keys: &[Argument],
    field: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity) = lower_record_identity(record, env)?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("writing to this saved root", span))?;
    let layer_keys = lower_layer_keys(keys, span, env)?;
    let saved = value_to_saved(eval_expr(value, env)?)
        .ok_or_else(|| unsupported("writing a resource value to a field", span))?;
    let plan = plan_layer_field_write(resource, &identity, layer, &layer_keys, field, &saved)
        .map_err(|error| RuntimeError {
            code: error.code,
            message: error.message,
            span,
        })?;
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
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported(
            "assigning a non-resource value to a saved record",
            span,
        ));
    };
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("writing this saved root", span))?;
    let value = resource_value_of(fields, span)?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_write(resource, &identity, &value, &*store).map_err(|error| RuntimeError {
            code: error.code,
            message: error.message,
            span,
        })?
    };
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
}

/// Apply a managed merge `merge ^root(key…) = value`, where `value` is a
/// materialized [`Value::Resource`]: drives [`marrow_write::plan_resource_merge`]
/// (copy supplied fields, keep absent ones) and commits.
fn eval_resource_merge(
    target: &Expression,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity) = lower_record_identity(target, env)?;
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported("merging a non-resource value", span));
    };
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("merging this saved root", span))?;
    let value = resource_value_of(fields, span)?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_merge(resource, &identity, &value, &*store).map_err(|error| RuntimeError {
            code: error.code,
            message: error.message,
            span,
        })?
    };
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
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
    let plan = {
        let store = env.store.borrow();
        plan_layer_merge(resource, &from_identity, &to_identity, layer, &*store).map_err(
            |error| RuntimeError {
                code: error.code,
                message: error.message,
                span,
            },
        )?
    };
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
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
    let (root, identity) = lower_record_identity(path, env)?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("deleting from this saved root", span))?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_delete(resource, &identity, &*store).map_err(|error| RuntimeError {
            code: error.code,
            message: error.message,
            span,
        })?
    };
    plan.commit(&mut *env.store.borrow_mut())
        .map_err(|error| store_error(error, span))?;
    Ok(())
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
    fields
        .into_iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| value)
        .ok_or_else(|| RuntimeError {
            code: RUN_ABSENT,
            message: format!("`{field}` is absent"),
            span,
        })
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
    let Some(Value::Resource(mut fields)) = env.lookup(name).cloned() else {
        return Err(unsupported("setting a field of a non-resource local", span));
    };
    match fields.iter().position(|(existing, _)| existing == field) {
        Some(index) => fields[index].1 = new_value,
        None => fields.push((field.to_string(), new_value)),
    }
    env.assign(name, Value::Resource(fields))
        .map_err(|error| match error {
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
        })
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
        Value::Resource(_) => return None,
    })
}

/// Lower a record path `^root(key…)` to its saved root name and identity key
/// values, evaluating each key argument in `env`.
fn lower_record_identity(
    expr: &Expression,
    env: &mut Env<'_>,
) -> Result<(String, Vec<SavedKey>), RuntimeError> {
    let Expression::Call { callee, args, span } = expr else {
        return Err(unsupported("this saved path", expr.span()));
    };
    let Expression::SavedRoot { name, .. } = callee.as_ref() else {
        return Err(unsupported("this saved path", *span));
    };
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "a keyed lookup with named or out arguments",
            *span,
        ));
    }
    let mut keys = Vec::with_capacity(args.len());
    for arg in args {
        keys.push(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("a key of this type", *span))?,
        );
    }
    Ok((name.clone(), keys))
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
            if args
                .iter()
                .any(|arg| arg.mode.is_some() || arg.name.is_some())
            {
                return Err(unsupported(
                    "a keyed lookup with named or out arguments",
                    *span,
                ));
            }
            let mut segments = lower_saved_path(callee, env)?;
            for arg in args {
                let key = value_to_key(eval_expr(&arg.value, env)?)
                    .ok_or_else(|| unsupported("a key of this type", *span))?;
                segments.push(PathSegment::RecordKey(key));
            }
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
    value_type_for(&field.ty.text)
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
    value_type_for(&layer.leaf_type.as_ref()?.text)
}

/// The declared type of a scalar member field inside a saved root's GROUP layer
/// (e.g. the `string` of `versions(version: int).title`).
fn resource_group_member_type(
    program: &CheckedProgram,
    root: &str,
    layer: &str,
    field: &str,
) -> Option<ValueType> {
    let resource = find_resource(program, root)?;
    let layer = resource
        .layers
        .iter()
        .find(|declared| declared.name == layer)?;
    let member = layer.members.iter().find_map(|member| match member {
        LayerMember::Field(member) if member.name == field => Some(member),
        _ => None,
    })?;
    value_type_for(&member.ty.text)
}

/// The [`ValueType`] a scalar type name denotes, or `None` for a non-scalar type.
fn value_type_for(type_name: &str) -> Option<ValueType> {
    Some(match type_name {
        "bool" => ValueType::Bool,
        "int" => ValueType::Int,
        "string" => ValueType::Str,
        "bytes" => ValueType::Bytes,
        "ErrorCode" => ValueType::ErrorCode,
        "date" => ValueType::Date,
        "instant" => ValueType::Instant,
        "duration" => ValueType::Duration,
        "decimal" => ValueType::Decimal,
        _ => return None,
    })
}

/// Convert a record-key value to a [`SavedKey`], or `None` for a type that is not
/// a key (only int/bool/string are runtime values this slice can key on).
fn value_to_key(value: Value) -> Option<SavedKey> {
    match value {
        Value::Int(n) => Some(SavedKey::Int(n)),
        Value::Bool(b) => Some(SavedKey::Bool(b)),
        Value::Str(s) => Some(SavedKey::Str(s)),
        Value::Instant(n) => Some(SavedKey::Instant(n)),
        Value::Resource(_) => None,
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
        Value::Instant(_) => return Err(unsupported("rendering an instant value", span)),
        Value::Resource(_) => return Err(unsupported("rendering a resource value", span)),
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
        LiteralKind::Decimal | LiteralKind::Bytes => Err(unsupported("this literal type", span)),
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
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, _) => Err(type_error("negation expects an integer", span)),
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
        BinaryOp::Add => int_op(left, right, env, span, i64::checked_add),
        BinaryOp::Subtract => int_op(left, right, env, span, i64::checked_sub),
        BinaryOp::Multiply => int_op(left, right, env, span, i64::checked_mul),
        BinaryOp::Divide => int_div(left, right, env, span, i64::checked_div),
        BinaryOp::Remainder => int_div(left, right, env, span, i64::checked_rem),
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

/// Apply a checked integer operation, mapping `None` (overflow) to `run.overflow`.
fn int_op(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
    op: fn(i64, i64) -> Option<i64>,
) -> Result<Value, RuntimeError> {
    let a = eval_int(left, env)?;
    let b = eval_int(right, env)?;
    op(a, b).map(Value::Int).ok_or_else(|| overflow(span))
}

/// Apply a checked division/remainder, rejecting a zero divisor and the
/// `i64::MIN / -1` overflow.
fn int_div(
    left: &Expression,
    right: &Expression,
    env: &mut Env<'_>,
    span: SourceSpan,
    op: fn(i64, i64) -> Option<i64>,
) -> Result<Value, RuntimeError> {
    let a = eval_int(left, env)?;
    let b = eval_int(right, env)?;
    if b == 0 {
        return Err(RuntimeError {
            code: RUN_DIVIDE_BY_ZERO,
            message: "integer division or remainder by zero".into(),
            span,
        });
    }
    op(a, b).map(Value::Int).ok_or_else(|| overflow(span))
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
