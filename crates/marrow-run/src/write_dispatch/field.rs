use marrow_check::{CheckedExpr as ExecExpr, StoreLeafKind};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{RuntimeError, unsupported};
use crate::expr::eval_expr;
use crate::path::{SavedPath, Terminal, lower};
use crate::store::{DataAddress, LayerAddress};
use crate::value::{LeafValue, Value, identity_keys_of, value_to_leaf};
use crate::write::{
    WriteError, plan_field_write, plan_identity_field_write, plan_nested_field_write,
    plan_nested_identity_field_write, validate_required_fields_after_field_write,
};
use crate::write_dispatch::required::created_required_field_path;
use crate::write_plan::WritePlan;

pub(crate) fn eval_saved_field_write(
    target: &ExecExpr,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let path = lower(target, env)?;
    if !matches!(path.terminal, crate::path::Terminal::Field { .. }) {
        return Err(unsupported("writing this saved path", span));
    }
    let value = eval_expr(value, env)?;
    path.write(value, span, env)
}

pub(crate) fn write_saved_field(
    path: SavedPath,
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let identity = path.identity.as_slice();
    let created_required_path =
        created_required_field_path(&path.place, identity, &[], &path.members, field, span, env)?;
    if let Terminal::Field {
        leaf: Some(StoreLeafKind::Identity { store_root, arity }),
        ..
    } = &path.terminal
    {
        write_identity_saved_field(&path, field, value, store_root, *arity, span, env)?;
    } else {
        write_scalar_saved_field(&path, field, value, span, env)?;
    }
    finish_saved_field_write(&path, created_required_path, env);
    Ok(())
}

fn write_identity_saved_field(
    path: &SavedPath,
    field: &str,
    value: Value,
    store_root: &str,
    arity: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let keys = identity_keys_of(value, store_root, span)?;
    let plan = plan_identity_field_write(
        &path.place,
        &path.identity,
        field,
        &keys,
        arity,
        env.store,
        span,
    );
    let plan = validate_field_plan(path, &[], field, plan, env);
    env.apply_plan(plan, span)
}

fn write_scalar_saved_field(
    path: &SavedPath,
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let saved = field_leaf_value(path, value, span)?;
    let plan = plan_field_write(&path.place, &path.identity, field, &saved, env.store, span);
    let plan = validate_field_plan(path, &[], field, plan, env);
    env.apply_plan(plan, span)
}

/// Outside a transaction, chain the planned field write with the immediate
/// required-fields check so a single-field write that leaves a record incomplete
/// is rejected before it lands. Inside a transaction the check is deferred to
/// commit, so the plan passes through unchanged.
fn validate_field_plan(
    path: &SavedPath,
    layers: &[LayerAddress],
    field: &str,
    plan: Result<WritePlan, WriteError>,
    env: &Env<'_>,
) -> Result<WritePlan, WriteError> {
    if env.transaction_depth() != 0 {
        return plan;
    }
    plan.and_then(|plan| {
        validate_required_fields_after_field_write(
            &path.place,
            &path.identity,
            layers,
            field,
            env.store,
            path.place.span,
        )?;
        Ok(plan)
    })
}

fn finish_saved_field_write(
    path: &SavedPath,
    created_required_path: Option<DataAddress>,
    env: &mut Env<'_>,
) {
    if let Some(path) = created_required_path {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(&path.place, &path.identity, &[]);
}

pub(crate) fn write_nested_field(
    path: SavedPath,
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let identity = path.identity.as_slice();
    let created_required_path = created_required_field_path(
        &path.place,
        identity,
        &path.layer_addresses,
        &path.members,
        field,
        span,
        env,
    )?;
    let plan = if let Terminal::Field {
        leaf: Some(StoreLeafKind::Identity { store_root, arity }),
        ..
    } = &path.terminal
    {
        let keys = identity_keys_of(value, store_root, span)?;
        plan_nested_identity_field_write(
            &path.place,
            identity,
            &path.layer_addresses,
            field,
            &keys,
            *arity,
            span,
        )
    } else {
        let saved = field_leaf_value(&path, value, span)?;
        plan_nested_field_write(
            &path.place,
            identity,
            &path.layer_addresses,
            field,
            &saved,
            span,
        )
    };
    let plan = validate_field_plan(&path, &path.layer_addresses, field, plan, env);
    finish_nested_field_write(&path, identity, created_required_path, plan, span, env)
}

fn field_leaf_value(
    path: &SavedPath,
    value: Value,
    span: SourceSpan,
) -> Result<LeafValue, RuntimeError> {
    match saved_field_leaf(path) {
        Some(leaf) => value_to_leaf(value, leaf, span),
        None => crate::value::value_to_saved(value)
            .map(LeafValue::Scalar)
            .ok_or_else(|| unsupported("writing a resource value to a field", span)),
    }
}

fn saved_field_leaf(path: &SavedPath) -> Option<&StoreLeafKind> {
    match &path.terminal {
        Terminal::Field {
            leaf: Some(leaf), ..
        } => Some(leaf),
        _ => None,
    }
}

fn finish_nested_field_write(
    path: &SavedPath,
    identity: &[SavedKey],
    created_required_path: Option<DataAddress>,
    plan: Result<WritePlan, WriteError>,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    env.apply_plan(plan, span)?;
    if let Some(created) = created_required_path {
        env.note_created_required_path(created);
    }
    env.defer_required_entry_check(&path.place, identity, &path.layer_addresses);
    Ok(())
}
