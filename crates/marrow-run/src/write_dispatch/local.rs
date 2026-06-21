use marrow_check::CheckedExpr as ExecExpr;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, assign_error, unsupported};
use crate::expr::eval_expr;
use crate::statement::coerce_error_code_value;
use crate::value::Value;

pub(crate) fn eval_local_field_set(
    base: &ExecExpr,
    field: &str,
    value: &ExecExpr,
    coerce_error_code: bool,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Some((name, groups)) = local_field_target(base) else {
        return Err(unsupported("setting a field of this value", span));
    };
    let new_value = coerce_error_code_value(eval_expr(value, env)?, coerce_error_code, span)?;
    write_local_field(&name, &groups, field, new_value, span, env)
}

/// The local variable and the chain of unkeyed groups a field-write base descends,
/// e.g. `("p", ["name"])` for the base `p.name` of `p.name.first = v`. A keyed-layer
/// lookup or any non-name root in the path is not a local field place and yields
/// `None`.
fn local_field_target(base: &ExecExpr) -> Option<(String, Vec<String>)> {
    match base {
        ExecExpr::Name { segments, .. } => match segments.as_slice() {
            [name] => Some((name.clone(), Vec::new())),
            _ => None,
        },
        ExecExpr::Field {
            base: inner, name, ..
        } => {
            let (root, mut groups) = local_field_target(inner)?;
            groups.push(name.clone());
            Some((root, groups))
        }
        _ => None,
    }
}

/// Update (or insert) `field` of the local resource bound to `base`, descending
/// `groups` as unkeyed nested groups and materializing any absent group as an empty
/// resource value along the way, then rebinding the variable.
pub(crate) fn write_local_field(
    base: &str,
    groups: &[String],
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Some(Value::Resource(mut fields)) = env.lookup(base).cloned() else {
        return Err(unsupported("setting a field of a non-resource local", span));
    };
    set_group_field(&mut fields, groups, field, value, span)?;
    env.assign(base, Value::Resource(fields))
        .map_err(|error| assign_error(base, error, span))
}

/// Write `field` into the resource `fields`, descending `groups` first. Each group
/// step navigates into (or creates) a nested resource value, so a nested group can be
/// populated field by field even when no whole-group value was assigned.
fn set_group_field(
    fields: &mut Vec<(String, Value)>,
    groups: &[String],
    field: &str,
    value: Value,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    let Some((group, rest)) = groups.split_first() else {
        set_field(fields, field, value);
        return Ok(());
    };
    let nested = match fields.iter().position(|(name, _)| name == group) {
        Some(index) => match &mut fields[index].1 {
            Value::Resource(nested) => nested,
            _ => return Err(unsupported("setting a field of a non-resource group", span)),
        },
        None => {
            fields.push((group.clone(), Value::Resource(Vec::new())));
            let Value::Resource(nested) = &mut fields.last_mut().expect("just pushed").1 else {
                unreachable!("just inserted a resource group")
            };
            nested
        }
    };
    set_group_field(nested, rest, field, value, span)
}

fn set_field(fields: &mut Vec<(String, Value)>, field: &str, value: Value) {
    match fields.iter().position(|(existing, _)| existing == field) {
        Some(index) => fields[index].1 = value,
        None => fields.push((field.to_string(), value)),
    }
}
