//! Write-side evaluation over the managed-write layer.

use crate::*;

/// Apply a managed field write `^root(key…).field = value`. Lowers the path,
/// evaluates the value, and drives [`SavedPath::write`] — which routes a top-level
/// field through the write planner's `plan_field_write` (validating the field and
/// value and keeping generated indexes coherent) and a group-entry target
/// `^root(key…).layer(key…).field = value` through `plan_nested_field_write` — then
/// commits. A planning failure surfaces with its `write.*` code.
pub(crate) fn eval_saved_field_write(
    base: &Expression,
    field: &str,
    quoted: bool,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    // A quoted/raw segment under a managed root (`^books(id)."old-title" = v`) is
    // raw access: gated to maintenance, writing the literal segment rather than a
    // declared field. An unquoted undeclared field stays `write.unknown_field`.
    if quoted {
        let value = eval_expr(value, env)?;
        return eval_raw_field_write(base, field, value, span, env);
    }
    // A plain `^root(id…).field` base is a top-level field write; a base reached
    // through one or more group layers (`^root(id…).layer(key…)….field = v` or the
    // unkeyed group hop `^root(id…).name.field = v`) writes inside that group.
    // Lowering the base and re-terminating at the field carries the layer chain
    // either way, and `SavedPath::write` routes top-level or nested by whether that
    // chain is empty. The path keys are evaluated before the right-hand value.
    let path = lower(base, env)?.into_field(field.to_string(), base.span())?;
    let value = eval_expr(value, env)?;
    path.write(value, span, env)
}

/// Apply a managed top-level field write from a pre-lowered identity and an
/// already-evaluated value, driving the write planner's `plan_field_write` and
/// committing. Shared by [`eval_saved_field_write`] and `out`/`inout` write-back.
pub(crate) fn write_saved_field(
    root: &str,
    identity: &[SavedKey],
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("writing to this saved root", span))?;
    let created_required_path = created_required_field_path(root, identity, &[], field, span, env)?;
    // A typed-reference field (`authorId: Author::Id`) stores the referenced
    // identity's canonical encoding; a scalar/enum field stores its saved value. An
    // unknown field falls through to `plan_field_write`, keeping its
    // `write.unknown_field` diagnostic.
    if let Some(LeafKind::Identity { arity, .. }) = resource_field_leaf(env.program, root, field) {
        let keys = identity_keys_of(value, span)?;
        let plan = plan_identity_field_write(resource, identity, field, &keys, arity);
        let plan = if env.transaction_depth() == 0 {
            let store = env.store.borrow();
            plan.and_then(|plan| {
                validate_required_fields_after_field_write(
                    resource,
                    identity,
                    &[],
                    field,
                    &*store,
                )?;
                Ok(plan)
            })
        } else {
            plan
        };
        env.apply_plan(plan, span)?;
        if let Some(path) = created_required_path {
            env.note_created_required_path(path);
        }
        env.defer_required_entry_check(root, identity, &[]);
        return Ok(());
    }
    let saved = value_to_saved(value)
        .ok_or_else(|| unsupported("writing a resource value to a field", span))?;
    let plan = {
        let store = env.store.borrow();
        plan_field_write(resource, identity, field, &saved, &*store).and_then(|plan| {
            if env.transaction_depth() == 0 {
                validate_required_fields_after_field_write(
                    resource,
                    identity,
                    &[],
                    field,
                    &*store,
                )?;
            }
            Ok(plan)
        })
    };
    env.apply_plan(plan, span)?;
    if let Some(path) = created_required_path {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(root, identity, &[]);
    Ok(())
}

/// Lower a quoted/raw record path `^root(key…)."segment"` to the encoded literal
/// path and gate it on maintenance, returning the path along with the root name and
/// resolved resource so a write can apply its declared-field guard. Quoted segments
/// are for existing raw data, import, export, migration, and repair; without
/// maintenance they are rejected with `write.raw_requires_maintenance` — distinct
/// from `write.unknown_field`, so a tool can tell raw syntax from a declared-field
/// typo. The base must lower to a top-level record identity under a managed root; a
/// raw segment names a literal `Field` directly under the record key, bypassing the
/// resource schema.
pub(crate) fn raw_segment_path<'p>(
    base: &Expression,
    segment: &str,
    span: SourceSpan,
    env: &mut Env<'p>,
) -> Result<(Vec<PathSegment>, String, &'p ResourceSchema), RuntimeError> {
    let (root, identity) = lower(base, env)?.into_record(base.span())?;
    let Some(resource) = find_resource(env.program, &root) else {
        return Err(unsupported("a raw segment under this saved path", span));
    };
    env.require_maintenance(
        WRITE_RAW_REQUIRES_MAINTENANCE,
        format!(
            "`\"{segment}\"` is a raw segment under managed root `^{root}`; \
             declare the field, or run in maintenance mode for raw access"
        ),
        span,
    )?;
    let path = saved_segments(&root, &identity, &[], Some(segment));
    Ok((path, root, resource))
}

