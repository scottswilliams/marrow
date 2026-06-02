//! The function-call spine: invocation, argument binding, and call evaluation.

use std::cell::RefCell;
use std::rc::Rc;

use marrow_check::{
    CheckedFunction, CheckedModule, CheckedParam, CheckedProgram, Def, DefItem, FileId, MarrowType,
    Resolution, ResolvableKind, resolve,
};
use marrow_schema::stdlib::Capability;
use marrow_schema::{KeyDef, Node, NodeKind, ResourceSchema, Type};
use marrow_store::Decimal;
use marrow_store::backend::Backend;
use marrow_store::path::SavedKey;
use marrow_store::value::ScalarType;
use marrow_syntax::{ArgMode, Argument, Block, Expression, ParamMode, SourceSpan};

use crate::collection::{
    Direction, eval_append, eval_entries, eval_keys, eval_neighbor, eval_next_id, eval_reversed,
    eval_values,
};
use crate::env::{Context, Env, Flow, TransactionState};
use crate::error::{
    RUN_NO_ENCLOSING_LOOP, RUN_PRIVATE_FUNCTION, RUN_TYPE, RUN_UNBOUND_NAME, RUN_UNCAUGHT_THROW,
    RUN_UNKNOWN_FUNCTION, RuntimeError, assign_error, raise, reraise_fault, type_error,
    unknown_function, unsupported,
};
use crate::exec::eval_block;
use crate::expr::eval_expr;
use crate::host::{Host, StepHook};
use crate::path::{SavedPath, Terminal, lower};
use crate::read::{eval_index_lookup, eval_resource_read, eval_saved_layer_read, read_local_field};
use crate::schema_query::{
    enum_in, find_store_resource, identity_key_defs, identity_root, is_saved_path,
};
use crate::stdlib::{
    eval_assert, eval_bytes_conversion, eval_clock_capability, eval_conversion, eval_count,
    eval_env, eval_error_constructor, eval_exists, eval_io, eval_log, eval_output, eval_std,
    is_std_module,
};
use crate::value::{RunOutput, Value, value_to_key};
use crate::write_dispatch::write_local_field;

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
    // An ordinary run installs no debugger hook.
    run_entry_impl(program, store, host, entry, args, None)
}

/// Like [`run_entry_with_host`], but installs an opt-in statement debugger: the
/// `hook` is called before each statement the run evaluates (see [`StepHook`]),
/// so a debug adapter can step and inspect [`Frame`]s. The only behavioral
/// difference from a plain run is those hook calls; returning `Err` from the hook
/// terminates the run with that error.
pub fn run_entry_with_debugger(
    program: &CheckedProgram,
    store: &RefCell<dyn Backend>,
    host: &Host,
    hook: &mut dyn StepHook,
    entry: &str,
    args: &[Value],
) -> Result<RunOutput, RuntimeError> {
    run_entry_impl(program, store, host, entry, args, Some(hook))
}

/// The shared entry path for [`run_entry_with_host`] and
/// [`run_entry_with_debugger`]: resolve `entry`, bind its parameters, and run its
/// body as the depth-1 activation, optionally threading a debugger `hook`. With
/// `hook` `None` this is exactly the former pre-debugger behavior.
//
// The borrowed run state and the hook share one lifetime `'p`: [`invoke`] stores
// the hook in the `Env<'p>` alongside the `'p`-borrowed context, and the `&mut`
// in the hook is invariant, so the compiler cannot shrink the state's lifetime to
// fit the hook's. Binding both to `'p` is sound — the caller's borrows all outlive
// this call.
pub(crate) fn run_entry_impl<'p>(
    program: &'p CheckedProgram,
    store: &'p RefCell<dyn Backend>,
    host: &'p Host,
    entry: &str,
    args: &[Value],
    hook: Option<&'p mut dyn StepHook>,
) -> Result<RunOutput, RuntimeError> {
    let segments: Vec<String> = entry.split("::").map(str::to_string).collect();
    // The entry is the root activation, invoked by the host directly, so it is
    // resolved as a bare name *from its own module* — visible regardless of `pub`,
    // exactly as the module would see it. `module::function` splits into that
    // module and the bare function name.
    let (leaf, module_prefix) = segments
        .split_last()
        .ok_or_else(|| unknown_function(entry, SourceSpan::default()))?;
    let entry_module = module_prefix.join("::");
    let (module, function) = resolve_program_function(
        program,
        &entry_module,
        std::slice::from_ref(leaf),
        SourceSpan::default(),
    )
    .map_err(|_| unknown_function(entry, SourceSpan::default()))?;
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
        transaction: Rc::new(RefCell::new(TransactionState::default())),
    };
    let value = match invoke(
        ctx,
        Rc::clone(&output),
        Some(module),
        &names,
        &function.body,
        function.span,
        args,
        &[],
        &[],
        &[],
        hook,
        1,
    )? {
        (Completion::Returned(value), ..) => value,
        (Completion::Threw { error, origin }, ..) => {
            return Err(raise(error, function.span, origin));
        }
        (
            Completion::Faulted {
                error,
                code,
                span,
                origin,
            },
            ..,
        ) => {
            return Err(reraise_fault(error, code, span, origin));
        }
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
pub(crate) enum Completion {
    Returned(Option<Value>),
    Threw {
        error: Value,
        /// The file the throw was first raised in. A catchable error is rebuilt
        /// at each call boundary it crosses, so this rides the completion to keep
        /// the deepest (raising) file rather than re-deriving it at each frame.
        origin: Option<FileId>,
    },
    /// A catchable fault (e.g. `write.unique_conflict`, `run.overflow`,
    /// `run.absent_element`) that escaped a called function uncaught. It crosses
    /// the call boundary as a catchable error like a throw, but keeps its own
    /// dotted `code` and source span so an uncaught fault surfaces as itself.
    Faulted {
        error: Value,
        code: &'static str,
        span: SourceSpan,
        origin: Option<FileId>,
    },
}

/// What an activation hands back: how it finished, each `out`/`inout` final
/// value, and the debugger hook moved back out so the caller keeps stepping.
pub(crate) type Activation<'p> = (Completion, Vec<Option<Value>>, Option<&'p mut dyn StepHook>);

