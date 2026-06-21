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
use crate::path::{KeyRole, lower, lower_keys};
use crate::statement::coerce_error_code_value;
use crate::store::{DataAddress, LayerAddress};
use crate::value::{Value, identity_keys_of, value_to_leaf};
use crate::write::{
    ReferencedIdentity, plan_layer_group_write, plan_layer_identity_leaf_write,
    plan_layer_leaf_write, validate_required_fields_after_group_write,
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

    let target = GroupEntryTarget {
        place,
        identity: &identity,
        parent_addresses: &parent_addresses,
        layer_facts,
        span,
    };

    if let Some(leaf) = &layer_facts.leaf {
        return write_layer_leaf(&target, keys, leaf, value, env);
    }

    write_direct_group_entry(&target, keys, value, env)
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
    value: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let value = coerce_error_code_value(
        eval_expr(value, env)?,
        target.layer_facts.error_code,
        target.span,
    )?;
    let expected = target.layer_facts.key_params.as_slice();
    let layer_keys = lower_keys(keys, target.span, KeyRole::Layer, None, expected, env)?;
    let mut layers = target.parent_addresses.to_vec();
    layers.push(LayerAddress::from_checked(target.layer_facts, layer_keys));

    let context = IndexWriteContext::new(
        target.place,
        target.identity,
        env.store,
        env.program.facts(),
        target.span,
    );
    let plan = match leaf {
        StoreLeafKind::Identity { store_root, arity } => {
            let keys = identity_keys_of(value, store_root, target.span)?;
            plan_layer_identity_leaf_write(
                context,
                &layers,
                ReferencedIdentity {
                    keys: &keys,
                    referenced_arity: *arity,
                },
            )
        }
        StoreLeafKind::Scalar(_) | StoreLeafKind::Enum { .. } => {
            let saved = value_to_leaf(value, leaf, target.span)?;
            plan_layer_leaf_write(context, &layers, &saved)
        }
    };
    env.apply_plan(plan, target.span)
}

fn write_direct_group_entry(
    target: &GroupEntryTarget<'_>,
    keys: &[ExecArg],
    value: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported(
            "assigning a non-resource value to a group entry",
            target.span,
        ));
    };
    let expected = target.layer_facts.key_params.as_slice();
    let layer_keys = lower_keys(keys, target.span, KeyRole::Layer, None, expected, env)?;
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
    env.defer_required_entry_check(target.place, target.identity, &layers);
    Ok(())
}
