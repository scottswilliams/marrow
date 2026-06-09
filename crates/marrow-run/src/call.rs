//! The function-call spine: invocation, argument binding, and call evaluation.

use std::rc::Rc;

use marrow_check::{
    CheckedArg as ExecArg, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr as ExecExpr,
    CheckedFunctionRef, CheckedRuntimeFunction, CheckedRuntimeModule, CheckedRuntimeProgram,
};
use marrow_schema::stdlib::Capability;
use marrow_syntax::SourceSpan;

use crate::activation::{Completion, Invocation, complete_call, executable_body, invoke};
use crate::call_args::{bind_arguments, bind_arguments_with_modes, eval_resource_constructor};
use crate::collection::{
    Direction, eval_append, eval_entries, eval_keys, eval_next_id, eval_reversed, eval_values,
};
use crate::durable_read::{eval_index_lookup, eval_resource_read, eval_saved_layer_read};
use crate::env::{Context, Env};
use crate::error::{RUN_UNKNOWN_FUNCTION, RuntimeError, unsupported};
use crate::host_effects::{eval_clock_capability, eval_env, eval_io, eval_log};
use crate::local_collection::eval_local_collection_read;
use crate::neighbor::eval_neighbor;
use crate::std_pure::eval_std;
use crate::stdlib::{
    ConversionKind, OutputKind, eval_assert, eval_bytes_conversion, eval_conversion, eval_count,
    eval_error_constructor, eval_exists, eval_output,
};
use crate::value::Value;

pub(crate) fn eval_call(
    call: &ExecExpr,
    _callee: &ExecExpr,
    args: &[ExecArg],
    target: &CheckedCallTarget,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match target {
        CheckedCallTarget::SavedIndexLookup => eval_checked_index_lookup(call, span, env).map(Some),
        CheckedCallTarget::SavedLayerRead => eval_saved_layer_read(call, span, env).map(Some),
        CheckedCallTarget::SavedResourceRead => eval_resource_read(call, span, env).map(Some),
        CheckedCallTarget::ErrorConstructor => eval_error_constructor(args, span, env).map(Some),
        CheckedCallTarget::ResourceConstructor(constructor) => {
            eval_resource_constructor(constructor, args, span, env).map(Some)
        }
        CheckedCallTarget::Builtin(target) => eval_builtin_call(*target, args, span, env),
        CheckedCallTarget::Std(target) => eval_std_call(*target, args, span, env),
        CheckedCallTarget::LocalCollection { name } => {
            eval_local_collection_read(name, args, span, env)?
                .map(Some)
                .ok_or_else(|| unsupported("a checked local collection lookup", span))
        }
        CheckedCallTarget::Function(target) => {
            let (module, function) = function_by_ref(env.program, *target, span)?;
            if args.iter().any(|arg| arg.mode.is_some()) {
                eval_call_with_modes(module, function, args, span, env)
            } else {
                eval_program_function(module, function, args, span, env)
            }
        }
    }
}

fn eval_program_function<'p>(
    module: &'p CheckedRuntimeModule,
    function: &'p CheckedRuntimeFunction,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'p>,
) -> Result<Option<Value>, RuntimeError> {
    let values = bind_arguments(&function.params, args, span, env)?;
    let (completion, _) = invoke_function(env, module, function, &values, &[])?;
    complete_call(completion, span)
}

/// Runs `function` as a child activation of `env`, moving the debugger hook into
/// the nested frame and back out on return. Callers differ only in how they bind
/// arguments and what they do with the writeback finals.
fn invoke_function<'p>(
    env: &mut Env<'p>,
    module: &'p CheckedRuntimeModule,
    function: &'p CheckedRuntimeFunction,
    values: &[Value],
    writeback: &[&'p str],
) -> Result<(Completion, Vec<Option<Value>>), RuntimeError> {
    let ctx = Context {
        program: env.program,
        store: env.store,
        host: env.host,
        transaction: Rc::clone(&env.transaction),
    };
    let names: Vec<&str> = function
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    let traversed_layers = env.traversed_layers.clone();
    let (completion, finals, hook) = invoke(Invocation {
        ctx,
        output: Rc::clone(&env.output),
        module: Some(module),
        param_names: &names,
        body: executable_body(function)?,
        span: function.span,
        args: values,
        writeback,
        traversed_layers: &traversed_layers,
        hook: env.hook.take(),
        depth: env.depth + 1,
    })?;
    env.hook = hook;
    Ok((completion, finals))
}