/// Bind `args` to `param_names`, evaluate `body` in a fresh activation at call
/// `depth`, and surface how it finished plus, for each `out`/`inout` parameter
/// named in `writeback`, its final value (param-order-aligned, `Some` only when
/// the body returned normally — a throw or fault skips write-back). Shared by
/// [`run_entry`] and call evaluation; non-`out`/`inout` calls pass an empty
/// `writeback`.
///
/// Traversal guards carry the caller's active saved-layer and generated-index
/// prefixes across helper calls, so dynamic writes are checked the same way
/// direct writes in the loop body are checked.
///
/// The optional `hook` is the opt-in debugger; it is moved into this activation
/// and, on every non-fatal outcome, moved back out in the returned tuple so the
/// caller can keep stepping after the call returns. A fatal `Err` aborts the run
/// and drops the borrow with it. Moving the `&mut` (rather than reborrowing)
/// preserves its `'p` lifetime exactly, so no `unsafe` is needed.
// The activation's inputs are independent (context, output, the module this
// activation runs in, parameter names, body, span, args, write-back set, debugger
// hook, call depth); bundling them would not aid clarity.
#[allow(clippy::too_many_arguments)]
pub(crate) fn invoke<'p>(
    ctx: Context<'p>,
    output: Rc<RefCell<String>>,
    module: Option<&'p CheckedModule>,
    param_names: &[&str],
    body: &Block,
    span: SourceSpan,
    args: &[Value],
    writeback: &[&str],
    traversed_layers: &[Vec<u8>],
    traversed_index_layers: &[Vec<u8>],
    hook: Option<&'p mut dyn StepHook>,
    depth: usize,
) -> Result<Activation<'p>, RuntimeError> {
    if args.len() != param_names.len() {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            format!(
                "expected {} argument(s), got {}",
                param_names.len(),
                args.len()
            ),
            span,
        ));
    }
    let mut env = Env::new(ctx, output, module, hook, depth);
    env.traversed_layers = traversed_layers.to_vec();
    env.traversed_index_layers = traversed_index_layers.to_vec();
    env.push_scope();
    if let Some(module) = module {
        for constant in &module.constants {
            if let Some(value) = &constant.value {
                let value = eval_expr(value, &mut env)?;
                env.bind(constant.name.clone(), value, false);
            }
        }
    }
    for (name, arg) in param_names.iter().zip(args) {
        // `out`/`inout` parameters are reassignable inside the callee; plain
        // parameters are read-only.
        env.bind((*name).to_string(), arg.clone(), writeback.contains(name));
    }
    let outcome = eval_block(body, &mut env);
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
    // The file every fault leaving this activation belongs to if it has not
    // already been stamped by a deeper frame. Resolved once on the cold path.
    let here = || module.and_then(|module| env.program.file_id_of(module));
    let completion = match outcome {
        Ok(Flow::Return(value)) => Completion::Returned(value),
        Ok(Flow::Normal) => Completion::Returned(None),
        // A `throw` raised directly in this body originates in this file.
        Ok(Flow::Throw(value)) => Completion::Threw {
            error: value,
            origin: here(),
        },
        // A catchable error escaped a function this body called: its thrown `Error`
        // rides the `Err` channel's `throw` field. Surface it as this activation's
        // own outcome so the caller's `try` can bind it — a language throw
        // (`run.uncaught_error`) as `Threw`, a recoverable fault as `Faulted` with
        // its own dotted code preserved. A fault with no `throw` value is fatal and
        // passes straight through. The origin a deeper frame already recorded is
        // kept; only an unstamped one takes this frame's file.
        Err(RuntimeError {
            throw: Some(error),
            code: RUN_UNCAUGHT_THROW,
            origin,
            ..
        }) => Completion::Threw {
            error: *error,
            origin: origin.or_else(here),
        },
        Err(RuntimeError {
            throw: Some(error),
            code,
            span,
            origin,
            ..
        }) => Completion::Faulted {
            error: *error,
            code,
            span,
            origin: origin.or_else(here),
        },
        Err(fatal) => return Err(fatal.with_origin_from(env.program, module)),
        Ok(Flow::Break(_)) | Ok(Flow::Continue(_)) => {
            return Err(RuntimeError::fault(
                RUN_NO_ENCLOSING_LOOP,
                "`break` or `continue` outside a loop".into(),
                span,
            )
            .with_origin_from(env.program, module));
        }
    };
    // Hand the debugger hook back to the caller so it can keep stepping after this
    // activation. It is `None` for an ordinary run.
    Ok((completion, finals, env.hook.take()))
}