/// Write a quoted/raw segment `^root(key…)."segment" = value` under maintenance:
/// the value's canonical bytes are stored at the literal path, bypassing the
/// schema's declared fields and index maintenance (raw data the schema does not
/// model). Off maintenance, [`raw_segment_path`] rejects it.
pub(crate) fn eval_raw_field_write(
    base: &Expression,
    segment: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (path, root, resource) = raw_segment_path(base, segment, span, env)?;
    // A raw segment runs no index maintenance, so clobbering a DECLARED field's
    // literal path would overwrite its stored value while leaving any index it feeds
    // stale. Anything the schema models must go through the managed write; raw access
    // is only for paths the schema does not model. This holds even under maintenance.
    if resource.field_type(&[segment]).is_some() {
        return Err(raise_fault(
            WRITE_RAW_DECLARED_FIELD,
            format!(
                "`\"{segment}\"` is a declared field of `^{root}`; write it as \
                 `^{root}(…).{segment}` so its indexes stay coherent — a raw \
                 segment is only for data the schema does not model"
            ),
            span,
        ));
    }
    // Raw segments are an untyped text boundary: `eval_raw_field_read` decodes them
    // as text, so a raw write takes a string to keep the round-trip symmetric.
    // Convert other scalars explicitly before a raw write.
    if !matches!(value, Value::Str(_)) {
        return Err(type_error(
            &format!(
                "a raw segment `\"{segment}\"` takes a string value; convert before a raw write"
            ),
            span,
        ));
    }
    let saved = value_to_saved(value)
        .ok_or_else(|| unsupported("writing a resource value to a raw segment", span))?;
    let bytes = encode_value(&saved).map_err(|error| error.located(span))?;
    env.store
        .borrow_mut()
        .write(&encode_path(&path), bytes)
        .map_err(|error| error.located(span))?;
    Ok(())
}

/// Write `value` to a scalar field inside a (possibly nested) keyed group entry
/// `^root(key…).layer(key…)….field = value`, a single-field update at any nesting
/// depth (e.g. `^books(id).versions(v).comments(c).text`) that leaves the entry's
/// other members in place. Driven from a lowered path's group chain by both a
/// direct write and an `out`/`inout` place write via [`SavedPath::write`]. Groups
/// carry no generated indexes, so this is a plain replace-in-place write through
/// the write planner's `plan_nested_field_write`.
pub(crate) fn write_nested_field(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("writing to this saved root", span))?;
    let layer_refs: Vec<(&str, &[SavedKey])> = layers
        .iter()
        .map(|(name, keys)| (name.as_str(), keys.as_slice()))
        .collect();
    let layer_names: Vec<&str> = layers.iter().map(|(name, _)| name.as_str()).collect();
    let created_required_path =
        created_required_field_path(root, identity, layers, field, span, env)?;
    // A typed-reference field inside a group entry stores the referenced identity's
    // canonical encoding; a scalar/enum field stores its saved value. An unknown
    // field falls through to keep its `write.unknown_field` diagnostic.
    if let Some(LeafKind::Identity { arity, .. }) =
        resource_nested_member_leaf(env.program, root, &layer_names, field)
    {
        let keys = identity_keys_of(value, span)?;
        let plan =
            plan_nested_identity_field_write(resource, identity, &layer_refs, field, &keys, arity);
        let plan = if env.transaction_depth() == 0 {
            let store = env.store.borrow();
            plan.and_then(|plan| {
                validate_required_fields_after_field_write(
                    resource,
                    identity,
                    &layer_refs,
                    field,
                    &*store,
                )?;
                Ok(plan)
            })
        } else {
            plan
        };
        env.apply_plan(plan, span)?;
        if let Some(path) = created_required_path {
            env.note_created_required_path(path);
        }
        env.defer_required_entry_check(root, identity, &layer_refs);
        return Ok(());
    }
    let saved = value_to_saved(value)
        .ok_or_else(|| unsupported("writing a resource value to a field", span))?;
    let plan = plan_nested_field_write(resource, identity, &layer_refs, field, &saved);
    let plan = if env.transaction_depth() == 0 {
        let store = env.store.borrow();
        plan.and_then(|plan| {
            validate_required_fields_after_field_write(
                resource,
                identity,
                &layer_refs,
                field,
                &*store,
            )?;
            Ok(plan)
        })
    } else {
        plan
    };
    env.apply_plan(plan, span)?;
    if let Some(path) = created_required_path {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(root, identity, &layer_refs);
    Ok(())
}

