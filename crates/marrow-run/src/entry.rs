use std::cell::RefCell;
use std::rc::Rc;

use marrow_check::{
    CheckedEntryFunction, CheckedFunctionRef, CheckedLiteralKind, CheckedReadOnlyExpression,
    CheckedRuntimeFunction, CheckedRuntimeProgram, CheckedRuntimeValueType, StoredValueMeaning,
};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType, decode_value};
use marrow_syntax::{Expression, SourceSpan, parse_expression};

use crate::activation::{
    Completion, Invocation, bind_module_constants, check_argument_count, executable_body, invoke,
};
use crate::call::function_by_ref;
use crate::env::{Context, Env, TransactionState};
use crate::error::{
    RuntimeError, ambiguous_function, entry_argument, private_function,
    raise_with_transaction_escape, reraise_fault_with_transaction_escape, type_error,
    unknown_function, unsupported,
};
use crate::expr::eval_expr;
use crate::host::{Host, StepHook};
use crate::value::{
    RunOutput, RunOutputSink, Value, enum_value_from_member, value_scalar_type, value_to_key,
};

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

    pub fn from_text_args(
        program: &'p CheckedRuntimeProgram,
        entry: &str,
        args: &[(&str, &str)],
    ) -> Result<Self, RuntimeError> {
        let target = entry_target(program, entry)?;
        let (_, function) = function_by_ref(program, target, SourceSpan::default())?;
        let args = decode_entry_text_args(program, function, args)?;
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
    let output: Rc<RefCell<dyn RunOutputSink + 'p>> = match &host.output {
        Some(output) => Rc::clone(output),
        None => Rc::new(RefCell::new(ForwardOutput { sink: output })),
    };
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
        CheckedRuntimeValueType::Identity { root, .. } => match value {
            Value::Identity(identity) => {
                let store = program.facts().store_by_root(root)?;
                (identity.root() == root.as_str() && store.identity_keys_match(identity.keys()))
                    .then_some(Value::Identity(identity))
            }
            _ => None,
        },
        CheckedRuntimeValueType::Resource
        | CheckedRuntimeValueType::GroupEntry
        | CheckedRuntimeValueType::LocalTree { .. }
        | CheckedRuntimeValueType::Error
        | CheckedRuntimeValueType::Invalid
        | CheckedRuntimeValueType::Unknown => None,
    }
}

fn decode_entry_text_args(
    program: &CheckedRuntimeProgram,
    function: &CheckedRuntimeFunction,
    supplied: &[(&str, &str)],
) -> Result<Vec<Value>, RuntimeError> {
    let params = function.entry_params();
    let mut slots: Vec<Vec<&str>> = vec![Vec::new(); params.len()];
    for (name, value) in supplied {
        let Some(index) = params.iter().position(|param| param.name == *name) else {
            return Err(entry_argument(format!(
                "entry argument `{name}` does not name a parameter"
            )));
        };
        slots[index].push(*value);
    }
    params
        .iter()
        .zip(slots)
        .map(|(param, values)| decode_entry_param(program, &param.ty, &param.name, values))
        .collect()
}

fn decode_entry_param(
    program: &CheckedRuntimeProgram,
    ty: &CheckedRuntimeValueType,
    name: &str,
    values: Vec<&str>,
) -> Result<Value, RuntimeError> {
    if values.is_empty() {
        return Err(entry_argument(format!(
            "entry argument `{name}` is required"
        )));
    }
    match ty {
        CheckedRuntimeValueType::Sequence(element) => {
            decode_entry_sequence(program, element, name, values)
        }
        _ => {
            if values.len() != 1 {
                return Err(entry_argument(format!(
                    "entry argument `{name}` was supplied more than once"
                )));
            }
            decode_entry_value(program, ty, name, values[0])
        }
    }
}

fn decode_entry_sequence(
    program: &CheckedRuntimeProgram,
    element: &CheckedRuntimeValueType,
    name: &str,
    values: Vec<&str>,
) -> Result<Value, RuntimeError> {
    if !entry_sequence_element_supported(element) {
        return Err(entry_argument(format!(
            "entry argument `{name}` has an unsupported sequence element type"
        )));
    }
    if values == ["[]"] {
        return Ok(Value::Sequence(Vec::new()));
    }
    if values.contains(&"[]") {
        return Err(entry_argument(format!(
            "entry argument `{name}` uses [] only as the whole empty sequence"
        )));
    }
    values
        .into_iter()
        .map(|value| decode_entry_value(program, element, name, value))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Sequence)
}

fn entry_sequence_element_supported(ty: &CheckedRuntimeValueType) -> bool {
    matches!(
        ty,
        CheckedRuntimeValueType::Primitive(_) | CheckedRuntimeValueType::Enum { .. }
    )
}

fn decode_entry_value(
    program: &CheckedRuntimeProgram,
    ty: &CheckedRuntimeValueType,
    name: &str,
    text: &str,
) -> Result<Value, RuntimeError> {
    match ty {
        CheckedRuntimeValueType::Primitive(scalar) => decode_scalar(*scalar, text)
            .ok_or_else(|| entry_argument(format!("entry argument `{name}` is not a {scalar:?}"))),
        CheckedRuntimeValueType::Enum {
            enum_id,
            allowed_members,
            ..
        } => decode_enum_arg(program, *enum_id, allowed_members, name, text),
        CheckedRuntimeValueType::Identity { root, .. } => {
            decode_identity_arg(program, root, name, text)
        }
        CheckedRuntimeValueType::Sequence(_)
        | CheckedRuntimeValueType::Resource
        | CheckedRuntimeValueType::GroupEntry
        | CheckedRuntimeValueType::LocalTree { .. }
        | CheckedRuntimeValueType::Error
        | CheckedRuntimeValueType::Invalid
        | CheckedRuntimeValueType::Unknown => Err(entry_argument(format!(
            "entry argument `{name}` has a type outside the run entry argument surface"
        ))),
    }
}