/// Resolve a program-function call from `env`'s module through the unified
/// resolver, mapping the outcome to a runtime `Result`: a `pub`-or-own-module
/// match yields its `(module, function)`; a non-`pub` cross-module target is a
/// distinct [`RUN_PRIVATE_FUNCTION`] fault; anything else is the usual
/// [`RUN_UNKNOWN_FUNCTION`]. Builtins/std are dispatched before this is reached.
pub(crate) fn resolve_program_function<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    segments: &[String],
    span: SourceSpan,
) -> Result<(&'p CheckedModule, &'p CheckedFunction), RuntimeError> {
    match resolve(program, from_module, segments, ResolvableKind::Function) {
        Resolution::Found(Def {
            module,
            item: DefItem::Function(function),
            ..
        }) => Ok((module, function)),
        Resolution::NotVisible(name) => Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_PRIVATE_FUNCTION,
            message: format!("function `{name}` is private to its module"),
            span,
        }),
        _ => Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_UNKNOWN_FUNCTION,
            message: format!("the program has no function `{}`", segments.join("::")),
            span,
        }),
    }
}

/// Bind a call's positional and named arguments to a function's parameters,
/// returning the argument values in parameter order. Positional arguments fill
/// parameters left to right and must precede any named argument; a named
/// argument binds the parameter of that name. Each parameter must be supplied
/// exactly once. This is the plain (by-value) path; a call carrying `out`/`inout`
/// arguments goes through [`bind_arguments_with_modes`] instead.
pub(crate) fn bind_arguments(
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
/// `None` otherwise) and validates that argument and parameter modes agree.
/// Local `inout` places are read now to seed the parameter; an `out` parameter is
/// seeded with a type-directed default it is expected to overwrite.
pub(crate) fn bind_arguments_with_modes(
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
                if is_saved_path(&arg.value) {
                    return Err(unsupported("saved `inout`", span));
                }
                let place = resolve_place(&arg.value, span, env)?;
                let current = place.read(span, env)?;
                (current, Some(place))
            }
            Some(ArgMode::Out) => {
                // `out` does not read the place, so it need not exist yet.
                let place = resolve_place(&arg.value, span, env)?;
                (out_seed(&param.ty), Some(place))
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
pub(crate) fn arg_param_index(
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
pub(crate) fn place_argument<T>(
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
pub(crate) fn collect_arguments<T>(
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

fn resolve_resource_value_constructor<'p>(
    program: &'p CheckedProgram,
    from_module: &str,
    segments: &[String],
) -> Option<(&'p CheckedModule, &'p ResourceSchema)> {
    match resolve(program, from_module, segments, ResolvableKind::Resource) {
        Resolution::Found(Def {
            module,
            item: DefItem::Resource(resource),
            ..
        }) => Some((module, resource)),
        _ => None,
    }
}

fn eval_resource_constructor(
    module: &CheckedModule,
    resource: &ResourceSchema,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let fields: Vec<&Node> = resource
        .members
        .iter()
        .filter(|node| node.is_plain_field())
        .collect();
    let mut slots: Vec<Option<Value>> = vec![None; fields.len()];

    for arg in args {
        if arg.mode.is_some() {
            return Err(type_error(
                &format!("`{}(...)` fields cannot be out arguments", resource.name),
                span,
            ));
        }
        let Some(name) = &arg.name else {
            return Err(type_error(
                &format!("`{}(...)` takes named fields", resource.name),
                span,
            ));
        };
        let index = fields
            .iter()
            .position(|field| &field.name == name)
            .ok_or_else(|| {
                type_error(&format!("`{}` has no field `{name}`", resource.name), span)
            })?;
        if slots[index].is_some() {
            return Err(type_error(
                &format!("field `{name}` is supplied more than once"),
                span,
            ));
        }
        let value = eval_expr(&arg.value, env)?;
        check_resource_constructor_value(
            env.program,
            module,
            resource,
            fields[index],
            &value,
            span,
        )?;
        slots[index] = Some(value);
    }

    for (field, slot) in fields.iter().zip(&slots) {
        if slot.is_none()
            && let NodeKind::Slot { required: true, .. } = field.kind
        {
            return Err(type_error(
                &format!("`{}` requires `{}`", resource.name, field.name),
                span,
            ));
        }
    }

    Ok(Value::Resource(
        fields
            .into_iter()
            .zip(slots)
            .filter_map(|(field, value)| value.map(|value| (field.name.clone(), value)))
            .collect(),
    ))
}

fn check_resource_constructor_value(
    program: &CheckedProgram,
    module: &CheckedModule,
    resource: &ResourceSchema,
    field: &Node,
    value: &Value,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    let ty = field.plain_field_type().ok_or_else(|| {
        type_error(
            &format!("`{}` has no field `{}`", resource.name, field.name),
            span,
        )
    })?;
    let expected = runtime_type_from_schema(program, module, ty);
    let accepted = value_matches_type(program, &expected, value);
    if accepted {
        Ok(())
    } else {
        Err(type_error(
            &format!("field `{}` has the wrong type", field.name),
            span,
        ))
    }
}

fn runtime_type_from_schema(
    program: &CheckedProgram,
    module: &CheckedModule,
    ty: &Type,
) -> MarrowType {
    match ty {
        Type::Scalar(scalar) => MarrowType::Primitive(*scalar),
        Type::Sequence(element) => {
            MarrowType::Sequence(Box::new(runtime_type_from_schema(program, module, element)))
        }
        Type::Identity(identity) => MarrowType::Identity(
            identity_root(program, identity).unwrap_or_else(|| identity.clone()),
        ),
        Type::Unknown => MarrowType::Unknown,
        Type::Named(name) if name == "Error" => MarrowType::Error,
        Type::Named(name)
            if module
                .resources
                .iter()
                .any(|resource| &resource.name == name) =>
        {
            MarrowType::Resource(name.clone())
        }
        Type::Named(name) if module.enums.iter().any(|enum_| &enum_.name == name) => {
            MarrowType::Enum {
                module: module.name.clone(),
                name: name.clone(),
            }
        }
        Type::Named(_) => MarrowType::Unknown,
    }
}

fn value_matches_type(program: &CheckedProgram, expected: &MarrowType, value: &Value) -> bool {
    match expected {
        MarrowType::Primitive(scalar) => value_scalar_type(value) == Some(*scalar),
        MarrowType::Identity(identity) => identity_value_matches(program, identity, value),
        MarrowType::Resource(_) | MarrowType::GroupEntry { .. } => {
            matches!(value, Value::Resource(_))
        }
        MarrowType::Enum { module, name } => {
            let Value::Int(ordinal) = value else {
                return false;
            };
            let Ok(ordinal) = usize::try_from(*ordinal) else {
                return false;
            };
            enum_in(program, module, name)
                .and_then(|schema| schema.members.get(ordinal))
                .is_some_and(|member| !member.category)
        }
        MarrowType::Sequence(element) => match value {
            Value::Sequence(items) => items
                .iter()
                .all(|item| value_matches_type(program, element, item)),
            _ => false,
        },
        MarrowType::LocalTree { value: element, .. } => match value {
            Value::LocalTree(entries) => entries
                .iter()
                .all(|entry| value_matches_type(program, element, &entry.value)),
            _ => false,
        },
        MarrowType::Error => matches!(value, Value::Resource(_)),
        MarrowType::Invalid => true,
        MarrowType::Unknown => true,
    }
}

fn value_scalar_type(value: &Value) -> Option<ScalarType> {
    match value {
        Value::Int(_) => Some(ScalarType::Int),
        Value::Bool(_) => Some(ScalarType::Bool),
        Value::Str(_) => Some(ScalarType::Str),
        Value::Instant(_) => Some(ScalarType::Instant),
        Value::Date(_) => Some(ScalarType::Date),
        Value::Duration(_) => Some(ScalarType::Duration),
        Value::Decimal(_) => Some(ScalarType::Decimal),
        Value::Bytes(_) => Some(ScalarType::Bytes),
        Value::Sequence(_) | Value::LocalTree(_) | Value::Resource(_) | Value::Identity(_) => None,
    }
}

fn identity_value_matches(program: &CheckedProgram, identity: &str, value: &Value) -> bool {
    let Some(identity_keys) = identity_key_defs(program, identity) else {
        return false;
    };
    match value {
        Value::Identity(keys) => identity_keys_match(identity_keys, keys),
        other if identity_keys.len() == 1 => value_to_key(other.clone())
            .is_some_and(|key| identity_keys_match(identity_keys, &[key])),
        _ => false,
    }
}

fn identity_keys_match(declared: &[KeyDef], keys: &[SavedKey]) -> bool {
    declared.len() == keys.len()
        && declared
            .iter()
            .zip(keys)
            .all(|(declared, key)| match declared.ty.scalar() {
                Some(expected) => expected == key.scalar_type(),
                None => true,
            })
}

/// Whether an argument's mode matches a parameter's: both plain, both `out`, or
/// both `inout`.
pub(crate) fn modes_match(arg: Option<ArgMode>, param: Option<ParamMode>) -> bool {
    matches!(
        (arg, param),
        (None, None)
            | (Some(ArgMode::Out), Some(ParamMode::Out))
            | (Some(ArgMode::InOut), Some(ParamMode::InOut))
    )
}

/// A resolved assignable place for an `out`/`inout` argument, captured before the
/// call (its saved identity keys evaluated once) so local `inout` can be read and
/// write-back does not re-evaluate those keys. A saved target is held as a
/// lowered [`SavedPath`] with a known terminal, so `out` write-back routes
/// through the same path model a direct assignment uses.
pub(crate) enum Place {
    /// A bare local variable: `n` or `book`.
    Local(String),
    /// A field of a local resource variable: `book.title`.
    LocalField { base: String, field: String },
    /// A saved target: a scalar field (`^books(id).title`), a nested group field
    /// (`^books(id).versions(v).text`), or a whole record (`^books(id)`), as a
    /// lowered path whose terminal records which.
    Saved(SavedPath),
}

/// Resolve an `out`/`inout` argument expression to its [`Place`], evaluating any
/// saved identity keys now. Supports a bare local, a field of a local resource, a
/// saved scalar field, and a whole saved resource for `out`; other shapes defer.
pub(crate) fn resolve_place(
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
            // entry. Lowering the base yields a record path; peel the trailing
            // `.field` onto it as the path's terminal (the layer chain, empty for a
            // top-level field, is already carried by the lowered path).
            let path = lower(base, env)?.into_field(name.clone(), base.span())?;
            Ok(Place::Saved(path))
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
            let path = lower(expr, env)?;
            // A whole saved record is the only assignable call here: a layer chain
            // or index branch has no whole-record value to read or write back.
            if !path.layers.is_empty() || !matches!(path.terminal, Terminal::Record) {
                return Err(unsupported("this saved path", expr.span()));
            }
            Ok(Place::Saved(path))
        }
        _ => Err(unsupported(
            "an out/inout argument that is not an assignable place",
            span,
        )),
    }
}

impl Place {
    /// The current local value at this place, to seed an `inout` parameter.
    pub(crate) fn read(&self, span: SourceSpan, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
        match self {
            Place::Local(name) => env.lookup(name).cloned().ok_or_else(|| RuntimeError {
                throw: None,
                origin: None,
                code: RUN_UNBOUND_NAME,
                message: format!("`{name}` is not bound"),
                span,
            }),
            Place::LocalField { base, field } => read_local_field(base, field, span, env),
            Place::Saved(_) => Err(unsupported("saved `inout`", span)),
        }
    }

    /// Write `value` back to this place after the callee returns normally.
    pub(crate) fn write(
        self,
        value: Value,
        span: SourceSpan,
        env: &mut Env<'_>,
    ) -> Result<(), RuntimeError> {
        match self {
            Place::Local(name) => env
                .assign(&name, value)
                .map_err(|error| assign_error(&name, error, span)),
            Place::LocalField { base, field } => write_local_field(&base, &field, value, span, env),
            Place::Saved(path) => path.write(value, span, env),
        }
    }
}

/// The default value of a resolved type: the empty sequence, a scalar zero, or
/// `None` for a type with no representable default (an identity, an unresolved
/// name, or `unknown`). Resource types are handled by the caller (an empty
/// resource), since the schema-blind [`Type`] places a resource as `Named`.
pub(crate) fn default_value(ty: &Type) -> Option<Value> {
    Some(match ty {
        Type::Sequence(_) => Value::Sequence(Vec::new()),
        Type::Scalar(ScalarType::Int) => Value::Int(0),
        Type::Scalar(ScalarType::Bool) => Value::Bool(false),
        Type::Scalar(ScalarType::Str) => Value::Str(String::new()),
        Type::Scalar(ScalarType::Bytes) => Value::Bytes(Vec::new()),
        Type::Scalar(ScalarType::Date) => Value::Date(0),
        Type::Scalar(ScalarType::Instant) => Value::Instant(0),
        Type::Scalar(ScalarType::Duration) => Value::Duration(0),
        Type::Scalar(ScalarType::Decimal) => Value::Decimal(Decimal::parse("0")?),
        Type::Identity(_) | Type::Named(_) | Type::Unknown => return None,
    })
}

/// The value an `out` parameter is seeded with before the callee assigns it. A
/// correct callee assigns it before returning (a checker rule to require this is
/// still pending), so the placeholder is normally unobserved. Only the four
/// scalars with a simple zero seed to that zero; any other type — a temporal or
/// decimal scalar, a sequence, a resource, an identity — starts as an empty
/// resource.
pub(crate) fn out_seed(ty: &MarrowType) -> Value {
    let zero = match ty {
        MarrowType::Primitive(
            scalar @ (ScalarType::Int | ScalarType::Bool | ScalarType::Str | ScalarType::Bytes),
        ) => default_value(&Type::Scalar(*scalar)),
        _ => None,
    };
    zero.unwrap_or_else(|| Value::Resource(Vec::new()))
}

/// Evaluate a call to a program function, returning its returned value (or
/// `None` for a function that returns nothing). Arguments may be positional or
/// named; local `inout` and supported `out` arguments write back after the call.
pub(crate) fn eval_call(
    callee: &Expression,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    // A call whose callee names a declared index off a saved root
    // (`^books.byIsbn(isbn)`) is an index lookup, not a keyed-layer read.
    if let Expression::Field { base, name, .. } = callee
        && let Expression::SavedRoot { name: root, .. } = base.as_ref()
        && let Some((store, _)) = find_store_resource(env.program, root)
        && let Some(index) = store.indexes.iter().find(|index| &index.name == name)
    {
        return eval_index_lookup(store, index, args, span, env).map(Some);
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
    // Expand a short-form leading segment through the active module's import
    // aliases once, up front, so `clock::now()` dispatches like
    // `std::clock::now()` and `books::add(...)` like `shelf::books::add(...)`,
    // matching the checker (`marrow_check::expand_alias`). All builtin/std/function
    // dispatch below uses the expanded form; with no aliases it is a no-op.
    let segments = marrow_check::expand_alias(segments, &env.aliases);
    // `Error(...)` is the builtin error constructor (named arguments), not a
    // program function.
    if let [name] = segments.as_slice()
        && name == "Error"
    {
        return eval_error_constructor(args, span, env).map(Some);
    }
    if let Some((module, resource)) =
        resolve_resource_value_constructor(env.program, env.module, &segments)
    {
        return eval_resource_constructor(module, resource, args, span, env).map(Some);
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
                "nextId" => return eval_next_id(args, span, env).map(Some),
                "append" => return eval_append(args, span, env).map(Some),
                "bytes" => return eval_bytes_conversion(args, span, env).map(Some),
                other if ScalarType::from_scalar_name(other).is_some() => {
                    return eval_conversion(other, args, span, env).map(Some);
                }
                // `keys(<layer>)` materializes the layer's child keys as a sequence
                // value. Direct loops use this enumeration only for address-only
                // collections such as index branches.
                "keys" => return eval_keys(args, span, env).map(Some),
                // `count(path)` is a one-layer tree scan over the lowered path.
                "count" => return eval_count(args, span, env).map(Some),
                // `values`/`entries` materialize each child's value (a whole record
                // for a primary root, an entry value for a keyed layer); `entries`
                // pairs it with the key for the two-name `for k, v in ...` binding.
                "values" => return eval_values(args, span, env).map(Some),
                "entries" => return eval_entries(args, span, env).map(Some),
                // `reversed(<iterable>)` yields the same elements in reverse key
                // order; `next`/`prev` return the nearest stored neighbor identity.
                "reversed" => return eval_reversed(args, span, env).map(Some),
                "next" => return eval_neighbor(Direction::Ascending, args, span, env).map(Some),
                "prev" => return eval_neighbor(Direction::Descending, args, span, env).map(Some),
                _ => {}
            }
        }
        // `std::<module>::<op>` is a builtin module call. A recognized op routes
        // to its descriptor's capability family in the shared stdlib table; an
        // unrecognized op under a known module still routes by module so its
        // handler raises the same `unsupported` error as before, and an unknown
        // module falls through to the program-function dispatch.
        if let [first, second, op] = segments.as_slice()
            && first == "std"
        {
            match marrow_schema::stdlib::lookup(second, op).map(|entry| entry.capability) {
                Some(Capability::Clock) => {
                    return eval_clock_capability(op, args, span, env).map(Some);
                }
                Some(Capability::Env) => return eval_env(op, args, span, env).map(Some),
                Some(Capability::Log) => return eval_log(op, args, span, env),
                Some(Capability::Io) => return eval_io(op, args, span, env),
                Some(Capability::Assert) => return eval_assert(op, args, span, env),
                Some(Capability::Pure) => return eval_std(second, op, args, span, env).map(Some),
                // An unrecognized op keeps the by-module routing so its handler
                // reports the same error: the capability modules first check the
                // host capability, so an unknown op under them faults the same way
                // a recognized one would; every other known module is pure and
                // routes to `eval_std`. Knownness comes from the shared table, so
                // there is no hand-kept module list to drift from it. An unknown
                // module falls through to function dispatch.
                None => match second.as_str() {
                    "env" => return eval_env(op, args, span, env).map(Some),
                    "log" => return eval_log(op, args, span, env),
                    "io" => return eval_io(op, args, span, env),
                    "assert" => return eval_assert(op, args, span, env),
                    other if is_std_module(other) => {
                        return eval_std(second, op, args, span, env).map(Some);
                    }
                    _ => {}
                },
            }
        }
    }
    let ctx = Context {
        program: env.program,
        store: env.store,
        host: env.host,
        transaction: Rc::clone(&env.transaction),
    };
    // Resolve the call from this activation's module: a bare name in its own
    // module, a qualified `mod::fn` elsewhere (which must be `pub`). The resolved
    // module seeds the callee's activation, so its own short-form imports expand.
    let (module, function) = resolve_program_function(ctx.program, env.module, &segments, span)?;
    if has_moded {
        return eval_call_with_modes(module, function, args, span, env);
    }
    let values = bind_arguments(&function.params, args, span, env)?;
    let names: Vec<&str> = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    // Move the debugger hook into the callee and the depth one deeper, then move
    // the hook back so the caller keeps stepping after the call returns.
    let depth = env.depth;
    let traversed_layers = env.traversed_layers.clone();
    let traversed_index_layers = env.traversed_index_layers.clone();
    let (completion, _, hook) = invoke(
        ctx,
        Rc::clone(&env.output),
        Some(module),
        &names,
        &function.body,
        function.span,
        &values,
        &[],
        &traversed_layers,
        &traversed_index_layers,
        env.hook.take(),
        depth + 1,
    )?;
    env.hook = hook;
    complete_call(completion, span)
}

/// Turn a callee's [`Completion`] into this activation's result: a normal return
/// yields its value; an uncaught throw or recoverable fault is re-raised as a
/// catchable error riding the `Err` channel's `throw` value, consumed by the
/// nearest `try` or this activation's [`invoke`].
pub(crate) fn complete_call(
    completion: Completion,
    span: SourceSpan,
) -> Result<Option<Value>, RuntimeError> {
    match completion {
        Completion::Returned(value) => Ok(value),
        Completion::Threw { error, origin } => Err(raise(error, span, origin)),
        Completion::Faulted {
            error,
            code,
            span: fault_span,
            origin,
        } => Err(reraise_fault(error, code, fault_span, origin)),
    }
}

/// Evaluate a program-function call that has `out`/`inout` arguments. Each moded
/// argument resolves to an assignable [`Place`]; local `inout` places are read to
/// seed the parameter, and all supported moded places are written back after the
/// callee returns normally. The callee's throw or fault skips write-back. The
/// argument's mode must match the parameter's.
pub(crate) fn eval_call_with_modes<'p>(
    module: &'p CheckedModule,
    function: &'p CheckedFunction,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'p>,
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
        transaction: Rc::clone(&env.transaction),
    };
    // Move the debugger hook into the callee and the depth one deeper, then move
    // the hook back so the caller keeps stepping after the call returns.
    let depth = env.depth;
    let traversed_layers = env.traversed_layers.clone();
    let traversed_index_layers = env.traversed_index_layers.clone();
    let (completion, finals, hook) = invoke(
        ctx,
        Rc::clone(&env.output),
        Some(module),
        &names,
        &function.body,
        function.span,
        &values,
        &writeback,
        &traversed_layers,
        &traversed_index_layers,
        env.hook.take(),
        depth + 1,
    )?;
    env.hook = hook;
    // Write each out/inout parameter's final value back to its place. On a throw
    // or fault `finals` is all `None`, so nothing is written.
    for (place, final_value) in places.into_iter().zip(finals) {
        if let (Some(place), Some(value)) = (place, final_value) {
            place.write(value, span, env)?;
        }
    }
    complete_call(completion, span)
}