/// Apply a whole keyed-group-entry write `^root(key…).layer(key…) = value`, where
/// `value` is a materialized [`Value::Resource`]. Lowers its fields to a
/// `ResourceValue` and drives the write planner's `plan_layer_group_write` (replace
/// semantics for the one entry), then commits. Groups carry no generated indexes.
pub(crate) fn eval_group_entry_write(
    record: &Expression,
    layer: &str,
    keys: &[Argument],
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity, parents) = lower(record, env)?.into_layers(record.span())?;
    // A keyed-entry write adds/replaces a key in this layer's key set.
    env.guard_traversed_layer(
        &nested_layer_prefix(&root, &identity, &parents, layer),
        span,
    )?;
    // A declared keyed LEAF (e.g. `tags(pos: int): string`) takes a scalar value
    // written at the keyed path, sharing the write planner's keyed-leaf write path with
    // `append`; an identity-typed keyed leaf stores the referenced identity. A keyed
    // GROUP takes a whole-entry resource value.
    let layer_names: Vec<&str> = parents
        .iter()
        .map(|(name, _)| name.as_str())
        .chain(std::iter::once(layer))
        .collect();
    if let Some(leaf) = resource_layer_leaf_chain(env.program, &root, &layer_names) {
        let resource = find_resource(env.program, &root)
            .ok_or_else(|| unsupported("writing to this saved root", span))?;
        let expected = layer_key_params(env.program, &root, &layer_names);
        let value = eval_expr(value, env)?;
        let layer_keys = lower_keys(keys, span, false, expected, env)?;
        let parent_refs: Vec<(&str, &[SavedKey])> = parents
            .iter()
            .map(|(name, keys)| (name.as_str(), keys.as_slice()))
            .collect();
        let plan = match leaf {
            LeafKind::Identity { arity, .. } => {
                let identity_keys = identity_keys_of(value, span)?;
                if parents.is_empty() {
                    plan_layer_identity_leaf_write(
                        resource,
                        &identity,
                        layer,
                        &layer_keys,
                        &identity_keys,
                        arity,
                    )
                } else {
                    plan_nested_layer_identity_leaf_write(
                        resource,
                        &identity,
                        &parent_refs,
                        layer,
                        &layer_keys,
                        &identity_keys,
                        arity,
                    )
                }
            }
            LeafKind::Scalar(_) => {
                let saved = value_to_saved(value)
                    .ok_or_else(|| unsupported("writing a resource value to a keyed leaf", span))?;
                if parents.is_empty() {
                    plan_layer_leaf_write(resource, &identity, layer, &layer_keys, &saved)
                } else {
                    plan_nested_layer_leaf_write(
                        resource,
                        &identity,
                        &parent_refs,
                        layer,
                        &layer_keys,
                        &saved,
                    )
                }
            }
        };
        env.apply_plan(plan, span)?;
        return Ok(());
    }
    if !parents.is_empty() {
        return Err(unsupported("assigning a nested group entry", span));
    }
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported(
            "assigning a non-resource value to a group entry",
            span,
        ));
    };
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("writing to this saved root", span))?;
    let expected = layer_key_params(env.program, &root, &[layer]);
    let layer_keys = lower_keys(keys, span, false, expected, env)?;
    let group_members = match resource.descend_layers(&[layer]) {
        Some(node) => node.members.as_slice(),
        None => &[],
    };
    let value = resource_value_of(env.program, group_members, fields, span)?;
    let entry_layers = vec![(layer.to_string(), layer_keys.clone())];
    let created_required_paths = created_required_paths_for_value(
        &root,
        &identity,
        &entry_layers,
        group_members,
        &value,
        span,
        env,
    )?;
    let plan = plan_layer_group_write(resource, &identity, layer, &layer_keys, &value);
    env.apply_plan(plan, span)?;
    for path in created_required_paths {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(&root, &identity, &[(layer, layer_keys.as_slice())]);
    Ok(())
}

/// Apply a whole-resource write `^root(key…) = value`, where `value` is a
/// materialized [`Value::Resource`]. Lowers its present fields to a
/// `ResourceValue` and drives the write planner's `plan_resource_write` (replace
/// semantics, keeping generated indexes coherent), then commits.
pub(crate) fn eval_resource_write(
    target: &Expression,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity) = lower(target, env)?.into_record(target.span())?;
    let value = eval_expr(value, env)?;
    write_resource(&root, &identity, value, span, env)
}

