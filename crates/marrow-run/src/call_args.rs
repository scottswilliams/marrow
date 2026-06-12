use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedIdentityConstructor, CheckedParam,
    CheckedResourceConstructor, CheckedRuntimeValueType,
};
use marrow_schema::{KeyDef, Type};
use marrow_store::Decimal;
use marrow_store::key::SavedKey;
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RUN_TYPE, RuntimeError, type_error, unsupported};
use crate::expr::eval_expr;
use crate::path::lower_keys;
use crate::value::identity_value;
use crate::value::{Value, value_scalar_type};

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
        let value = eval_expr(&arg.value, env)?;
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

pub(crate) fn eval_resource_constructor(
    constructor: &CheckedResourceConstructor,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let mut slots: Vec<Option<Value>> = vec![None; constructor.fields.len()];

    for arg in args {
        let Some(name) = &arg.name else {
            return Err(type_error(
                &format!("`{}(...)` takes named fields", constructor.name),
                span,
            ));
        };
        let index = constructor
            .fields
            .iter()
            .position(|field| &field.name == name)
            .ok_or_else(|| {
                type_error(
                    &format!("`{}` has no field `{name}`", constructor.name),
                    span,
                )
            })?;
        if slots[index].is_some() {
            return Err(type_error(
                &format!("field `{name}` is supplied more than once"),
                span,
            ));
        }
        let value = eval_expr(&arg.value, env)?;
        if !checked_value_accepts(&constructor.fields[index].ty, &value) {
            return Err(type_error(
                &format!("field `{name}` has the wrong type"),
                span,
            ));
        }
        slots[index] = Some(value);
    }

    for (field, slot) in constructor.fields.iter().zip(&slots) {
        if slot.is_none() && field.required {
            return Err(type_error(
                &format!("`{}` requires `{}`", constructor.name, field.name),
                span,
            ));
        }
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

pub(crate) fn eval_identity_constructor(
    constructor: &CheckedIdentityConstructor,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(unsupported("`Id` with named arguments", span));
    }
    let Some((root_arg, key_args)) = args.split_first() else {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            "`Id` takes a saved root followed by its key argument(s)".into(),
            span,
        ));
    };
    match &root_arg.value {
        ExecExpr::SavedRoot { name, .. } if name == &constructor.root => {}
        _ => return Err(unsupported("`Id` with this root argument", span)),
    }
    if key_args.len() != constructor.keys.len() {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            format!(
                "`Id(^{})` expects {} key argument(s), but {} were given",
                constructor.root,
                constructor.keys.len(),
                key_args.len(),
            ),
            span,
        ));
    }
    let keys = lower_keys(key_args, span, false, None, &constructor.keys, env)?;
    Ok(identity_value(&constructor.root, keys))
}

pub(crate) fn checked_value_accepts(expected: &CheckedRuntimeValueType, value: &Value) -> bool {
    match expected {
        CheckedRuntimeValueType::Primitive(scalar) => value_scalar_type(value) == Some(*scalar),
        CheckedRuntimeValueType::Identity { root, keys } => {
            let Some(identity_keys) = keys.as_deref() else {
                return false;
            };
            match value {
                Value::Identity(identity) => {
                    identity.root() == root.as_str()
                        && identity_keys_match(identity_keys, identity.keys())
                }
                _ => false,
            }
        }
        CheckedRuntimeValueType::Resource | CheckedRuntimeValueType::GroupEntry => {
            matches!(value, Value::Resource(_))
        }
        CheckedRuntimeValueType::Enum {
            enum_id,
            allowed_members,
            ..
        } => {
            let Some(enum_id) = enum_id else {
                return false;
            };
            let Value::Enum(value) = value else {
                return false;
            };
            value.enum_id == *enum_id && allowed_members.contains(&value.member_id)
        }
        CheckedRuntimeValueType::Sequence(element) => match value {
            Value::Sequence(items) => items
                .iter()
                .all(|item| checked_value_accepts(element, item)),
            _ => false,
        },
        CheckedRuntimeValueType::LocalTree { value: element, .. } => match value {
            Value::LocalTree(entries) => entries
                .iter()
                .all(|entry| checked_value_accepts(element, &entry.value)),
            _ => false,
        },
        CheckedRuntimeValueType::Error => matches!(value, Value::Resource(_)),
        CheckedRuntimeValueType::Invalid | CheckedRuntimeValueType::Unknown => false,
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

#[cfg(test)]
mod default_value_tests {
    use marrow_check::CheckedRuntimeValueType;
    use marrow_schema::{KeyDef, Type};
    use marrow_store::Decimal;
    use marrow_store::key::SavedKey;
    use marrow_store::value::ScalarType;

    use crate::call_args::{checked_value_accepts, default_value};
    use crate::value::{Value, identity_value};

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
            Some(Value::Sequence(Vec::new()))
        );
        assert_eq!(default_value(&Type::Identity("books".into())), None);
        assert_eq!(default_value(&Type::Named("Book".into())), None);
        assert_eq!(default_value(&Type::Unknown), None);
    }

    #[test]
    fn composite_identity_value_requires_matching_store_root() {
        let expected = CheckedRuntimeValueType::Identity {
            root: "books".into(),
            keys: Some(vec![
                KeyDef {
                    name: "tenant".into(),
                    ty: Type::Scalar(ScalarType::Int),
                },
                KeyDef {
                    name: "book".into(),
                    ty: Type::Scalar(ScalarType::Int),
                },
            ]),
        };
        let other_root = Value::Identity(crate::value::IdentityValue::for_root(
            "authors",
            vec![SavedKey::Int(7), SavedKey::Int(11)],
        ));

        assert!(!checked_value_accepts(&expected, &other_root));
    }

    #[test]
    fn single_key_identity_requires_store_root_provenance() {
        let expected = CheckedRuntimeValueType::Identity {
            root: "books".into(),
            keys: Some(vec![KeyDef {
                name: "id".into(),
                ty: Type::Scalar(ScalarType::Int),
            }]),
        };

        assert!(!checked_value_accepts(&expected, &Value::Int(7)));
        assert!(checked_value_accepts(
            &expected,
            &identity_value("books", vec![SavedKey::Int(7)])
        ));
        assert!(!checked_value_accepts(
            &expected,
            &identity_value("authors", vec![SavedKey::Int(7)])
        ));
    }
}