#[cfg(test)]
mod default_value_tests {
    use marrow_check::MarrowType;
    use marrow_schema::Type;
    use marrow_store::Decimal;
    use marrow_store::value::ScalarType;

    use crate::call::{default_value, out_seed};
    use crate::value::Value;

    // Pin the `var` defaults exhaustively against the values the dedicated
    // uninitialized-default table produced before it was folded into one fn.
    #[test]
    fn var_default_matches_the_old_table() {
        assert_eq!(
            default_value(&Type::Scalar(ScalarType::Int)),
            Some(Value::Int(0))
        );
        assert_eq!(
            default_value(&Type::Scalar(ScalarType::Bool)),
            Some(Value::Bool(false))
        );
        assert_eq!(
            default_value(&Type::Scalar(ScalarType::Str)),
            Some(Value::Str(String::new()))
        );
        assert_eq!(
            default_value(&Type::Scalar(ScalarType::Bytes)),
            Some(Value::Bytes(Vec::new()))
        );
        assert_eq!(
            default_value(&Type::Scalar(ScalarType::Date)),
            Some(Value::Date(0))
        );
        assert_eq!(
            default_value(&Type::Scalar(ScalarType::Instant)),
            Some(Value::Instant(0))
        );
        assert_eq!(
            default_value(&Type::Scalar(ScalarType::Duration)),
            Some(Value::Duration(0))
        );
        assert_eq!(
            default_value(&Type::Scalar(ScalarType::Decimal)),
            Some(Value::Decimal(Decimal::parse("0").unwrap()))
        );
        assert_eq!(
            default_value(&Type::Sequence(Box::new(Type::Scalar(ScalarType::Int)))),
            Some(Value::Sequence(Vec::new()))
        );
        assert_eq!(default_value(&Type::Identity("books".into())), None);
        assert_eq!(default_value(&Type::Named("Book".into())), None);
        assert_eq!(default_value(&Type::Unknown), None);
    }