/// Apply a whole-resource write from a pre-lowered identity and an
/// already-evaluated [`Value::Resource`], driving
/// the write planner's `plan_resource_write` (replace semantics) and committing.
/// Shared by [`eval_resource_write`] and `out`/`inout` write-back.
pub(crate) fn write_resource(
    root: &str,
    identity: &[SavedKey],
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
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("writing this saved root", span))?;
    // A whole-record write adds/replaces a key in the root's identity layer.
    env.guard_traversed_layer(&[PathSegment::Root(root.into())], span)?;
    let value = resource_value_of(env.program, &resource.members, fields, span)?;
    let created_required_paths = created_required_paths_for_value(
        root,
        identity,
        &[],
        &resource.members,
        &value,
        span,
        env,
    )?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_write(resource, identity, &value, &*store)
    };
    env.apply_plan(plan, span)?;
    for path in created_required_paths {
        env.note_created_required_path(path);
    }
    env.defer_required_entry_check(root, identity, &[]);
    Ok(())
}

/// Apply a managed merge `merge ^root(key…) = value`: drives
/// the write planner's `plan_resource_merge` (copy supplied fields, keep absent ones)
/// and commits. When the source is another saved record of the same root
/// (`merge ^root(to) = ^root(from)`), this is a tree-shaped merge: its child-layer
/// subtrees are copied too, so the source identity is lowered and passed through.
/// A local-value source (`merge ^root(id) = patch`) carries only top-level fields.
pub(crate) fn eval_resource_merge(
    target: &Expression,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity) = lower(target, env)?.into_record(target.span())?;
    // A saved-record source contributes its child-layer subtrees; lower its
    // identity (rejecting a cross-root merge) before reading its scalar fields.
    let source = if is_saved_path(value) {
        let (source_root, source_identity) = lower(value, env)?.into_record(value.span())?;
        if source_root != root {
            return Err(unsupported("merging across saved roots", span));
        }
        Some(source_identity)
    } else {
        None
    };
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported("merging a non-resource value", span));
    };
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("merging this saved root", span))?;
    // A whole-record merge can create a new identity in the root's identity layer.
    env.guard_traversed_layer(&[PathSegment::Root(root.clone())], span)?;
    let value = resource_value_of(env.program, &resource.members, fields, span)?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_merge(resource, &identity, &value, source.as_deref(), &*store)
    };
    env.apply_plan(plan, span)?;
    Ok(())
}

/// Apply a merge into a local resource var `merge draft = source`: overlay each
/// populated source field onto the local binding, leaving the local's other
/// fields in place: a `merge` preserves fields the source does not supply. The
/// local is ordinary program
/// state, so this is a sequence of local-field writes, not a managed saved write.
pub(crate) fn eval_local_merge(
    target: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Value::Resource(fields) = eval_expr(value, env)? else {
        return Err(unsupported("merging a non-resource value", span));
    };
    if env.lookup(target).is_none() {
        return Err(unsupported("merging into an unbound local", span));
    }
    for (field, value) in fields {
        write_local_field(target, &field, value, span, env)?;
    }
    Ok(())
}

/// Apply a keyed-layer merge `merge ^root(to).layer = ^root(from).layer`: copy
/// the source layer's entries over the target layer (an overlay, leaving target
/// entries the source does not cover in place). Both sides must name the same
/// layer of the same saved root. Drives the write planner's `plan_layer_merge`, which
/// reads the source subtree, then commits.
pub(crate) fn eval_layer_merge(
    target_record: &Expression,
    layer: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    // The source is a saved layer path `^root(from).layer` naming the same root
    // and layer as the target.
    let Expression::Field {
        base: source_record,
        name: source_layer,
        ..
    } = value
    else {
        return Err(unsupported("merging this value into a layer", span));
    };
    if source_layer.as_str() != layer {
        return Err(unsupported(
            "merging between differently named layers",
            span,
        ));
    }
    let (to_root, to_identity) = lower(target_record, env)?.into_record(target_record.span())?;
    let (from_root, from_identity) =
        lower(source_record, env)?.into_record(source_record.span())?;
    if from_root != to_root {
        return Err(unsupported("merging a layer across saved roots", span));
    }
    let resource = find_resource(env.program, &to_root)
        .ok_or_else(|| unsupported("merging into this saved root", span))?;
    // A layer merge overlays entries into the target layer's key set.
    env.guard_traversed_layer(&layer_prefix(&to_root, &to_identity, layer), span)?;
    let plan = {
        let store = env.store.borrow();
        plan_layer_merge(resource, &from_identity, &to_identity, layer, &*store)
    };
    env.apply_plan(plan, span)?;
    Ok(())
}