pub(crate) fn function_by_ref(
    program: &CheckedRuntimeProgram,
    target: CheckedFunctionRef,
    span: SourceSpan,
) -> Result<(&CheckedRuntimeModule, &CheckedRuntimeFunction), RuntimeError> {
    let module = program
        .modules()
        .get(target.module as usize)
        .ok_or_else(|| checked_target_error("function module", span))?;
    let function = module
        .functions()
        .get(target.function as usize)
        .ok_or_else(|| checked_target_error("function", span))?;
    Ok((module, function))
}

fn checked_target_error(target: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        throw: None,
        origin: None,
        code: RUN_UNKNOWN_FUNCTION,
        message: format!("checked call target no longer names a {target}"),
        span,
    }
}

fn eval_checked_index_lookup(
    call: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Some(place) = call.saved_place() else {
        return Err(unsupported("a checked saved index lookup", span));
    };
    eval_index_lookup(place, span, env)
}

fn eval_builtin_call(
    target: CheckedBuiltinCall,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match target {
        CheckedBuiltinCall::Print => eval_output(OutputKind::Print, args, span, env),
        CheckedBuiltinCall::Write => eval_output(OutputKind::Write, args, span, env),
        CheckedBuiltinCall::Exists => eval_exists(args, span, env).map(Some),
        CheckedBuiltinCall::NextId => eval_next_id(args, span, env).map(Some),
        CheckedBuiltinCall::Append => eval_append(args, span, env).map(Some),
        CheckedBuiltinCall::Bytes => eval_bytes_conversion(args, span, env).map(Some),
        CheckedBuiltinCall::ErrorCode => {
            eval_conversion(ConversionKind::ErrorCode, args, span, env).map(Some)
        }
        CheckedBuiltinCall::Conversion(scalar) => {
            eval_conversion(ConversionKind::Scalar(scalar), args, span, env).map(Some)
        }
        CheckedBuiltinCall::Keys => eval_keys(args, span, env).map(Some),
        CheckedBuiltinCall::Count => eval_count(args, span, env).map(Some),
        CheckedBuiltinCall::Values => eval_values(args, span, env).map(Some),
        CheckedBuiltinCall::Entries => eval_entries(args, span, env).map(Some),
        CheckedBuiltinCall::Reversed => eval_reversed(args, span, env).map(Some),
        CheckedBuiltinCall::Next => eval_neighbor(Direction::Ascending, args, span, env).map(Some),
        CheckedBuiltinCall::Prev => eval_neighbor(Direction::Descending, args, span, env).map(Some),
    }
}

fn eval_std_call(
    target: marrow_check::CheckedStdCall,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match target.capability {
        Capability::Clock => eval_clock_capability(target.op, args, span, env).map(Some),
        Capability::Env => eval_env(target.op, args, span, env).map(Some),
        Capability::Log => eval_log(target.op, args, span, env),
        Capability::Io => eval_io(target.op, args, span, env),
        Capability::Assert => eval_assert(target.op, args, span, env),
        Capability::Pure => eval_std(target.module, target.op, args, span, env).map(Some),
    }
}

pub(crate) fn eval_call_with_modes<'p>(
    module: &'p CheckedRuntimeModule,
    function: &'p CheckedRuntimeFunction,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'p>,
) -> Result<Option<Value>, RuntimeError> {
    let (values, places) = bind_arguments_with_modes(&function.params, args, span, env)?;
    let writeback: Vec<&str> = function
        .params
        .iter()
        .filter(|param| param.mode.is_some())
        .map(|param| param.name.as_str())
        .collect();
    let (completion, finals) = invoke_function(env, module, function, &values, &writeback)?;
    for (place, final_value) in places.into_iter().zip(finals) {
        if let (Some(place), Some(value)) = (place, final_value) {
            place.write(value, span, env)?;
        }
    }
    complete_call(completion, span)
}
