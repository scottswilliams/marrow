use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedLayer, CheckedSavedPlace,
};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::env::{AssignError, Env, TraversedLayer};
use crate::error::{RUN_TYPE, RuntimeError, assign_error, unsupported, write_fault};
use crate::expr::eval_expr;
use crate::index_maintenance::IndexWriteContext;
use crate::path::{direct_root_place, lower};
use crate::statement::coerce_error_code_value;
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, identity_value, value_to_leaf};
use crate::write::{next_after, next_id, next_layer_pos, plan_layer_leaf_write};

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
    let items = match env.lookup_mut(name) {
        Ok(Value::Sequence(items)) => items,
        Ok(_) => return Err(unsupported("appending to this path", span)),
        Err(error) => return Err(assign_error(name, error, *target_span)),
    };
    let highest = items.highest_position().unwrap_or(0);
    let pos = next_after(highest).map_err(|error| write_fault(error, span))?;
    items.append(pos, appended);
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
    let append_layer =
        append_layer(place, layer).ok_or_else(|| unsupported("appending to this layer", span))?;
    let leaf = append_layer
        .leaf
        .as_ref()
        .ok_or_else(|| unsupported("appending to this layer", span))?;
    let value = coerce_error_code_value(eval_expr(value, env)?, append_layer.error_code, span)?;
    let saved = value_to_leaf(value, leaf, span)?;
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
    let context = IndexWriteContext::new(place, &identity, env.store, env.program.facts(), span);
    let plan = plan_layer_leaf_write(context, &entry_layers, &saved);
    env.apply_plan(plan, span)?;
    Ok(Value::Int(pos))
}

fn append_layer<'a>(place: &'a CheckedSavedPlace, layer: &str) -> Option<&'a CheckedSavedLayer> {
    place
        .layers
        .iter()
        .rev()
        .find(|candidate| candidate.name == layer)
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