/// The encoded-path prefix of a keyed child layer `^root(identity…).layer` — the
/// layer whose child keys an entry write, append, or layer merge changes. Matches
/// the prefix [`traversed_layer_prefix`] produces for a loop over that layer, so
/// [`Env::guard_traversed_layer`] can compare them.
pub(crate) fn layer_prefix(root: &str, identity: &[SavedKey], layer: &str) -> Vec<PathSegment> {
    nested_layer_prefix(root, identity, &[], layer)
}

pub(crate) fn nested_layer_prefix(
    root: &str,
    identity: &[SavedKey],
    parents: &[(String, Vec<SavedKey>)],
    layer: &str,
) -> Vec<PathSegment> {
    let mut levels = parents.to_vec();
    levels.push((layer.to_string(), Vec::new()));
    saved_segments(root, identity, &levels, None)
}

/// Lower a materialized resource value's present fields to a `ResourceValue` for
/// the managed-write planners: a scalar/enum field lands in `fields`, a typed
/// reference (an identity value) in `identities`. A nested resource field — a value
/// that is neither a scalar nor an identity — is unsupported. `members` are the
/// declared fields the value is being written into (the resource's own, or a group
/// layer's), used to pair each supplied identity with the referenced resource's
/// identity arity so the planner can validate the staged leaf's shape.
pub(crate) fn resource_value_of(
    program: &CheckedProgram,
    members: &[Node],
    fields: Vec<(String, Value)>,
    span: SourceSpan,
) -> Result<ResourceValue, RuntimeError> {
    let mut value = ResourceValue::default();
    collect_resource_value(program, members, fields, &mut Vec::new(), span, &mut value)?;
    Ok(value)
}

fn collect_resource_value(
    program: &CheckedProgram,
    members: &[Node],
    fields: Vec<(String, Value)>,
    prefix: &mut Vec<String>,
    span: SourceSpan,
    out: &mut ResourceValue,
) -> Result<(), RuntimeError> {
    for (name, value) in fields {
        if let Some(group) = members.iter().find(|node| {
            node.name == name
                && node.key_params.is_empty()
                && matches!(node.element, Element::Group)
        }) {
            let Value::Resource(fields) = value else {
                return Err(unsupported(
                    "a non-resource value for an unkeyed group",
                    span,
                ));
            };
            prefix.push(name);
            collect_resource_value(program, &group.members, fields, prefix, span, out)?;
            prefix.pop();
            continue;
        }
        let field = flattened_field_name(prefix, &name);
        // A single-key identity collapses to its bare key value at runtime, so a
        // scalar value could be either a plain field or a single-key reference;
        // the planner disambiguates by the declared field type. An identity value
        // is always a reference. Splitting here keeps the runtime value the source
        // of truth for what was supplied, and the schema for how each lands.
        match value {
            Value::Identity(keys) => {
                // The referenced arity comes from the declared field type. A value
                // supplied for a field the schema does not declare as an identity
                // keeps its own length as the expected arity; the planner's
                // declared-type check then rejects it as a `write.type_mismatch`.
                let referenced_arity =
                    identity_field_arity(program, members, &name).unwrap_or(keys.len());
                out.identities.push(SuppliedIdentity {
                    field,
                    keys,
                    referenced_arity,
                });
            }
            other => {
                let saved = value_to_saved(other)
                    .ok_or_else(|| unsupported("a nested resource field", span))?;
                out.fields.push((field, saved));
            }
        }
    }
    Ok(())
}

fn flattened_field_name(prefix: &[String], name: &str) -> String {
    if prefix.is_empty() {
        return name.to_string();
    }
    let mut field = prefix.join(".");
    field.push('.');
    field.push_str(name);
    field
}

/// The referenced resource's identity arity for a member field declared as a typed
/// reference (`field: Resource::Id`), or `None` when `field` is not declared as a
/// plain identity field in `members`.
fn identity_field_arity(program: &CheckedProgram, members: &[Node], field: &str) -> Option<usize> {
    let ty = members.iter().find_map(|node| match &node.element {
        Element::Slot { ty, .. } if node.name == field && node.key_params.is_empty() => Some(ty),
        _ => None,
    })?;
    match leaf_kind(program, ty)? {
        LeafKind::Identity { arity, .. } => Some(arity),
        LeafKind::Scalar(_) => None,
    }
}

