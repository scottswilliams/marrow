use marrow_check::{CheckedExpr as ExecExpr, CheckedSavedMember};
use marrow_syntax::SourceSpan;

use crate::env::{Env, TraversedLayer};
use crate::error::{RuntimeError, raise_fault, unsupported};
use crate::path::{SavedPath, Terminal, direct_root_place, lower, lower_keys};
use crate::store::{DataAddress, LayerAddress};
use crate::write::{
    WRITE_REQUIRED_FIELD, WRITE_REQUIRES_MAINTENANCE, plan_data_delete, plan_field_delete,
    plan_resource_delete, plan_store_root_delete,
};
use crate::write_dispatch::required::{
    checked_field_required, checked_group_has_required_materialized_field, checked_member_exists,
    checked_unkeyed_group, required_delete_has_preexisting_data, required_paths_under_group,
};

pub(crate) fn eval_delete(
    path: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if let ExecExpr::Field { base, name, .. } = path
        && base.saved_place().is_some()
    {
        return eval_field_delete(base, name, span, env);
    }
    if let ExecExpr::Call { callee, .. } = path
        && let ExecExpr::Field { base, .. } = callee.as_ref()
        && base.saved_place().is_some()
    {
        return eval_layer_entry_delete(path, span, env);
    }
    if let Some(place) = direct_root_place(path).filter(|place| !place.identity_keys.is_empty()) {
        return eval_whole_root_delete(place, span, env);
    }
    let path = lower(path, env)?;
    if !path.layers.is_empty() || !matches!(path.terminal, Terminal::Record) {
        return Err(unsupported("this saved path", span));
    }
    env.guard_traversed_layer(&TraversedLayer::record(&path.place, span)?, span)?;
    let plan = plan_resource_delete(&path.place, &path.identity, env.store, span);
    env.apply_plan(plan, span)?;
    Ok(())
}

/// Drop a whole keyed root and its generated index branches.
pub(crate) fn eval_whole_root_delete(
    place: &marrow_check::CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let name = place.root.as_str();
    env.require_maintenance(
        WRITE_REQUIRES_MAINTENANCE,
        format!(
            "dropping the whole managed root `^{name}` is maintenance work; \
             run in maintenance mode to drop the root"
        ),
        span,
    )?;
    env.apply_plan(plan_store_root_delete(place, span), span)
}

pub(crate) fn eval_field_delete(
    base: &ExecExpr,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if nested_group_delete_base(base) {
        let base_path = lower(base, env)?;
        return delete_nested_field(base_path, field, span, env);
    }
    let base_path = top_level_delete_base(base, span, env)?;
    match checked_unkeyed_group(&base_path.members, field) {
        Some(group) => delete_top_level_group(&base_path, field, group, span, env),
        None => delete_top_level_field(&base_path, field, span, env),
    }
}

fn nested_group_delete_base(base: &ExecExpr) -> bool {
    match base {
        ExecExpr::Call { callee, .. } => matches!(callee.as_ref(), ExecExpr::Field { .. }),
        ExecExpr::Field { base, .. } => base.saved_place().is_some(),
        _ => false,
    }
}

