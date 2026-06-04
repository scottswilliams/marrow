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
    NestedLayerTarget, WriteError, plan_layer_group_write, plan_layer_identity_leaf_write,
    plan_layer_leaf_write, plan_nested_layer_identity_leaf_write, plan_nested_layer_leaf_write,
};
use crate::write_dispatch::{created_required_paths_for_value, resource_value_of};
use crate::write_plan::WritePlan;

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
    let nested_parent = !record_path.layers.is_empty();
    let parent_addresses = record_path.layer_addresses.clone();
    let mut traversed_layers = parent_addresses.clone();
    traversed_layers.push(LayerAddress::from_checked(layer_facts, Vec::new()));
    let traversed = DataAddress::layer_prefix(place, &identity, &traversed_layers, span)?;
    env.guard_traversed_layer(&TraversedLayer::data(traversed), span)?;

    if let Some(leaf) = &layer_facts.leaf {
        return write_layer_leaf(
            LayerLeafWrite {
                place,
                identity: &identity,
                parent_addresses: &parent_addresses,
                keys,
                leaf: leaf.clone(),
                value,
                span,
            },
            env,
        );
    }

    if nested_parent {
        return Err(unsupported("assigning a nested group entry", span));
    }
    write_direct_group_entry(place, &identity, keys, value, span, env)
}

struct LayerLeafWrite<'a> {
    place: &'a CheckedSavedPlace,
    identity: &'a [SavedKey],
    parent_addresses: &'a [LayerAddress],
    keys: &'a [ExecArg],
    leaf: StoreLeafKind,
    value: &'a ExecExpr,
    span: SourceSpan,
}

fn write_layer_leaf(input: LayerLeafWrite<'_>, env: &mut Env<'_>) -> Result<(), RuntimeError> {
    let LayerLeafWrite {
        place,
        identity,
        parent_addresses,
        keys,
        leaf,
        value,
        span,
    } = input;
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

    let plan = layer_leaf_plan(LeafPlanInput {
        place,
        identity,
        layers: &layers,
        leaf,
        value,
        span,
    })?;
    env.apply_plan(plan, span)
}

struct LeafPlanInput<'a> {
    place: &'a CheckedSavedPlace,
    identity: &'a [SavedKey],
    layers: &'a [LayerAddress],
    leaf: StoreLeafKind,
    value: Value,
    span: SourceSpan,
}

fn layer_leaf_plan(
    input: LeafPlanInput<'_>,
) -> Result<Result<WritePlan, WriteError>, RuntimeError> {
    match input.leaf.clone() {
        StoreLeafKind::Identity { store_root, arity } => {
            identity_layer_leaf_plan(input, &store_root, arity)
        }
        StoreLeafKind::Scalar(_) | StoreLeafKind::Enum { .. } => scalar_layer_leaf_plan(input),
    }
}

fn identity_layer_leaf_plan(
    input: LeafPlanInput<'_>,
    store_root: &str,
    arity: usize,
) -> Result<Result<WritePlan, WriteError>, RuntimeError> {
    let identity_keys = identity_keys_of(input.value, store_root, input.span)?;
    if input.layers.len() == 1 {
        return Ok(plan_layer_identity_leaf_write(
            input.place,
            input.identity,
            input.layers,
            &identity_keys,
            arity,
            input.span,
        ));
    }
    Ok(plan_nested_layer_identity_leaf_write(
        input.place,
        input.identity,
        NestedLayerTarget {
            layers: input.layers,
        },
        &identity_keys,
        arity,
        input.span,
    ))
}

fn scalar_layer_leaf_plan(
    input: LeafPlanInput<'_>,
) -> Result<Result<WritePlan, WriteError>, RuntimeError> {
    let saved = value_to_leaf(input.value, &input.leaf, input.span)?;
    if input.layers.len() == 1 {
        return Ok(plan_layer_leaf_write(
            input.place,
            input.identity,
            input.layers,
            &saved,
            input.span,
        ));
    }
    Ok(plan_nested_layer_leaf_write(
        input.place,
        input.identity,
        NestedLayerTarget {
            layers: input.layers,
        },
        &saved,
        input.span,
    ))
}

fn write_direct_group_entry(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
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
    let created_required_paths = created_required_paths_for_value(
        place,
        identity,
        std::slice::from_ref(&layer_address),
        &layer_facts.members,
        &value,
        span,
        env,
    )?;
    let plan = plan_layer_group_write(
        place,
        identity,
        std::slice::from_ref(&layer_address),
        &value,
        span,
    );
    env.apply_plan(plan, span)?;
    for path in created_required_paths {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(place, identity, &[layer_address]);
    Ok(())
}