/// Apply a `delete`, dispatching on the target shape: a `.field` off a saved
/// record deletes that field (tearing down any index it feeds, with a guard
/// against deleting a top-level required field); a `.layer(key…)` deletes that
/// keyed entry's subtree; a bare `^root(key…)` or singleton deletes the whole
/// record via the write planner's `plan_resource_delete` (removing the record and its
/// index entries). All commit before returning.
pub(crate) fn eval_delete(
    path: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    // Read the target shape to dispatch, mirroring the merge target-shape pattern:
    // a `.field` off a saved record is a field delete (top-level, or a group-entry
    // field via `is_group_base`); a `.layer(key…)` call off a saved record is a
    // keyed-entry subtree delete; anything else (`^root(key…)` or a singleton
    // `^settings`) is the whole-record delete handled below.
    if let Expression::Field { base, name, .. } = path
        && is_saved_path(base)
    {
        return eval_field_delete(base, name, span, env);
    }
    if let Expression::Call { callee, args, .. } = path
        && let Expression::Field { base, name, .. } = callee.as_ref()
        && is_saved_path(base)
    {
        return eval_layer_entry_delete(base, name, args, span, env);
    }
    // `delete ^books` on a KEYED root (arity >= 1) is a whole managed-root drop:
    // maintenance work, gated on the capability. Deleting one identity
    // (`delete ^books(1)`, a `Call`) and a keyless singleton (`delete ^settings`,
    // arity 0) stay ordinary work and fall through to the record-delete path.
    if let Expression::SavedRoot { name, .. } = path
        && matches!(root_identity_arity(env.program, name), Some(arity) if arity >= 1)
    {
        return eval_whole_root_delete(name, span, env);
    }
    let (root, identity) = lower(path, env)?.into_record(path.span())?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("deleting from this saved root", span))?;
    // Deleting a record removes a key from the root's identity layer.
    env.guard_traversed_layer(&[PathSegment::Root(root.clone())], span)?;
    let plan = {
        let store = env.store.borrow();
        plan_resource_delete(resource, &identity, &*store)
    };
    env.apply_plan(plan, span)?;
    Ok(())
}

/// Drop a whole managed root `delete ^books` (a keyed root). This is maintenance
/// work: without the maintenance capability it is rejected with
/// `write.requires_maintenance`. Under maintenance, one backend subtree delete of
/// the root prefix removes every record AND every generated index branch, since
/// they all sit under `[Root(name)]` (the backend `delete` removes the value and
/// every value below it). The traversal guard still fires against the root prefix,
/// so a root drop during a loop over that root is caught.
pub(crate) fn eval_whole_root_delete(
    name: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    env.require_maintenance(
        WRITE_REQUIRES_MAINTENANCE,
        format!(
            "dropping the whole managed root `^{name}` is maintenance work; \
             run in maintenance mode to drop the root"
        ),
        span,
    )?;
    let root = vec![PathSegment::Root(name.to_string())];
    env.guard_traversed_layer(&root, span)?;
    env.store
        .borrow_mut()
        .delete(&encode_path(&root))
        .map_err(|error| error.located(span))?;
    Ok(())
}

/// Apply a managed field delete `delete ^root(key…).field`. A top-level field
/// (`^books(id).subtitle`) drives the write planner's `plan_field_delete` — removing
/// the field path and tearing down any index it feeds — after the required-field
/// guard. A group-entry field (`^books(id).versions(v).text`) is a plain subtree
/// delete of that one path (group layers carry no generated indexes, so there is
/// nothing to tear down). A top-level field delete does not change
/// any traversed layer's key set, so it is not guarded against the identity layer.
pub(crate) fn eval_field_delete(
    base: &Expression,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    // A field reached through one or more group layers deletes inside that group
    // entry, with no index interaction.
    if is_group_base(base) {
        let (root, identity, layers) = lower(base, env)?.into_layers(base.span())?;
        return delete_nested_field(&root, &identity, &layers, field, span, env);
    }
    let (root, identity) = lower(base, env)?.into_record(base.span())?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("deleting from this saved root", span))?;
    if let Some(group) = resource.members.iter().find(|declared| {
        declared.name == field
            && declared.key_params.is_empty()
            && matches!(declared.element, Element::Group)
    }) {
        let deletes_required = unkeyed_group_has_required_materialized_field(group);
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
        let path = saved_segments(&root, &identity, &[(field.to_string(), Vec::new())], None);
        let required_paths = required_paths_under_group(&root, &identity, &[], field, group);
        let had_required_data = deletes_required
            && env.host.maintenance
            && required_delete_has_preexisting_data(&required_paths, span, env)?;
        env.store
            .borrow_mut()
            .delete(&encode_path(&path))
            .map_err(|error| error.located(span))?;
        if had_required_data {
            env.note_maintenance_required_delete(&root, &identity, &[]);
        }
        return Ok(());
    }
    // Deleting a required field on its own would leave the resource invalid; it is
    // only allowed when the surrounding entry or whole resource is deleted, or
    // under an explicit maintenance run (repair may drop a required field).
    let deletes_required = resource.members.iter().any(|declared| {
        declared.name == field
            && matches!(declared.element, Element::Slot { required, .. } if required)
    });
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
    let path = saved_segments(&root, &identity, &[], Some(field));
    let had_required_data = deletes_required
        && env.host.maintenance
        && required_delete_has_preexisting_data(std::slice::from_ref(&path), span, env)?;
    let plan = {
        let store = env.store.borrow();
        plan_field_delete(resource, &identity, field, &*store)
    };
    env.apply_plan(plan, span)?;
    if had_required_data {
        env.note_maintenance_required_delete(&root, &identity, &[]);
    }
    Ok(())
}