    // Pin the `out` seed exhaustively against the values the dedicated zero-value
    // table produced: only Int/Bool/Str/Bytes seed to a scalar zero; every other
    // type (temporal, decimal, sequence, resource, identity, unknown) is an empty
    // resource. The fold must not widen the seed to the new scalar defaults.
    #[test]
    fn out_seed_matches_the_old_table() {
        assert_eq!(
            out_seed(&MarrowType::Primitive(ScalarType::Int)),
            Value::Int(0)
        );
        assert_eq!(
            out_seed(&MarrowType::Primitive(ScalarType::Bool)),
            Value::Bool(false)
        );
        assert_eq!(
            out_seed(&MarrowType::Primitive(ScalarType::Str)),
            Value::Str(String::new())
        );
        assert_eq!(
            out_seed(&MarrowType::Primitive(ScalarType::Bytes)),
            Value::Bytes(Vec::new())
        );
        let empty = Value::Resource(Vec::new());
        assert_eq!(out_seed(&MarrowType::Primitive(ScalarType::Date)), empty);
        assert_eq!(out_seed(&MarrowType::Primitive(ScalarType::Instant)), empty);
        assert_eq!(
            out_seed(&MarrowType::Primitive(ScalarType::Duration)),
            empty
        );
        assert_eq!(out_seed(&MarrowType::Primitive(ScalarType::Decimal)), empty);
        assert_eq!(
            out_seed(&MarrowType::Sequence(Box::new(MarrowType::Primitive(
                ScalarType::Int
            )))),
            empty
        );
        assert_eq!(out_seed(&MarrowType::Resource("Book".into())), empty);
        assert_eq!(
            out_seed(&MarrowType::GroupEntry {
                resource: "Book".into(),
                layers: vec!["versions".into()],
            }),
            empty
        );
        assert_eq!(out_seed(&MarrowType::Identity("books".into())), empty);
        assert_eq!(out_seed(&MarrowType::Error), empty);
        assert_eq!(out_seed(&MarrowType::Unknown), empty);
    }
}
