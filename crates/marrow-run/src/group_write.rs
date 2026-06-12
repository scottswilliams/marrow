//! Whole keyed-group-entry and keyed-leaf writes.

use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedPlace, StoreLeafKind,
};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::env::{Env, TraversedLayer};
use crate::error::{RuntimeError, unsupported};
use crate::expr::eval_expr;
use crate::path::{lower, lower_keys};
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, identity_keys_of, value_to_leaf};
use crate::write::{
    plan_layer_group_write, plan_layer_identity_leaf_write, plan_layer_leaf_write,
    validate_required_fields_after_group_write,
};
use crate::write_dispatch::{created_required_paths_for_value, resource_value_of};

pub(crate) fn eval_group_entry_write(
    target: &ExecExpr,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let ExecExpr::Call {
        callee, args: keys, ..
    } = target
    else {
        return Err(unsupported("assigning this saved path", span));
    };
    let ExecExpr::Field { base: record, .. } = callee.as_ref() else {
        return Err(unsupported("assigning this saved path", span));
    };
    let Some(place) = target.saved_place() else {
        return Err(unsupported("assigning this saved path", span));
    };
    let Some(layer_facts) = place.layers.last() else {
        return Err(unsupported("assigning this saved path", span));
    };
    let record_path = lower(record, env)?;
    let identity = record_path.identity.clone();
    let parent_addresses = record_path.layer_addresses.clone();
    let mut traversed_layers = parent_addresses.clone();
    traversed_layers.push(LayerAddress::from_checked(layer_facts, Vec::new()));
    let traversed = DataAddress::layer_prefix(place, &identity, &traversed_layers, span)?;
    env.guard_traversed_layer(&TraversedLayer::data(traversed), span)?;

    if let Some(leaf) = &layer_facts.leaf {
        return write_layer_leaf(
            place,
            &identity,
            &parent_addresses,
            keys,
            leaf.clone(),
            value,
            span,
            env,
        );
    }

    write_direct_group_entry(place, &identity, &parent_addresses, keys, value, span, env)
}

#[allow(clippy::too_many_arguments)]
fn write_layer_leaf(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    parent_addresses: &[LayerAddress],
    keys: &[ExecArg],
    leaf: StoreLeafKind,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let value = eval_expr(value, env)?;
    let expected = place
        .layers
        .last()
        .map_or(&[][..], |layer| layer.key_params.as_slice());
    let layer_keys = lower_keys(keys, span, false, None, expected, env)?;
    let mut layers = parent_addresses.to_vec();
    let Some(layer_facts) = place.layers.last() else {
        return Err(unsupported("assigning this saved path", span));
    };
    layers.push(LayerAddress::from_checked(layer_facts, layer_keys.clone()));

    let plan = match &leaf {
        StoreLeafKind::Identity { store_root, arity } => {
            let keys = identity_keys_of(value, store_root, span)?;
            plan_layer_identity_leaf_write(place, identity, &layers, &keys, *arity, span)
        }
        StoreLeafKind::Scalar(_) | StoreLeafKind::Enum { .. } => {
            let saved = value_to_leaf(value, &leaf, span)?;
            plan_layer_leaf_write(place, identity, &layers, &saved, span)
        }
    };
    env.apply_plan(plan, span)
}

fn write_direct_group_entry(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    parent_addresses: &[LayerAddress],
    keys: &[ExecArg],
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported(
            "assigning a non-resource value to a group entry",
            span,
        ));
    };
    let Some(layer_facts) = place.layers.last() else {
        return Err(unsupported("assigning this saved path", span));
    };
    let expected = layer_facts.key_params.as_slice();
    let layer_keys = lower_keys(keys, span, false, None, expected, env)?;
    let value = resource_value_of(&layer_facts.members, fields, span)?;
    let layer_address = LayerAddress::from_checked(layer_facts, layer_keys.clone());
    let mut layers = parent_addresses.to_vec();
    layers.push(layer_address);
    let created_required_paths = created_required_paths_for_value(
        place,
        identity,
        &layers,
        &layer_facts.members,
        &value,
        span,
        env,
    )?;
    let plan = plan_layer_group_write(place, identity, &layers, &value, span);
    let plan = if env.transaction_depth() == 0 {
        plan.and_then(|plan| {
            validate_required_fields_after_group_write(place, identity, &layers, env.store, span)?;
            Ok(plan)
        })
    } else {
        plan
    };
    env.apply_plan(plan, span)?;
    for path in created_required_paths {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(place, identity, &layers);
    Ok(())
}
