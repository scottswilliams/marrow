use marrow_check::{CheckedExpr as ExecExpr, CheckedSavedMember, StoreLeafKind};
use marrow_syntax::SourceSpan;

use crate::env::{Env, TraversedLayer};
use crate::error::{RuntimeError, unsupported};
use crate::expr::eval_expr;
use crate::path::{SavedPath, Terminal, lower};
use crate::value::{LeafValue, Value, value_to_leaf, value_to_saved};
use crate::write::{
    RequiredAbsentRemedy, RequiredEnforcement, ResourceValue, SuppliedIdentity, plan_resource_write,
};
use crate::write_dispatch::required::created_required_paths_for_value;

pub(crate) fn eval_resource_write(
    target: &ExecExpr,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let path = lower(target, env)?;
    if !path.layers.is_empty() || !matches!(path.terminal, Terminal::Record) {
        return Err(unsupported("this saved path", target.span()));
    }
    let value = eval_expr(value, env)?;
    write_resource(path, value, span, env)
}

pub(crate) fn eval_resource_write_value(
    target: &ExecExpr,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let path = lower(target, env)?;
    if !path.layers.is_empty() || !matches!(path.terminal, Terminal::Record) {
        return Err(unsupported("this saved path", target.span()));
    }
    write_resource(path, value, span, env)
}

pub(crate) fn write_resource(
    path: SavedPath,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Value::Resource(fields) = value else {
        return Err(unsupported(
            "assigning a non-resource value to a saved record",
            span,
        ));
    };
    let identity = path.identity.as_slice();
    env.guard_traversed_layer(&TraversedLayer::record(&path.place, span)?, span)?;
    let value = resource_value_of(&path.place.members, fields, span)?;
    let created_required_paths = created_required_paths_for_value(
        &path.place,
        identity,
        &[],
        &path.place.members,
        &value,
        span,
        env,
    )?;
    let plan = plan_resource_write(
        &path.place,
        identity,
        &value,
        env.store,
        env.program.facts(),
        span,
        RequiredEnforcement::for_transaction_depth(env.transaction_depth()),
    );
    env.apply_plan(plan, span)?;
    for path in created_required_paths {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(
        &path.place,
        identity,
        &[],
        span,
        RequiredAbsentRemedy::PopulateInValue,
    );
    Ok(())
}

pub(crate) fn resource_value_of(
    members: &[CheckedSavedMember],
    fields: Vec<(String, Value)>,
    span: SourceSpan,
) -> Result<ResourceValue, RuntimeError> {
    let mut value = ResourceValue::default();
    collect_resource_value(members, fields, &mut Vec::new(), span, &mut value)?;
    Ok(value)
}

fn collect_resource_value(
    members: &[CheckedSavedMember],
    fields: Vec<(String, Value)>,
    prefix: &mut Vec<String>,
    span: SourceSpan,
    out: &mut ResourceValue,
) -> Result<(), RuntimeError> {
    for (name, value) in fields {
        if let Some(group) = members
            .iter()
            .find(|member| member.name == name && member.is_unkeyed_group())
        {
            let Value::Resource(fields) = value else {
                return Err(unsupported(
                    "a non-resource value for an unkeyed group",
                    span,
                ));
            };
            prefix.push(name);
            collect_resource_value(&group.group_members, fields, prefix, span, out)?;
            prefix.pop();
            continue;
        }
        let field = flattened_field_name(prefix, &name);
        match value {
            Value::Identity(identity) => {
                let Some(StoreLeafKind::Identity { store_root, arity }) =
                    plain_field_leaf(members, &name)
                else {
                    return Err(unsupported("a nested resource field", span));
                };
                let keys = identity.into_keys_for_root(store_root, span)?;
                out.identities.push(SuppliedIdentity {
                    field,
                    keys,
                    referenced_arity: *arity,
                });
            }
            other => {
                let saved = match plain_field_leaf(members, &name) {
                    Some(leaf @ (StoreLeafKind::Scalar(_) | StoreLeafKind::Enum { .. })) => {
                        value_to_leaf(other, leaf, span)?
                    }
                    _ => value_to_saved(other)
                        .map(LeafValue::Scalar)
                        .ok_or_else(|| unsupported("a nested resource field", span))?,
                };
                out.fields.push((field, saved));
            }
        }
    }
    Ok(())
}

fn flattened_field_name(prefix: &[String], name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}.{name}", prefix.join("."))
    }
}

fn plain_field_leaf<'a>(
    members: &'a [CheckedSavedMember],
    field: &str,
) -> Option<&'a StoreLeafKind> {
    members
        .iter()
        .find(|member| member.name == field && member.is_plain_field())
        .and_then(|member| member.leaf.as_ref())
}
