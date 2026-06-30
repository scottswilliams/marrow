//! Whole keyed-group-entry and keyed-leaf writes.

use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedLayer, CheckedSavedPlace,
    StoreLeafKind,
};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::env::{Env, TraversedLayer};
use crate::error::{RuntimeError, unsupported};
use crate::expr::eval_expr;
use crate::index_maintenance::IndexWriteContext;
use crate::path::{KeyRole, SavedPath, lower, lower_keys};
use crate::statement::coerce_error_code_value;
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, value_to_leaf};
use crate::write::{
    plan_layer_group_write, plan_layer_leaf_write, validate_required_fields_after_group_write,
};
use crate::write_dispatch::{created_required_paths_for_value, resource_value_of};

pub(crate) fn eval_group_entry_write(
    target: &ExecExpr,
    value: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (owned, keys) = resolve_group_entry_target(target, span, env)?;
    let target = owned.as_target();
    if let Some(leaf) = &target.layer_facts.leaf {
        let value = eval_expr(value, env)?;
        return write_layer_leaf(&target, keys, leaf, value, env);
    }

    let value = eval_expr(value, env)?;
    write_direct_group_entry(&target, keys, value, env)
}

pub(crate) fn eval_group_entry_write_value(
    target: &ExecExpr,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (owned, keys) = resolve_group_entry_target(target, span, env)?;
    let target = owned.as_target();
    if let Some(leaf) = &target.layer_facts.leaf {
        return write_layer_leaf(&target, keys, leaf, value, env);
    }

    write_direct_group_entry(&target, keys, value, env)
}

pub(crate) fn write_group_path(
    path: SavedPath,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Some(layer_facts) = path.place.layers.last() else {
        return Err(unsupported("assigning this saved path", span));
    };
    let Some((layer_address, parent_addresses)) = path.layer_addresses.split_last() else {
        return Err(unsupported("assigning this saved path", span));
    };
    let mut traversed_layers = parent_addresses.to_vec();
    traversed_layers.push(LayerAddress::from_checked(layer_facts, Vec::new()));
    let traversed =
        DataAddress::layer_prefix(&path.place, &path.identity, &traversed_layers, span)?;
    env.guard_traversed_layer(&TraversedLayer::data(traversed), span)?;

    let target = GroupEntryTarget {
        place: &path.place,
        identity: &path.identity,
        parent_addresses,
        layer_facts,
        span,
    };
    if let Some(leaf) = &target.layer_facts.leaf {
        write_layer_leaf_at(&target, layer_address.keys.clone(), leaf, value, env)
    } else {
        write_direct_group_entry_at(&target, layer_address.keys.clone(), value, env)
    }
}

fn resolve_group_entry_target<'a>(
    target: &'a ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(OwnedGroupEntryTarget, &'a [ExecArg]), RuntimeError> {
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

    Ok((
        OwnedGroupEntryTarget {
            place: place.clone(),
            identity,
            parent_addresses,
            layer_facts: layer_facts.clone(),
            span,
        },
        keys,
    ))
}

struct OwnedGroupEntryTarget {
    place: CheckedSavedPlace,
    identity: Vec<SavedKey>,
    parent_addresses: Vec<LayerAddress>,
    layer_facts: CheckedSavedLayer,
    span: SourceSpan,
}

impl OwnedGroupEntryTarget {
    fn as_target(&self) -> GroupEntryTarget<'_> {
        GroupEntryTarget {
            place: &self.place,
            identity: &self.identity,
            parent_addresses: &self.parent_addresses,
            layer_facts: &self.layer_facts,
            span: self.span,
        }
    }
}

struct GroupEntryTarget<'a> {
    place: &'a CheckedSavedPlace,
    identity: &'a [SavedKey],
    parent_addresses: &'a [LayerAddress],
    layer_facts: &'a CheckedSavedLayer,
    span: SourceSpan,
}

fn write_layer_leaf(
    target: &GroupEntryTarget<'_>,
    keys: &[ExecArg],
    leaf: &StoreLeafKind,
    value: Value,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let expected = target.layer_facts.key_params.as_slice();
    let layer_keys = lower_keys(keys, target.span, KeyRole::Layer, None, expected, env)?;
    write_layer_leaf_at(target, layer_keys, leaf, value, env)
}

fn write_layer_leaf_at(
    target: &GroupEntryTarget<'_>,
    layer_keys: Vec<SavedKey>,
    leaf: &StoreLeafKind,
    value: Value,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let value = coerce_error_code_value(value, target.layer_facts.error_code, target.span)?;
    let mut layers = target.parent_addresses.to_vec();
    layers.push(LayerAddress::from_checked(target.layer_facts, layer_keys));

    let context = IndexWriteContext::new(
        target.place,
        target.identity,
        env.store,
        env.program.facts(),
        target.span,
    );
    let saved = value_to_leaf(value, leaf, target.span)?;
    let plan = plan_layer_leaf_write(context, &layers, &saved);
    env.apply_plan(plan, target.span)
}

fn write_direct_group_entry(
    target: &GroupEntryTarget<'_>,
    keys: &[ExecArg],
    value: Value,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let expected = target.layer_facts.key_params.as_slice();
    let layer_keys = lower_keys(keys, target.span, KeyRole::Layer, None, expected, env)?;
    write_direct_group_entry_at(target, layer_keys, value, env)
}

fn write_direct_group_entry_at(
    target: &GroupEntryTarget<'_>,
    layer_keys: Vec<SavedKey>,
    value: Value,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Value::Resource(fields) = value else {
        return Err(unsupported(
            "assigning a non-resource value to a group entry",
            target.span,
        ));
    };
    let value = resource_value_of(&target.layer_facts.members, fields, target.span)?;
    let layer_address = LayerAddress::from_checked(target.layer_facts, layer_keys);
    let mut layers = target.parent_addresses.to_vec();
    layers.push(layer_address);
    let created_required_paths = created_required_paths_for_value(
        target.place,
        target.identity,
        &layers,
        &target.layer_facts.members,
        &value,
        target.span,
        env,
    )?;
    let context = IndexWriteContext::new(
        target.place,
        target.identity,
        env.store,
        env.program.facts(),
        target.span,
    );
    let plan = plan_layer_group_write(context, &layers, &value);
    let plan = if env.transaction_depth() == 0 {
        plan.and_then(|plan| {
            validate_required_fields_after_group_write(
                target.place,
                target.identity,
                &layers,
                env.store,
                target.span,
            )?;
            Ok(plan)
        })
    } else {
        plan
    };
    env.apply_plan(plan, target.span)?;
    for path in created_required_paths {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(target.place, target.identity, &layers, target.span);
    Ok(())
}
