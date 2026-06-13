use std::cell::RefCell;
use std::rc::Rc;

use marrow_check::{CheckedBody as ExecBody, CheckedRuntimeFunction, CheckedRuntimeModule, FileId};
use marrow_syntax::SourceSpan;

use crate::env::{Context, Env, Flow, TraversedLayer};
use crate::error::{
    RUN_NO_ENCLOSING_LOOP, RUN_TYPE, RUN_UNCAUGHT_THROW, RuntimeError,
    raise_with_transaction_escape, reraise_fault_with_transaction_escape, unsupported,
};
use crate::exec::eval_block;
use crate::expr::eval_expr;
use crate::host::StepHook;
use crate::value::{RunOutputSink, Value};

pub(crate) enum Completion {
    Returned(Option<Value>),
    ReturnedAbsent,
    Threw {
        error: Value,
        span: SourceSpan,
        origin: Option<FileId>,
        transaction_escape: bool,
    },
    Faulted {
        code: &'static str,
        message: String,
        span: SourceSpan,
        origin: Option<FileId>,
        transaction_escape: bool,
    },
}

pub(crate) fn executable_body(
    function: &CheckedRuntimeFunction,
) -> Result<&ExecBody, RuntimeError> {
    function
        .body()
        .ok_or_else(|| unsupported("a function with no checked runtime body", function.span))
}

pub(crate) type Activation<'p> = (Completion, Option<&'p mut dyn StepHook>);

pub(crate) struct Invocation<'a, 'p> {
    pub(crate) ctx: Context<'p>,
    pub(crate) output: Rc<RefCell<dyn RunOutputSink + 'p>>,
    pub(crate) module: Option<&'p CheckedRuntimeModule>,
    pub(crate) param_names: &'a [&'p str],
    pub(crate) body: &'p ExecBody,
    pub(crate) span: SourceSpan,
    pub(crate) args: &'a [Value],
    pub(crate) traversed_layers: &'a [TraversedLayer],
    pub(crate) hook: Option<&'p mut dyn StepHook>,
    pub(crate) depth: usize,
}

pub(crate) fn invoke<'a, 'p>(input: Invocation<'a, 'p>) -> Result<Activation<'p>, RuntimeError> {
    let Invocation {
        ctx,
        output,
        module,
        param_names,
        body,
        span,
        args,
        traversed_layers,
        hook,
        depth,
    } = input;
    check_argument_count(param_names, args, span)?;
    let mut env = activation_env(ActivationEnv {
        ctx,
        output,
        module,
        hook,
        depth,
        traversed_layers,
    });
    bind_module_constants(module, &mut env)?;
    bind_activation_params(param_names, args, &mut env);
    let outcome = eval_block(body, &mut env);
    env.pop_scope();
    let completion = activation_completion(outcome, span, module, &env)?;
    Ok((completion, env.hook.take()))
}

pub(crate) fn check_argument_count(
    param_names: &[&str],
    args: &[Value],
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if args.len() == param_names.len() {
        return Ok(());
    }
    Err(RuntimeError::fault(
        RUN_TYPE,
        format!(
            "expected {} argument(s), got {}",
            param_names.len(),
            args.len()
        ),
        span,
    ))
}

struct ActivationEnv<'a, 'p> {
    ctx: Context<'p>,
    output: Rc<RefCell<dyn RunOutputSink + 'p>>,
    module: Option<&'p CheckedRuntimeModule>,
    hook: Option<&'p mut dyn StepHook>,
    depth: usize,
    traversed_layers: &'a [TraversedLayer],
}

fn activation_env<'a, 'p>(input: ActivationEnv<'a, 'p>) -> Env<'p> {
    let mut env = Env::new(
        input.ctx,
        input.output,
        input.module,
        input.hook,
        input.depth,
    );
    env.traversed_layers = input.traversed_layers.to_vec();
    env.push_scope();
    env
}

fn bind_module_constants(
    module: Option<&CheckedRuntimeModule>,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if let Some(module) = module {
        for constant in &module.constants {
            if let Some(value) = &constant.value {
                let value = eval_expr(value, env)?;
                env.bind(constant.name.clone(), value, false);
            }
        }
    }
    Ok(())
}

fn bind_activation_params(param_names: &[&str], args: &[Value], env: &mut Env<'_>) {
    for (name, arg) in param_names.iter().zip(args) {
        env.bind((*name).to_string(), arg.clone(), false);
    }
}

fn activation_completion(
    outcome: Result<Flow, RuntimeError>,
    span: SourceSpan,
    module: Option<&CheckedRuntimeModule>,
    env: &Env<'_>,
) -> Result<Completion, RuntimeError> {
    let here = activation_origin(module, env);
    Ok(match outcome {
        Ok(Flow::Return(value)) => Completion::Returned(value),
        Ok(Flow::ReturnAbsent) => Completion::ReturnedAbsent,
        Ok(Flow::Normal) => Completion::Returned(None),
        Ok(Flow::Throw {
            value,
            span,
            transaction_escape,
        }) => Completion::Threw {
            error: value,
            span,
            origin: here,
            transaction_escape,
        },
        Err(RuntimeError {
            throw: Some(error),
            code: RUN_UNCAUGHT_THROW,
            span,
            origin,
            transaction_escape,
            ..
        }) => Completion::Threw {
            error: *error,
            span,
            origin: origin.or(here),
            transaction_escape,
        },
        Err(error) if error.is_catchable() => {
            let transaction_escape = error.is_transaction_escape();
            Completion::Faulted {
                code: error.code,
                message: error.message,
                span: error.span,
                origin: error.origin.or(here),
                transaction_escape,
            }
        }
        Err(fatal) => return Err(fatal.with_origin_from(env.program, module)),
        Ok(Flow::Break) | Ok(Flow::Continue) => {
            return Err(RuntimeError::fault(
                RUN_NO_ENCLOSING_LOOP,
                "`break` or `continue` outside a loop".into(),
                span,
            )
            .with_origin_from(env.program, module));
        }
    })
}

fn activation_origin(module: Option<&CheckedRuntimeModule>, env: &Env<'_>) -> Option<FileId> {
    module.and_then(|module| env.program.file_id_of(module))
}

pub(crate) fn complete_call(completion: Completion) -> Result<Option<Value>, RuntimeError> {
    match completion {
        Completion::Returned(value) => Ok(value),
        Completion::ReturnedAbsent => Ok(None),
        Completion::Threw {
            error,
            span: throw_span,
            origin,
            transaction_escape,
        } => Err(raise_with_transaction_escape(
            error,
            throw_span,
            origin,
            transaction_escape,
        )),
        Completion::Faulted {
            code,
            message,
            span: fault_span,
            origin,
            transaction_escape,
        } => Err(reraise_fault_with_transaction_escape(
            code,
            message,
            fault_span,
            origin,
            transaction_escape,
        )),
    }
}
