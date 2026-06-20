use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedPlace, StoreLeafKind,
};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::env::{AssignError, Env, TraversedLayer};
use crate::error::{RUN_TYPE, RuntimeError, assign_error, overflow, unsupported, write_fault};
use crate::expr::eval_expr;
use crate::path::{direct_root_place, lower};
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, identity_value, value_to_leaf};
use crate::write::{next_id, next_layer_pos, plan_layer_leaf_write};

pub(crate) fn eval_next_id(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            "`nextId` takes one argument".into(),
            span,
        ));
    };
    let Some(place) = direct_root_place(&arg.value) else {
        return Err(unsupported("`nextId` of this path", span));
    };
    let next = next_id(place, env.store, span);
    let next = next.map_err(|error| write_fault(error, span))?;
    Ok(identity_value(&place.root, vec![SavedKey::Int(next)]))
}

pub(crate) fn eval_append(
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [target, value] = args else {
        return Err(RuntimeError::fault(
            RUN_TYPE,
            "`append` takes a layer path and a value".into(),
            span,
        ));
    };
    if let Some(value) = eval_local_append(&target.value, &value.value, span, env)? {
        return Ok(value);
    }
    eval_saved_append(&target.value, &value.value, span, env)
}

fn eval_local_append(
    target: &ExecExpr,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let ExecExpr::Name {
        segments,
        span: target_span,
        ..
    } = target
    else {
        return Ok(None);
    };
    let [name] = segments.as_slice() else {
        return Ok(None);
    };
    if env.lookup(name).is_none() {
        return Err(assign_error(name, AssignError::Unbound, *target_span));
    }
    let appended = eval_expr(value, env)?;
    let Some(Value::Sequence(mut items)) = env.lookup(name).cloned() else {
        return Err(unsupported("appending to this path", span));
    };
    items.push(appended);
    let pos = i64::try_from(items.len()).map_err(|_| overflow(span))?;
    env.assign(name, Value::Sequence(items))
        .map_err(|error| assign_error(name, error, *target_span))?;
    Ok(Some(Value::Int(pos)))
}

fn eval_saved_append(
    target: &ExecExpr,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let ExecExpr::Field {
        base, name: layer, ..
    } = target
    else {
        return Err(unsupported("appending to this path", span));
    };
    let base_path = lower(base, env)?;
    let identity = base_path.identity.clone();
    let parent_addresses = base_path.layer_addresses.clone();
    let Some(place) = target.saved_place() else {
        return Err(unsupported("appending to this path", span));
    };
    let leaf =
        append_leaf(place, layer).ok_or_else(|| unsupported("appending to this layer", span))?;
    let saved = value_to_leaf(eval_expr(value, env)?, leaf, span)?;
    let mut prefix_layers = parent_addresses;
    let Some(layer_facts) = place.layers.last() else {
        return Err(unsupported("appending to this layer", span));
    };
    prefix_layers.push(LayerAddress::from_checked(layer_facts, Vec::new()));
    let traversed = DataAddress::layer_prefix(place, &identity, &prefix_layers, span)?;
    env.guard_traversed_layer(&TraversedLayer::data(traversed), span)?;
    let pos = next_append_position(place, &identity, &prefix_layers, span, env)?;
    let mut entry_layers = prefix_layers;
    let Some(entry_layer) = entry_layers.last_mut() else {
        return Err(unsupported("appending to this layer", span));
    };
    entry_layer.keys = vec![SavedKey::Int(pos)];
    let plan = plan_layer_leaf_write(place, &identity, &entry_layers, &saved, env.store, span);
    env.apply_plan(plan, span)?;
    Ok(Value::Int(pos))
}

fn append_leaf<'a>(place: &'a CheckedSavedPlace, layer: &str) -> Option<&'a StoreLeafKind> {
    place
        .layers
        .iter()
        .rev()
        .find(|candidate| candidate.name == layer)
        .and_then(|candidate| candidate.leaf.as_ref())
}

fn next_append_position(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    layers: &[LayerAddress],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<i64, RuntimeError> {
    next_layer_pos(place, identity, layers, env.store, span)
        .map_err(|error| write_fault(error, span))
}
