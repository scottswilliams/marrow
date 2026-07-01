use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedIdentityConstructor, CheckedParam,
    CheckedResourceConstructor,
};
use marrow_schema::Type;
use marrow_store::Decimal;
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, type_error};
use crate::expr::{eval_expr, eval_into_slot};
use crate::path::{KeyRole, lower_keys};
use crate::statement::coerce_error_code_value;
use crate::value::identity_value;
use crate::value::{Sequence, Value};

pub(crate) fn bind_arguments(
    params: &[CheckedParam],
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut slots: Vec<Option<Value>> = vec![None; params.len()];
    let mut next_positional = 0;
    let mut seen_named = false;
    for arg in args {
        let index = arg_param_index(arg, params, &mut next_positional, &mut seen_named, span)?;
        // A `T?` parameter admits the empty optional: an absent maybe-present read or
        // call flows in as `Value::Absent`. An out-of-range index has no parameter
        // type; evaluate it plainly and let `place_argument` raise the arity fault.
        let value = eval_into_slot(&arg.value, params.get(index).map(|param| &param.ty), env)?;
        place_argument(&mut slots, index, value, params, span)?;
    }
    collect_arguments(slots, params, span)
}

fn arg_param_index(
    arg: &ExecArg,
    params: &[CheckedParam],
    next_positional: &mut usize,
    seen_named: &mut bool,
    span: SourceSpan,
) -> Result<usize, RuntimeError> {
    match &arg.name {
        None => {
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

fn place_argument(
    slots: &mut [Option<Value>],
    index: usize,
    value: Value,
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

fn collect_arguments(
    slots: Vec<Option<Value>>,
    params: &[CheckedParam],
    span: SourceSpan,
) -> Result<Vec<Value>, RuntimeError> {
    slots
        .into_iter()
        .zip(params)
        .map(|(slot, param)| {
            slot.ok_or_else(|| type_error(&format!("missing argument for `{}`", param.name), span))
        })
        .collect()
}

/// Build a resource value from a checked constructor call. The checker is the
/// sole owner of argument-shape and per-field type validation, so this path only
/// evaluates each supplied named argument and binds it to its field, preserving
/// constructor field order in the resulting value.
pub(crate) fn eval_resource_constructor(
    constructor: &CheckedResourceConstructor,
    args: &[ExecArg],
    _span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut slots: Vec<Option<Value>> = vec![None; constructor.fields.len()];

    for arg in args {
        let index = arg
            .name
            .as_ref()
            .and_then(|name| {
                constructor
                    .fields
                    .iter()
                    .position(|field| &field.name == name)
            })
            .expect("checked resource constructor binds each argument to a field");
        let value = eval_expr(&arg.value, env)?;
        slots[index] = Some(coerce_error_code_value(
            value,
            constructor.fields[index].error_code,
            arg.value.span(),
        )?);
    }

    Ok(Value::Resource(
        constructor
            .fields
            .iter()
            .zip(slots)
            .filter_map(|(field, value)| value.map(|value| (field.name.clone(), value)))
            .collect(),
    ))
}

/// Build an identity value from a checked `Id(^root, ...)` construct. The checker
/// owns argument shape (positional-only, a matching declared root, key arity), so
/// this path asserts that invariant structurally and lowers the key arguments.
pub(crate) fn eval_identity_constructor(
    constructor: &CheckedIdentityConstructor,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Some((root_arg, key_args)) = args.split_first() else {
        unreachable!("checked identity constructor supplies a saved root argument");
    };
    debug_assert!(
        args.iter().all(|arg| arg.name.is_none())
            && matches!(&root_arg.value, ExecExpr::SavedRoot { name, .. } if name == &constructor.root)
            && key_args.len() == constructor.keys.len(),
        "checked identity constructor matches its declared root and key arity",
    );
    let keys = lower_keys(
        key_args,
        span,
        KeyRole::IdentityKeys,
        None,
        &constructor.keys,
        env,
    )?;
    Ok(identity_value(&constructor.root, keys))
}

pub(crate) fn default_value(ty: &Type) -> Option<Value> {
    Some(match ty {
        Type::Sequence(_) => Value::Sequence(Sequence::default()),
        Type::Scalar(ScalarType::Int) => Value::Int(0),
        Type::Scalar(ScalarType::Bool) => Value::Bool(false),
        Type::Scalar(ScalarType::Str) => Value::Str(String::new()),
        Type::Scalar(ScalarType::Bytes) => Value::Bytes(Vec::new()),
        Type::Scalar(ScalarType::Date) => Value::Date(0),
        Type::Scalar(ScalarType::Instant) => Value::Instant(0),
        Type::Scalar(ScalarType::Duration) => Value::Duration(0),
        Type::Scalar(ScalarType::Decimal) => Value::Decimal(Decimal::parse("0")?),
        // An uninitialized optional binding is the empty optional.
        Type::Optional(_) => Value::Absent,
        Type::Identity(_) | Type::Named(_) | Type::Unknown => return None,
    })
}

#[cfg(test)]
mod default_value_tests {
    use marrow_schema::Type;
    use marrow_store::Decimal;
    use marrow_store::value::ScalarType;

    use crate::call_args::default_value;
    use crate::value::Value;

    #[test]
    fn var_default_matches_the_runtime_contract() {
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
            Some(Value::Sequence(crate::value::Sequence::default()))
        );
        assert_eq!(default_value(&Type::Identity("books".into())), None);
        assert_eq!(default_value(&Type::Named("Book".into())), None);
        assert_eq!(default_value(&Type::Unknown), None);
    }
}