fn top_level_delete_base(
    base: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<SavedPath, RuntimeError> {
    let base_path = lower(base, env)?;
    if base_path.layers.is_empty() && matches!(base_path.terminal, Terminal::Record) {
        Ok(base_path)
    } else {
        Err(unsupported("deleting from this saved root", span))
    }
}

fn delete_top_level_group(
    base_path: &SavedPath,
    field: &str,
    group: &CheckedSavedMember,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let identity = base_path.identity.as_slice();
    let deletes_required = checked_group_has_required_materialized_field(group);
    if !env.host.maintenance && deletes_required {
        return Err(raise_fault(
            WRITE_REQUIRED_FIELD,
            format!(
                "cannot delete unkeyed group `{field}` because it contains a required \
                 field; delete the whole record instead, or run in maintenance mode"
            ),
            span,
        ));
    }
    let required_paths =
        required_paths_under_group(&base_path.place, identity, &[], field, group, span)?;
    let had_required_data = deletes_required
        && env.host.maintenance
        && required_delete_has_preexisting_data(&required_paths, span, env)?;
    let address =
        DataAddress::member_path(&base_path.place, identity, &[], &[field.to_string()], span)?;
    env.apply_plan(plan_data_delete(address), span)?;
    if had_required_data {
        env.note_maintenance_required_delete(&base_path.place, identity, &[]);
    }
    Ok(())
}

fn delete_top_level_field(
    base_path: &SavedPath,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let identity = base_path.identity.as_slice();
    let deletes_required = checked_field_required(&base_path.members, field).unwrap_or(false);
    if !env.host.maintenance && deletes_required {
        return Err(raise_fault(
            WRITE_REQUIRED_FIELD,
            format!(
                "cannot delete required field `{field}`; delete the whole record \
                 instead, or run in maintenance mode"
            ),
            span,
        ));
    }
    let required_address =
        DataAddress::member_path(&base_path.place, identity, &[], &[field.to_string()], span)?;
    let had_required_data = deletes_required
        && env.host.maintenance
        && required_delete_has_preexisting_data(
            std::slice::from_ref(&required_address),
            span,
            env,
        )?;
    let plan = plan_field_delete(&base_path.place, identity, field, env.store, span);
    env.apply_plan(plan, span)?;
    if had_required_data {
        env.note_maintenance_required_delete(&base_path.place, identity, &[]);
    }
    Ok(())
}

pub(crate) fn delete_nested_field(
    path: SavedPath,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if let Some(group) = checked_unkeyed_group(&path.members, field) {
        return delete_nested_unkeyed_group(&path, field, group, span, env);
    }
    delete_nested_scalar_field(&path, field, span, env)
}

fn delete_nested_unkeyed_group(
    path: &SavedPath,
    field: &str,
    group: &CheckedSavedMember,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let deletes_required = checked_group_has_required_materialized_field(group);
    if !env.host.maintenance && deletes_required {
        return Err(raise_fault(
            WRITE_REQUIRED_FIELD,
            format!(
                "cannot delete unkeyed group `{field}` because it contains a required \
                 field; delete the whole record instead, or run in maintenance mode"
            ),
            span,
        ));
    }
    let identity = path.identity.as_slice();
    let required_paths = required_paths_under_group(
        &path.place,
        identity,
        &path.layer_addresses,
        field,
        group,
        span,
    )?;
    let had_required_data = deletes_required
        && env.host.maintenance
        && required_delete_has_preexisting_data(&required_paths, span, env)?;
    let address = DataAddress::member_path(
        &path.place,
        identity,
        &path.layer_addresses,
        &[field.to_string()],
        span,
    )?;
    env.apply_plan(plan_data_delete(address), span)?;
    note_nested_required_delete(path, had_required_data, env);
    Ok(())
}

fn delete_nested_scalar_field(
    path: &SavedPath,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    if !checked_member_exists(&path.members, field) {
        return Err(unsupported("deleting this group field", span));
    }
    let deletes_required = checked_field_required(&path.members, field).unwrap_or(false);
    if !env.host.maintenance && deletes_required {
        return Err(raise_fault(
            WRITE_REQUIRED_FIELD,
            format!(
                "cannot delete required field `{field}`; delete the whole record \
                 instead, or run in maintenance mode"
            ),
            span,
        ));
    }
    let identity = path.identity.as_slice();
    let required_address = DataAddress::member_path(
        &path.place,
        identity,
        &path.layer_addresses,
        &[field.to_string()],
        span,
    )?;
    let had_required_data = deletes_required
        && env.host.maintenance
        && required_delete_has_preexisting_data(
            std::slice::from_ref(&required_address),
            span,
            env,
        )?;
    let address = DataAddress::member_path(
        &path.place,
        identity,
        &path.layer_addresses,
        &[field.to_string()],
        span,
    )?;
    env.apply_plan(plan_data_delete(address), span)?;
    note_nested_required_delete(path, had_required_data, env);
    Ok(())
}

fn note_nested_required_delete(path: &SavedPath, had_required_data: bool, env: &mut Env<'_>) {
    if had_required_data {
        env.note_maintenance_required_delete(&path.place, &path.identity, &path.layer_addresses);
    }
}

pub(crate) fn eval_layer_entry_delete(
    target: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let ExecExpr::Call {
        callee, args: keys, ..
    } = target
    else {
        return Err(unsupported("deleting this layer entry", span));
    };
    let ExecExpr::Field { base: record, .. } = callee.as_ref() else {
        return Err(unsupported("deleting this layer entry", span));
    };
    let Some(layer_facts) = target.saved_place().and_then(|place| place.layers.last()) else {
        return Err(unsupported("deleting this layer entry", span));
    };
    let record_path = lower(record, env)?;
    let identity = record_path.identity.clone();
    let expected = layer_facts.key_params.as_slice();
    let entry_keys = lower_keys(keys, span, false, None, expected, env)?;
    let mut layer_addresses = record_path.layer_addresses;
    layer_addresses.push(LayerAddress::from_checked(layer_facts, Vec::new()));
    let traversed = DataAddress::layer_prefix(
        target.saved_place().expect("checked above"),
        &identity,
        &layer_addresses,
        span,
    )?;
    env.guard_traversed_layer(&TraversedLayer::data(traversed), span)?;
    layer_addresses.last_mut().expect("terminal layer").keys = entry_keys;
    let address = DataAddress::layer_prefix(
        target.saved_place().expect("checked above"),
        &identity,
        &layer_addresses,
        span,
    )?;
    env.apply_plan(plan_data_delete(address), span)?;
    Ok(())
}