fn decode_scalar(scalar: ScalarType, text: &str) -> Option<Value> {
    if scalar == ScalarType::Str {
        return Some(Value::Str(text.to_string()));
    }
    let (expression, diagnostics) = parse_expression(text);
    if !diagnostics.is_empty() {
        return None;
    }
    let value = eval_scalar_arg_expression(scalar, expression.as_ref()?)?;
    (value_scalar_type(&value) == Some(scalar)).then_some(value)
}

fn scalar_key(scalar: ScalarType, text: &str) -> Option<SavedKey> {
    value_to_key(decode_scalar(scalar, text)?, SourceSpan::default())
        .ok()
        .flatten()
}

fn eval_scalar_arg_expression(scalar: ScalarType, expression: &Expression) -> Option<Value> {
    match expression {
        Expression::Literal { kind, text, span } => {
            let value = crate::expr::eval_literal(lower_literal_kind(*kind), text, *span).ok()?;
            (value_scalar_type(&value) == Some(scalar)).then_some(value)
        }
        Expression::Call { callee, args, .. } => {
            let Expression::Name { segments, .. } = callee.as_ref() else {
                return None;
            };
            let [name] = segments.as_slice() else {
                return None;
            };
            let [arg] = args.as_slice() else {
                return None;
            };
            if arg.name.is_some() || *name != scalar.name() {
                return None;
            }
            let Value::Str(text) = eval_scalar_arg_expression(ScalarType::Str, &arg.value)? else {
                return None;
            };
            match (scalar, decode_value(text.as_bytes(), scalar)?) {
                (ScalarType::Date, SavedValue::Date(days)) => Some(Value::Date(days)),
                (ScalarType::Instant, SavedValue::Instant(nanos)) => Some(Value::Instant(nanos)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn lower_literal_kind(kind: marrow_syntax::LiteralKind) -> CheckedLiteralKind {
    match kind {
        marrow_syntax::LiteralKind::Integer => CheckedLiteralKind::Integer,
        marrow_syntax::LiteralKind::Decimal => CheckedLiteralKind::Decimal,
        marrow_syntax::LiteralKind::Duration => CheckedLiteralKind::Duration,
        marrow_syntax::LiteralKind::String => CheckedLiteralKind::String,
        marrow_syntax::LiteralKind::Bytes => CheckedLiteralKind::Bytes,
        marrow_syntax::LiteralKind::Bool => CheckedLiteralKind::Bool,
    }
}

fn decode_enum_arg(
    program: &CheckedRuntimeProgram,
    enum_id: Option<marrow_check::EnumId>,
    allowed_members: &[marrow_check::EnumMemberId],
    name: &str,
    text: &str,
) -> Result<Value, RuntimeError> {
    let Some(enum_id) = enum_id else {
        return Err(entry_argument(format!(
            "entry argument `{name}` has an unresolved enum type"
        )));
    };
    let matches: Vec<_> = allowed_members
        .iter()
        .copied()
        .filter(|member_id| {
            program
                .facts()
                .enum_member(*member_id)
                .is_some_and(|member| member.enum_id == enum_id)
                && program
                    .facts()
                    .enum_member_catalog_path(*member_id)
                    .is_some_and(|path| enum_spelling_matches(&path, text))
        })
        .collect();
    let [member_id] = matches.as_slice() else {
        return Err(entry_argument(format!(
            "entry argument `{name}` is not an accepted enum member"
        )));
    };
    enum_value_from_member(program.facts(), *member_id)
        .map(Value::Enum)
        .ok_or_else(|| entry_argument(format!("entry argument `{name}` is not selectable")))
}

fn enum_spelling_matches(path: &str, text: &str) -> bool {
    path == text
        || path
            .rsplit("::")
            .next()
            .is_some_and(|member| member == text)
        || path
            .strip_suffix(text)
            .is_some_and(|prefix| prefix.ends_with("::"))
}

fn decode_identity_arg(
    program: &CheckedRuntimeProgram,
    root: &str,
    name: &str,
    text: &str,
) -> Result<Value, RuntimeError> {
    let Some(store) = program.facts().store_by_root(root) else {
        return Err(entry_argument(format!(
            "entry argument `{name}` references unknown store `^{root}`"
        )));
    };
    let [key] = store.identity_keys.as_slice() else {
        return Err(entry_argument(format!(
            "entry argument `{name}` references composite identity `^{root}`; expose a wrapper entry with scalar key parameters"
        )));
    };
    let Some(StoredValueMeaning::Scalar(scalar)) = key.value_meaning else {
        return Err(entry_argument(format!(
            "entry argument `{name}` identity key is outside the scalar surface"
        )));
    };
    let Some(key) = scalar_key(scalar, text) else {
        return Err(entry_argument(format!(
            "entry argument `{name}` is not a valid key for `^{root}`"
        )));
    };
    Ok(crate::value::identity_value(root, vec![key]))
}
