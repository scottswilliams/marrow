use std::cell::RefCell;
use std::rc::Rc;

use marrow_check::{
    CheckedEntryFunction, CheckedFunctionRef, CheckedReadOnlyExpression, CheckedRuntimeFunction,
    CheckedRuntimeProgram, CheckedRuntimeValueType,
};
use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::activation::{
    Completion, Invocation, bind_module_constants, check_argument_count, executable_body, invoke,
};
use crate::call::function_by_ref;
use crate::env::{Context, Env, TransactionState};
use crate::error::{
    RuntimeError, ambiguous_function, private_function, raise_with_transaction_escape,
    reraise_fault_with_transaction_escape, type_error, unknown_function, unsupported,
};
use crate::expr::eval_expr;
use crate::host::{Host, StepHook};
use crate::value::{RunOutput, RunOutputSink, Value, enum_value_from_member, value_scalar_type};

struct ForwardOutput<'a> {
    sink: &'a mut dyn RunOutputSink,
}

impl RunOutputSink for ForwardOutput<'_> {
    fn write(&mut self, text: &str) {
        self.sink.write(text);
    }
}

#[derive(Debug, Clone)]
pub struct CheckedEntryCall<'p> {
    program: &'p CheckedRuntimeProgram,
    target: CheckedFunctionRef,
    args: Vec<Value>,
}

impl<'p> CheckedEntryCall<'p> {
    pub fn new(
        program: &'p CheckedRuntimeProgram,
        entry: &str,
        args: Vec<Value>,
    ) -> Result<Self, RuntimeError> {
        let target = entry_target(program, entry)?;
        let (_, function) = function_by_ref(program, target, SourceSpan::default())?;
        let args = canonicalize_entry_args(program, function, args)?;
        Ok(Self {
            program,
            target,
            args,
        })
    }
}

pub fn run_entry(
    store: &TreeStore,
    call: &CheckedEntryCall<'_>,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, RuntimeError> {
    run_entry_with_host(store, &Host::new(), call, output)
}

pub fn evaluate_checked_read_only_expression(
    store: &TreeStore,
    program: &CheckedRuntimeProgram,
    expression: &CheckedReadOnlyExpression,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, RuntimeError> {
    if expression.source_digest() != program.source_digest() {
        return Err(unsupported(
            "a checked read-only expression from a different checked program",
            SourceSpan::default(),
        ));
    }
    if expression.read_only_context_digest() != program.read_only_context_digest() {
        return Err(unsupported(
            "a checked read-only expression from a different checked program",
            SourceSpan::default(),
        ));
    }
    let module = program
        .modules()
        .get(expression.file_id().0 as usize)
        .ok_or_else(|| {
            unsupported(
                "a checked read-only expression whose source module is missing",
                SourceSpan::default(),
            )
        })?;
    if module.source_file != expression.source_file() {
        return Err(unsupported(
            "a checked read-only expression whose source file no longer matches",
            SourceSpan::default(),
        ));
    }

    let output: Rc<RefCell<dyn RunOutputSink + '_>> =
        Rc::new(RefCell::new(ForwardOutput { sink: output }));
    let host = Host::new();
    let ctx = Context {
        program,
        store,
        host: &host,
        transaction: Rc::new(RefCell::new(TransactionState::default())),
    };
    let mut env = Env::new(ctx, output, Some(module), None, 1);
    env.push_scope();
    let value = (|| {
        bind_module_constants(Some(module), &mut env)?;
        eval_expr(expression.expression(), &mut env)
    })();
    env.pop_scope();
    value
        .map(|value| RunOutput { value: Some(value) })
        .map_err(|error| error.with_origin_from(program, Some(module)))
}

pub fn run_entry_with_host(
    store: &TreeStore,
    host: &Host,
    call: &CheckedEntryCall<'_>,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, RuntimeError> {
    run_entry_impl(store, host, call, output, None)
}

pub fn run_entry_with_debugger(
    store: &TreeStore,
    host: &Host,
    hook: &mut dyn StepHook,
    call: &CheckedEntryCall<'_>,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, RuntimeError> {
    run_entry_impl(store, host, call, output, Some(hook))
}

fn run_entry_impl<'p>(
    store: &'p TreeStore,
    host: &'p Host,
    call: &'p CheckedEntryCall<'p>,
    output: &'p mut dyn RunOutputSink,
    hook: Option<&'p mut dyn StepHook>,
) -> Result<RunOutput, RuntimeError> {
    let program = call.program;
    let target = call.target;
    let (module, function) = function_by_ref(program, target, SourceSpan::default())?;
    let args = &call.args;
    let output: Rc<RefCell<dyn RunOutputSink + 'p>> =
        Rc::new(RefCell::new(ForwardOutput { sink: output }));
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
    let value = match invoke(Invocation {
        ctx,
        output: Rc::clone(&output),
        module: Some(module),
        param_names: &names,
        body: executable_body(function)?,
        span: function.span,
        args,
        traversed_layers: &[],
        hook,
        depth: 1,
    })? {
        (Completion::Returned(value), ..) => value,
        (Completion::ReturnedAbsent, ..) => None,
        (
            Completion::Threw {
                error,
                span,
                origin,
                transaction_escape,
            },
            ..,
        ) => {
            return Err(raise_with_transaction_escape(
                error,
                span,
                origin,
                transaction_escape,
            ));
        }
        (
            Completion::Faulted {
                code,
                message,
                span,
                origin,
                transaction_escape,
            },
            ..,
        ) => {
            return Err(reraise_fault_with_transaction_escape(
                code,
                message,
                span,
                origin,
                transaction_escape,
            ));
        }
    };
    Ok(RunOutput { value })
}

