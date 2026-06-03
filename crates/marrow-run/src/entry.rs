use std::cell::RefCell;
use std::rc::Rc;

use marrow_check::{
    CheckedEntryFunction, CheckedRuntimeFunction, CheckedRuntimeProgram, CheckedRuntimeValueType,
};
use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::activation::{Completion, Invocation, check_argument_count, executable_body, invoke};
use crate::call::function_by_ref;
use crate::env::{Context, TransactionState};
use crate::error::{
    RuntimeError, ambiguous_function, private_function, raise, reraise_fault, type_error,
    unknown_function,
};
use crate::host::{Host, StepHook};
use crate::value::{RunOutput, Value, enum_value_from_member};

#[derive(Debug, Clone)]
pub struct CheckedEntryCall {
    entry: String,
    args: Vec<Value>,
}

impl CheckedEntryCall {
    pub fn new(
        program: &CheckedRuntimeProgram,
        entry: &str,
        args: Vec<Value>,
    ) -> Result<Self, RuntimeError> {
        let target = entry_target(program, entry)?;
        let (_, function) = function_by_ref(program, target, SourceSpan::default())?;
        let args = canonicalize_entry_args(program, function, args)?;
        Ok(Self {
            entry: entry.to_string(),
            args,
        })
    }

    pub(crate) fn args(&self) -> &[Value] {
        &self.args
    }
}

pub fn run_entry(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    call: &CheckedEntryCall,
) -> Result<RunOutput, RuntimeError> {
    run_entry_with_host(program, store, &Host::new(), call)
}

pub fn run_entry_with_host(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    host: &Host,
    call: &CheckedEntryCall,
) -> Result<RunOutput, RuntimeError> {
    run_entry_impl(program, store, host, call, None)
}

pub fn run_entry_with_debugger(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    host: &Host,
    hook: &mut dyn StepHook,
    call: &CheckedEntryCall,
) -> Result<RunOutput, RuntimeError> {
    run_entry_impl(program, store, host, call, Some(hook))
}

fn run_entry_impl<'p>(
    program: &'p CheckedRuntimeProgram,
    store: &'p TreeStore,
    host: &'p Host,
    call: &CheckedEntryCall,
    hook: Option<&'p mut dyn StepHook>,
) -> Result<RunOutput, RuntimeError> {
    let target = entry_target(program, &call.entry)?;
    let (module, function) = function_by_ref(program, target, SourceSpan::default())?;
    let args = canonicalize_entry_args(program, function, call.args().to_vec())?;
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
    let value = match invoke(Invocation {
        ctx,
        output: Rc::clone(&output),
        module: Some(module),
        param_names: &names,
        body: executable_body(function)?,
        span: function.span,
        args: &args,
        writeback: &[],
        traversed_layers: &[],
        hook,
        depth: 1,
    })? {
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
        .map(|(param, value)| {
            reject_entry_mode(param.mode, &param.name)?;
            canonical_entry_value(program, &param.ty, value, &param.name)
        })
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

fn reject_entry_mode(
    mode: Option<marrow_check::CheckedParamMode>,
    name: &str,
) -> Result<(), RuntimeError> {
    if mode.is_none() {
        return Ok(());
    }
    Err(type_error(
        &format!("entry parameter `{name}` is out/inout and must be called from checked source"),
        SourceSpan::default(),
    ))
}

fn value_scalar_type(value: &Value) -> Option<marrow_schema::ScalarType> {
    match value {
        Value::Int(_) => Some(marrow_schema::ScalarType::Int),
        Value::Bool(_) => Some(marrow_schema::ScalarType::Bool),
        Value::Str(_) => Some(marrow_schema::ScalarType::Str),
        Value::Instant(_) => Some(marrow_schema::ScalarType::Instant),
        Value::Date(_) => Some(marrow_schema::ScalarType::Date),
        Value::Duration(_) => Some(marrow_schema::ScalarType::Duration),
        Value::Decimal(_) => Some(marrow_schema::ScalarType::Decimal),
        Value::Bytes(_) => Some(marrow_schema::ScalarType::Bytes),
        Value::Enum(_)
        | Value::Sequence(_)
        | Value::LocalTree(_)
        | Value::Resource(_)
        | Value::Identity(_) => None,
    }
}