/// Delete a scalar field inside a (possibly nested) keyed group entry,
/// `delete ^root(key…).layer(key…)….field`. Groups carry no generated indexes, so
/// this is a plain subtree delete of the one field path. The innermost layer must
/// declare `field` as a scalar member.
pub(crate) fn delete_nested_field(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let layer_names: Vec<&str> = layers.iter().map(|(name, _)| name.as_str()).collect();
    let layer_refs: Vec<(&str, &[SavedKey])> = layers
        .iter()
        .map(|(name, keys)| (name.as_str(), keys.as_slice()))
        .collect();
    if let Some(group) = nested_unkeyed_group(env.program, root, &layer_names, field) {
        let deletes_required = unkeyed_group_has_required_materialized_field(group);
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
        let mut group_layers = layers.to_vec();
        group_layers.push((field.to_string(), Vec::new()));
        let path = saved_segments(root, identity, &group_layers, None);
        let required_paths = required_paths_under_group(root, identity, layers, field, group);
        let had_required_data = deletes_required
            && env.host.maintenance
            && required_delete_has_preexisting_data(&required_paths, span, env)?;
        env.store
            .borrow_mut()
            .delete(&encode_path(&path))
            .map_err(|error| error.located(span))?;
        if had_required_data {
            env.note_maintenance_required_delete(root, identity, &layer_refs);
        }
        return Ok(());
    }
    if !resource_nested_member_exists(env.program, root, &layer_names, field) {
        return Err(unsupported("deleting this group field", span));
    }
    let deletes_required =
        nested_field_required(env.program, root, &layer_names, field).unwrap_or(false);
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
    let path = saved_segments(root, identity, layers, Some(field));
    let had_required_data = deletes_required
        && env.host.maintenance
        && required_delete_has_preexisting_data(std::slice::from_ref(&path), span, env)?;
    env.store
        .borrow_mut()
        .delete(&encode_path(&path))
        .map_err(|error| error.located(span))?;
    if had_required_data {
        env.note_maintenance_required_delete(root, identity, &layer_refs);
    }
    Ok(())
}