fn entry_target(
    program: &CheckedRuntimeProgram,
    entry: &str,
) -> Result<marrow_check::CheckedFunctionRef, RuntimeError> {
    match program.entry_function_ref(entry) {
        CheckedEntryFunction::Found(target) => Ok(target),
        CheckedEntryFunction::Ambiguous => Err(ambiguous_function(entry, SourceSpan::default())),
        CheckedEntryFunction::Private => Err(private_function(entry, SourceSpan::default())),
        CheckedEntryFunction::Missing => Err(unknown_function(entry, SourceSpan::default())),
    }
}

fn canonicalize_entry_args(
    program: &CheckedRuntimeProgram,
    function: &CheckedRuntimeFunction,
    args: Vec<Value>,
) -> Result<Vec<Value>, RuntimeError> {
    let names: Vec<&str> = function
        .entry_params()
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    check_argument_count(&names, &args, SourceSpan::default())?;
    function
        .entry_params()
        .iter()
        .zip(args)
        .map(|(param, value)| canonical_entry_value(program, &param.ty, value, &param.name))
        .collect()
}

fn canonical_entry_value(
    program: &CheckedRuntimeProgram,
    ty: &CheckedRuntimeValueType,
    value: Value,
    name: &str,
) -> Result<Value, RuntimeError> {
    if let Some(value) = canonical_entry_value_impl(program, ty, value) {
        return Ok(value);
    }
    Err(type_error(
        &format!("entry argument `{name}` has the wrong type"),
        SourceSpan::default(),
    ))
}

fn canonical_entry_value_impl(
    program: &CheckedRuntimeProgram,
    expected: &CheckedRuntimeValueType,
    value: Value,
) -> Option<Value> {
    match expected {
        CheckedRuntimeValueType::Primitive(scalar) => {
            (value_scalar_type(&value) == Some(*scalar)).then_some(value)
        }
        CheckedRuntimeValueType::Enum {
            enum_id,
            allowed_members,
            ..
        } => {
            let Some(enum_id) = enum_id else {
                return None;
            };
            let Value::Enum(value) = value else {
                return None;
            };
            if value.enum_id != *enum_id || !allowed_members.contains(&value.member_id) {
                return None;
            }
            enum_value_from_member(program.facts(), value.member_id).map(Value::Enum)
        }
        CheckedRuntimeValueType::Sequence(element) => match value {
            Value::Sequence(items) => items
                .into_iter()
                .map(|item| canonical_entry_value_impl(program, element, item))
                .collect::<Option<Vec<_>>>()
                .map(Value::Sequence),
            _ => None,
        },
        CheckedRuntimeValueType::Identity { .. }
        | CheckedRuntimeValueType::Resource
        | CheckedRuntimeValueType::GroupEntry
        | CheckedRuntimeValueType::LocalTree { .. }
        | CheckedRuntimeValueType::Error
        | CheckedRuntimeValueType::Invalid
        | CheckedRuntimeValueType::Unknown => None,
    }
}