fn created_required_field_path(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    field: &str,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Option<Vec<PathSegment>>, RuntimeError> {
    if env.transaction_depth() == 0 {
        return Ok(None);
    }
    let layer_names: Vec<&str> = layers.iter().map(|(name, _)| name.as_str()).collect();
    if !nested_field_required(env.program, root, &layer_names, field).unwrap_or(false) {
        return Ok(None);
    }
    let path = saved_segments(root, identity, layers, Some(field));
    let absent = env
        .store
        .borrow()
        .read(&encode_path(&path))
        .map_err(|error| error.located(span))?
        .is_none();
    Ok(absent.then_some(path))
}

fn created_required_paths_for_value(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    members: &[Node],
    value: &ResourceValue,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<Vec<PathSegment>>, RuntimeError> {
    if env.transaction_depth() == 0 {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for field in crate::write::materialized_plain_fields(members) {
        if !field.required || !resource_value_supplies(value, &field.path) {
            continue;
        }
        let Some(path) = saved_materialized_field_path(root, identity, layers, &field.path) else {
            continue;
        };
        if env
            .store
            .borrow()
            .read(&encode_path(&path))
            .map_err(|error| error.located(span))?
            .is_none()
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn resource_value_supplies(value: &ResourceValue, field: &[String]) -> bool {
    let name = field.join(".");
    value.fields.iter().any(|(field, _)| field == &name)
        || value
            .identities
            .iter()
            .any(|identity| identity.field == name)
}

fn required_delete_has_preexisting_data(
    paths: &[Vec<PathSegment>],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<bool, RuntimeError> {
    for path in paths {
        if env.required_path_created_in_transaction(path) {
            continue;
        }
        if env
            .store
            .borrow()
            .read(&encode_path(path))
            .map_err(|error| error.located(span))?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn required_paths_under_group(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    group_name: &str,
    group: &Node,
) -> Vec<Vec<PathSegment>> {
    crate::write::materialized_plain_fields(&group.members)
        .into_iter()
        .filter(|field| field.required)
        .filter_map(|field| {
            let mut field_layers = layers.to_vec();
            field_layers.push((group_name.to_string(), Vec::new()));
            saved_materialized_field_path(root, identity, &field_layers, &field.path)
        })
        .collect()
}

fn saved_materialized_field_path(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    field: &[String],
) -> Option<Vec<PathSegment>> {
    let name = field.last()?;
    let mut field_layers = layers.to_vec();
    for group in &field[..field.len().saturating_sub(1)] {
        field_layers.push((group.clone(), Vec::new()));
    }
    Some(saved_segments(root, identity, &field_layers, Some(name)))
}

fn nested_unkeyed_group<'a>(
    program: &'a CheckedProgram,
    root: &str,
    layers: &[&str],
    field: &str,
) -> Option<&'a Node> {
    let resource = find_resource(program, root)?;
    let members = match layers {
        [] => &resource.members,
        _ => &resource.descend_layers(layers)?.members,
    };
    members.iter().find(|node| {
        node.name == field && node.key_params.is_empty() && matches!(node.element, Element::Group)
    })
}

fn nested_field_required(
    program: &CheckedProgram,
    root: &str,
    layers: &[&str],
    field: &str,
) -> Option<bool> {
    let resource = find_resource(program, root)?;
    let members = match layers {
        [] => &resource.members,
        _ => &resource.descend_layers(layers)?.members,
    };
    members.iter().find_map(|node| match &node.element {
        Element::Slot { required, .. } if node.name == field && node.key_params.is_empty() => {
            Some(*required)
        }
        _ => None,
    })
}

fn unkeyed_group_has_required_materialized_field(group: &Node) -> bool {
    crate::write::materialized_plain_fields(&group.members)
        .into_iter()
        .any(|field| field.required)
}

/// Apply a keyed-entry subtree delete `delete ^root(key…).layer(entryKey…)`. The
/// backend `delete` is a subtree delete, so one delete of the entry prefix removes
/// the whole entry (a keyed leaf value, or a group entry with all its members and
/// nested layers). Child layers feed no generated index, so there is no index
/// maintenance. The guard fires against the layer prefix so a self-mutating
/// traversal of that layer is still caught.
pub(crate) fn eval_layer_entry_delete(
    record: &Expression,
    layer: &str,
    keys: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let (root, identity, chain) = lower(record, env)?.into_layers(record.span())?;
    // The full layer chain the delete targets must be declared on the resource.
    let layer_names: Vec<&str> = chain
        .iter()
        .map(|(name, _)| name.as_str())
        .chain(std::iter::once(layer))
        .collect();
    let expected = layer_key_params(env.program, &root, &layer_names);
    let entry_keys = lower_keys(keys, span, false, expected, env)?;
    if !resource_layer_chain_exists(env.program, &root, &layer_names) {
        return Err(unsupported("deleting this layer entry", span));
    }
    // Deleting an entry changes the innermost layer's key set, so guard against
    // that layer's prefix whether it sits directly under the record or below
    // keyed group entries.
    env.guard_traversed_layer(&nested_layer_prefix(&root, &identity, &chain, layer), span)?;
    // The full level chain to the entry: the lowered group chain plus the terminal
    // keyed layer being deleted.
    let mut levels = chain;
    levels.push((layer.to_string(), entry_keys));
    let path = saved_segments(&root, &identity, &levels, None);
    env.store
        .borrow_mut()
        .delete(&encode_path(&path))
        .map_err(|error| error.located(span))?;
    Ok(())
}

/// Set a field of a local resource variable, e.g. `book.title = t`. The base
/// must be a mutable local bound to a resource value; the field is updated (or
/// inserted) and the variable rebound.
pub(crate) fn eval_local_field_set(
    base: &Expression,
    field: &str,
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Expression::Name { segments, .. } = base else {
        return Err(unsupported("setting a field of this value", span));
    };
    let [name] = segments.as_slice() else {
        return Err(unsupported("setting a field of this value", span));
    };
    let new_value = eval_expr(value, env)?;
    write_local_field(name, field, new_value, span, env)
}

/// Update (or insert) `field` of the local resource bound to `base` with an
/// already-evaluated value, rebinding the variable. Shared by
/// [`eval_local_field_set`] and `out`/`inout` write-back.
pub(crate) fn write_local_field(
    base: &str,
    field: &str,
    value: Value,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<(), RuntimeError> {
    let Some(Value::Resource(mut fields)) = env.lookup(base).cloned() else {
        return Err(unsupported("setting a field of a non-resource local", span));
    };
    match fields.iter().position(|(existing, _)| existing == field) {
        Some(index) => fields[index].1 = value,
        None => fields.push((field.to_string(), value)),
    }
    env.assign(base, Value::Resource(fields))
        .map_err(|error| assign_error(base, error, span))
}
