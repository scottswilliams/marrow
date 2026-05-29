//! Managed writes over the saved store.
//!
//! This layer composes the typed tree shape from `marrow-schema` with the
//! ordered-bytes store from `marrow-store`. A managed write is planned in full —
//! validated against the schema and lowered to encoded paths — before any change
//! is visible, so a rejected write leaves the store untouched and a committed
//! one is internally coherent.
//!
//! It covers whole-resource writes, single-field writes, deletes, and merges
//! over a resource's top-level fields, keeping generated indexes (unique and
//! non-unique) coherent. Keyed-layer writes — leaf entries and group-entry
//! fields — build on this.

use marrow_schema::{LayerMember, LayerSchema, ResourceSchema, SavedRootSchema};
use marrow_store::backend::Backend;
use marrow_store::mem::StoreError;
use marrow_store::path::{
    ChildSegment, PathSegment, SavedKey, decode_key_value, encode_key_value, encode_path,
};
use marrow_store::value::{SavedValue, ValueError, ValueType, decode_value, encode_value};

/// A field's value in a write: a saved value, or explicitly absent (omitted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Saved(SavedValue),
    Absent,
}

/// A resource value supplied to a write: its top-level fields, by name. Keyed
/// layers are added in a later slice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceValue {
    pub fields: Vec<(String, FieldValue)>,
}

/// A managed write that could not be planned. `code` is a stable `write.*`
/// identifier, mirroring the dotted codes used by the checker and store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteError {
    pub code: &'static str,
    pub message: String,
}

/// A required field was absent in a whole-resource write.
pub const WRITE_REQUIRED_ABSENT: &str = "write.required_absent";
/// A field value's type does not match the resource schema.
pub const WRITE_TYPE_MISMATCH: &str = "write.type_mismatch";
/// The resource has no saved root, so it cannot be written to saved data.
pub const WRITE_NO_SAVED_ROOT: &str = "write.no_saved_root";
/// The supplied identity keys do not match the resource's saved root.
pub const WRITE_IDENTITY_MISMATCH: &str = "write.identity_mismatch";
/// The store reported an error (e.g. a corrupt stored path) during a write.
pub const WRITE_STORE: &str = "write.store";
/// A field write names a field the resource does not declare.
pub const WRITE_UNKNOWN_FIELD: &str = "write.unknown_field";
/// A unique index already maps the supplied key(s) to a different resource, so
/// committing this write would violate the uniqueness constraint.
pub const WRITE_UNIQUE_CONFLICT: &str = "write.unique_conflict";
/// A keyed-layer write names a layer the resource does not declare.
pub const WRITE_UNKNOWN_LAYER: &str = "write.unknown_layer";
/// A keyed-leaf write targets a group layer, which holds nested members rather
/// than a single leaf value.
pub const WRITE_NOT_A_LEAF_LAYER: &str = "write.not_a_leaf_layer";
/// A group-entry field write targets a leaf layer, which holds a single value
/// rather than nested members.
pub const WRITE_NOT_A_GROUP_LAYER: &str = "write.not_a_group_layer";
/// A keyed-layer write supplies the wrong number of layer keys.
pub const WRITE_LAYER_KEY_ARITY: &str = "write.layer_key_arity";
/// The integer key space is exhausted: the highest existing key is `i64::MAX`,
/// so no next identity or layer position can be allocated.
pub const WRITE_ID_OVERFLOW: &str = "write.id_overflow";
/// `nextId` was asked for a root whose identity shape has no default integer
/// allocation policy: a composite identity (two or more keys), a single
/// non-integer identity key, or a keyless singleton root. The default per-root
/// policy is only available for a resource with one `int` identity key; other
/// identity shapes are application-provided (builtins.md:180-183,
/// types.md:262-263). Distinct from `write.no_saved_root` so a tool can tell a
/// local/singleton resource from one whose identity is simply not auto-allocated.
pub const WRITE_NEXT_ID_UNSUPPORTED: &str = "write.next_id_unsupported";
/// Deleting a `required` field on its own is rejected: a required field can only
/// go away when its surrounding keyed entry or whole resource is deleted
/// (docs/language `resources-and-storage.md`). The runtime enforces this guard
/// before planning, since `plan_field_delete` itself only sees one field. The
/// guard lifts under an explicit maintenance run, which may drop a required field.
pub const WRITE_REQUIRED_FIELD: &str = "write.required_field";
/// A maintenance-only managed operation — dropping a whole managed root
/// (`delete ^books`) — was attempted without the maintenance capability.
/// Deleting one identity is ordinary work; dropping the whole root is
/// maintenance work that code must opt into (docs/language
/// `resources-and-storage.md`, "Delete And Merge"). The runtime enforces this.
pub const WRITE_REQUIRES_MAINTENANCE: &str = "write.requires_maintenance";
/// A quoted/raw path segment under a managed root (`^books(id)."old-title"`) was
/// used outside maintenance. Quoted segments are for existing raw data, import,
/// export, migration, and repair; they do not create undeclared fields. Without
/// maintenance the runtime rejects them — distinct from `write.unknown_field`, so
/// a tool can tell "you used raw syntax" from "you typo'd a declared field"
/// (docs/language `resources-and-storage.md`, "Managed Saved Trees").
pub const WRITE_RAW_REQUIRES_MAINTENANCE: &str = "write.raw_requires_maintenance";

/// Wrap a store error met while planning a write into a `write.store` failure.
fn store_failed(error: StoreError) -> WriteError {
    WriteError {
        code: WRITE_STORE,
        message: format!("the store could not be read while planning: {error:?}"),
    }
}

/// Wrap a value-encoding error (e.g. a date/instant outside year 0001-9999) met
/// while planning a write, preserving the codec's stable dotted code so the
/// write is rejected rather than persisting a non-canonical value.
fn encode_failed(error: ValueError) -> WriteError {
    WriteError {
        code: error.code(),
        message: error.to_string(),
    }
}

/// One staged store operation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PlanStep {
    Write { path: Vec<u8>, value: Vec<u8> },
    Delete { path: Vec<u8> },
}

/// A staged, validated set of store operations. Apply it with
/// [`WritePlan::commit`]; drop it to abandon the write with no effect.
///
/// A plan is validated against the store as read at plan time — including unique
/// conflict checks — so a backend with concurrent writers must serialize
/// plan-then-commit externally. The in-memory store is single-writer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WritePlan {
    steps: Vec<PlanStep>,
}

impl WritePlan {
    /// Apply the staged operations to `store`, in order. A backend write may fail
    /// (e.g. a persistent store meeting I/O), so this is fallible.
    pub fn commit(self, store: &mut dyn Backend) -> Result<(), StoreError> {
        for step in self.steps {
            match step {
                PlanStep::Write { path, value } => store.write(&path, value)?,
                PlanStep::Delete { path } => store.delete(&path)?,
            }
        }
        Ok(())
    }
}

/// Plan a whole-resource write: replace the resource at `identity` with `value`.
/// Validates required fields and value types against `schema`, and rejects a
/// unique-index conflict, before staging anything; then plans to clear the old
/// subtree, write each present field, and keep generated index entries coherent
/// (delete the entries for the currently-stored values, write entries for the
/// new values). `store` is read, not written; apply the returned [`WritePlan`]
/// with [`WritePlan::commit`]. Returns a [`WriteError`] if the value does not
/// satisfy the schema or a unique key is already held by another resource.
pub fn plan_resource_write(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    value: &ResourceValue,
    store: &dyn Backend,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, identity)?;

    // Validate every field and collect the ones to write, before staging any
    // step — a rejected write must leave no trace.
    let mut to_write = Vec::new();
    for field in &schema.fields {
        match supplied_value(value, &field.name) {
            Some(FieldValue::Saved(saved)) => {
                check_type(&field.name, &field.ty.text, saved)?;
                to_write.push((field.name.as_str(), saved));
            }
            Some(FieldValue::Absent) | None => {
                if field.required {
                    return Err(WriteError {
                        code: WRITE_REQUIRED_ABSENT,
                        message: format!("required field `{}` is absent", field.name),
                    });
                }
            }
        }
    }

    // Reject a unique-index conflict before staging anything: a populated unique
    // key already held by a different identity blocks the write.
    for index in &schema.indexes {
        if index.unique {
            let new_keys = index_keys(&index.args, root, identity, value);
            check_unique_conflict(&index.name, root, identity, new_keys.as_deref(), store)?;
        }
    }

    // Replace semantics: clear the old subtree, then write the present fields.
    let mut steps = vec![PlanStep::Delete {
        path: encode_path(&identity_path(root, identity)),
    }];
    for (name, saved) in to_write {
        steps.push(PlanStep::Write {
            path: encode_path(&field_path(root, identity, name)),
            value: encode_value(saved).map_err(encode_failed)?,
        });
    }

    // Keep generated index entries coherent: delete the entry for the
    // currently-stored values, then write the entry for the new values. An entry
    // exists only when every indexed value is populated. A unique entry stores
    // the owning identity; a non-unique entry stores a presence marker.
    for index in &schema.indexes {
        if let Some(old_keys) =
            stored_index_keys(&index.args, root, identity, schema, store).map_err(store_failed)?
        {
            steps.push(PlanStep::Delete {
                path: encode_path(&index_path(root, &index.name, &old_keys)),
            });
        }
        if let Some(new_keys) = index_keys(&index.args, root, identity, value) {
            steps.push(PlanStep::Write {
                path: encode_path(&index_path(root, &index.name, &new_keys)),
                value: index_entry_value(index.unique, identity),
            });
        }
    }
    Ok(WritePlan { steps })
}

/// Plan a whole-resource delete: remove the resource at `identity` and tear down
/// its generated index entries (found by reading `store`). Returns a
/// [`WriteError`] only when the resource has no saved root or the identity arity
/// is wrong; deleting an absent resource is a successful no-op plan.
pub fn plan_resource_delete(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    store: &dyn Backend,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, identity)?;
    let mut steps = vec![PlanStep::Delete {
        path: encode_path(&identity_path(root, identity)),
    }];
    for index in &schema.indexes {
        if let Some(keys) =
            stored_index_keys(&index.args, root, identity, schema, store).map_err(store_failed)?
        {
            steps.push(PlanStep::Delete {
                path: encode_path(&index_path(root, &index.name, &keys)),
            });
        }
    }
    Ok(WritePlan { steps })
}

/// Plan a managed field write: set `field` of the resource at `identity` to
/// `value`, leaving the resource's other fields in place. Validates that the
/// field is declared and that `value` matches its type, rejects a unique-index
/// conflict, then stages the single field write and keeps any index the field
/// participates in coherent (remove the entry for the currently-stored value,
/// add the entry for the new value — docs/language `resources-and-storage.md`).
/// `store` is read, not written; apply the returned [`WritePlan`] with
/// [`WritePlan::commit`]. Returns a [`WriteError`] if the field is unknown, the
/// value is mistyped, or a unique key is already held by another resource.
///
/// This is a current-only update; it never clears the resource's other fields.
/// Whether a field write may create a new (and possibly required-incomplete)
/// resource is a transaction-contextual rule the runtime enforces, not here.
pub fn plan_field_write(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    field: &str,
    value: &SavedValue,
    store: &dyn Backend,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, identity)?;
    let declared = schema
        .fields
        .iter()
        .find(|declared| declared.name == field)
        .ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("resource `{}` has no field `{field}`", schema.name),
        })?;
    check_type(field, &declared.ty.text, value)?;

    // Reject a unique-index conflict on the written field before staging.
    for index in &schema.indexes {
        if index.unique && index.args.iter().any(|arg| arg == field) {
            let new_keys =
                field_write_index_keys(&index.args, root, identity, field, value, schema, store)
                    .map_err(store_failed)?;
            check_unique_conflict(&index.name, root, identity, new_keys.as_deref(), store)?;
        }
    }

    let mut steps = vec![PlanStep::Write {
        path: encode_path(&field_path(root, identity, field)),
        value: encode_value(value).map_err(encode_failed)?,
    }];

    // Keep any index the field feeds coherent: remove the entry for the
    // currently-stored values, then add the entry for the values after this
    // write. Other index arguments keep their stored values. A unique entry
    // stores the owning identity; a non-unique entry stores a presence marker.
    for index in &schema.indexes {
        if !index.args.iter().any(|arg| arg == field) {
            continue;
        }
        if let Some(old_keys) =
            stored_index_keys(&index.args, root, identity, schema, store).map_err(store_failed)?
        {
            steps.push(PlanStep::Delete {
                path: encode_path(&index_path(root, &index.name, &old_keys)),
            });
        }
        if let Some(new_keys) =
            field_write_index_keys(&index.args, root, identity, field, value, schema, store)
                .map_err(store_failed)?
        {
            steps.push(PlanStep::Write {
                path: encode_path(&index_path(root, &index.name, &new_keys)),
                value: index_entry_value(index.unique, identity),
            });
        }
    }
    Ok(WritePlan { steps })
}

/// Plan a managed field delete: remove `field` of the resource at `identity`,
/// leaving the resource's other fields in place, and tear down any index the
/// field feeds. Validates that the field is declared (`WRITE_UNKNOWN_FIELD`),
/// then stages the field-path delete plus, for each index whose key the field is
/// part of, a delete of the currently-stored index entry — removing the field
/// makes that key incomplete, so the entry must go (docs/language
/// `resources-and-storage.md`: an index entry exists only when every indexed
/// value is populated). This is the delete half of [`plan_field_write`]'s index
/// reconciliation: teardown only, with no new entry to add. Deleting an already
/// absent field is a successful no-op plan. `store` is read, not written.
pub fn plan_field_delete(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    field: &str,
    store: &dyn Backend,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, identity)?;
    if !schema.fields.iter().any(|declared| declared.name == field) {
        return Err(WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("resource `{}` has no field `{field}`", schema.name),
        });
    }

    let mut steps = vec![PlanStep::Delete {
        path: encode_path(&field_path(root, identity, field)),
    }];

    // Tear down every index entry the field feeds: with the field gone its key is
    // incomplete, so the stored entry no longer corresponds to a populated key.
    // There is no replacement entry — unlike a field write, a delete only removes.
    for index in &schema.indexes {
        if !index.args.iter().any(|arg| arg == field) {
            continue;
        }
        if let Some(old_keys) =
            stored_index_keys(&index.args, root, identity, schema, store).map_err(store_failed)?
        {
            steps.push(PlanStep::Delete {
                path: encode_path(&index_path(root, &index.name, &old_keys)),
            });
        }
    }
    Ok(WritePlan { steps })
}

/// Plan a managed merge: copy the supplied fields of `value` over the resource
/// already stored at `identity`, leaving stored fields the merge does not supply
/// untouched (a partial update, not a replace — docs/language
/// `resources-and-storage.md`). An omitted or [`FieldValue::Absent`] field is
/// left as stored; clearing a field is `delete`, not `merge`. Validates supplied
/// field types and that every required field is populated AFTER the merge
/// (supplied here or already stored), and rejects a unique conflict, before
/// staging. Generated index entries are kept coherent against the EFFECTIVE
/// (merged-over-stored) resource: an entry whose key is unchanged is left in
/// place, one whose key changes is moved. `store` is read, not written.
pub fn plan_resource_merge(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    value: &ResourceValue,
    source: Option<&[SavedKey]>,
    store: &dyn Backend,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, identity)?;

    // Validate the supplied fields and collect the ones to write. A required
    // field the merge does not supply must already be stored — the resulting
    // resource must still satisfy required fields.
    let mut to_write = Vec::new();
    for field in &schema.fields {
        match supplied_value(value, &field.name) {
            Some(FieldValue::Saved(saved)) => {
                check_type(&field.name, &field.ty.text, saved)?;
                to_write.push((field.name.as_str(), saved));
            }
            Some(FieldValue::Absent) | None => {
                if field.required
                    && store
                        .read(&encode_path(&field_path(root, identity, &field.name)))
                        .map_err(store_failed)?
                        .is_none()
                {
                    return Err(WriteError {
                        code: WRITE_REQUIRED_ABSENT,
                        message: format!(
                            "required field `{}` is absent and not already stored",
                            field.name
                        ),
                    });
                }
            }
        }
    }

    // Reject a unique-index conflict on the effective resource before staging.
    for index in &schema.indexes {
        if index.unique {
            let new_keys = effective_index_keys(&index.args, root, identity, value, schema, store)
                .map_err(store_failed)?;
            check_unique_conflict(&index.name, root, identity, new_keys.as_deref(), store)?;
        }
    }

    // Stage the field overwrites — no subtree clear, so untouched fields remain.
    let mut steps = Vec::new();
    for (name, saved) in to_write {
        steps.push(PlanStep::Write {
            path: encode_path(&field_path(root, identity, name)),
            value: encode_value(saved).map_err(encode_failed)?,
        });
    }

    // Reconcile each index against the effective resource: an unchanged key is
    // left alone (so an entry resting on an untouched field survives), a changed
    // key moves.
    for index in &schema.indexes {
        let old_keys =
            stored_index_keys(&index.args, root, identity, schema, store).map_err(store_failed)?;
        let new_keys = effective_index_keys(&index.args, root, identity, value, schema, store)
            .map_err(store_failed)?;
        if old_keys == new_keys {
            continue;
        }
        if let Some(old_keys) = &old_keys {
            steps.push(PlanStep::Delete {
                path: encode_path(&index_path(root, &index.name, old_keys)),
            });
        }
        if let Some(new_keys) = &new_keys {
            steps.push(PlanStep::Write {
                path: encode_path(&index_path(root, &index.name, new_keys)),
                value: index_entry_value(index.unique, identity),
            });
        }
    }

    // A merge copies a whole tree, not just the top-level scalars. When the source
    // is a saved identity, overlay each of its child-layer subtrees (history,
    // sequences, keyed trees) onto the matching target layer, reusing the layer
    // overlay so target entries the source does not cover are preserved. The
    // overlay reads the source subtree at plan time, before any target change.
    // Generated indexes do not span child layers, so this needs no index work.
    if let Some(source) = source {
        for layer in &schema.layers {
            let layer_plan = plan_layer_merge(schema, source, identity, &layer.name, store)?;
            steps.extend(layer_plan.steps);
        }
    }
    Ok(WritePlan { steps })
}

/// Plan a keyed-leaf write: set the entry at `^root(identity).layer(key)` to
/// `value`. `layer` must be a declared keyed LEAF (e.g. `tags(pos: int):
/// string`), `key` must match the layer's key arity, and `value` must match the
/// leaf type. A keyed leaf holds a single value at one path, so this is a plain
/// replace-in-place write with no index maintenance — generated indexes do not
/// span keyed child layers (docs/language `resources-and-storage.md`). Returns a
/// [`WriteError`] if the layer is unknown, is a group rather than a leaf, the key
/// arity is wrong, or the value is mistyped.
pub fn plan_layer_leaf_write(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    layer: &str,
    key: &[SavedKey],
    value: &SavedValue,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, identity)?;
    let declared = schema
        .layers
        .iter()
        .find(|declared| declared.name == layer)
        .ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_LAYER,
            message: format!("resource `{}` has no keyed layer `{layer}`", schema.name),
        })?;
    let leaf_type = declared.leaf_type.as_ref().ok_or_else(|| WriteError {
        code: WRITE_NOT_A_LEAF_LAYER,
        message: format!("keyed layer `{layer}` is a group, not a leaf"),
    })?;
    if key.len() != declared.key_params.len() {
        return Err(WriteError {
            code: WRITE_LAYER_KEY_ARITY,
            message: format!(
                "keyed layer `{layer}` expects {} key(s), got {}",
                declared.key_params.len(),
                key.len()
            ),
        });
    }
    check_type(layer, &leaf_type.text, value)?;
    Ok(WritePlan {
        steps: vec![PlanStep::Write {
            path: encode_path(&layer_leaf_path(root, identity, layer, key)),
            value: encode_value(value).map_err(encode_failed)?,
        }],
    })
}

/// Plan a group-entry field write: set `field` of the keyed group entry at
/// `^root(identity).layer(key…)` to `value`. `layer` must be a declared GROUP
/// layer (e.g. `versions(version: int)` or `notes(noteId: string)`), `key` must
/// match the layer's key arity, `field` must be a scalar member of that group,
/// and `value` must match the member's type. A group-entry field holds a single
/// value at one path, and generated indexes do not span keyed child layers
/// (docs/language `resources-and-storage.md`), so this is a plain replace-in-
/// place write with no index maintenance; it leaves the entry's other members in
/// place. Returns a [`WriteError`] if the layer is unknown, is a leaf rather than
/// a group, the key arity is wrong, the field is not a scalar member, or the
/// value is mistyped.
pub fn plan_layer_field_write(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    layer: &str,
    key: &[SavedKey],
    field: &str,
    value: &SavedValue,
) -> Result<WritePlan, WriteError> {
    plan_nested_field_write(schema, identity, &[(layer, key)], field, value)
}

/// Plan a field write into a (possibly nested) keyed group entry, descending the
/// `layers` chain of `(layer, key…)` levels from the resource. Each level must
/// name a group layer with matching key arity; the field is a scalar member of
/// the innermost layer. Like the single-level case, groups carry no generated
/// indexes (`schema.index_in_group`), so this is a plain replace-in-place write.
pub fn plan_nested_field_write(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    layers: &[(&str, &[SavedKey])],
    field: &str,
    value: &SavedValue,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, identity)?;
    let innermost = descend_group_layers(schema, layers)?;
    let member = innermost
        .members
        .iter()
        .find_map(|member| match member {
            LayerMember::Field(member) if member.name == field => Some(member),
            _ => None,
        })
        .ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_FIELD,
            message: format!("group layer `{}` has no field `{field}`", innermost.name),
        })?;
    check_type(field, &member.ty.text, value)?;
    let mut path = nested_layer_path(root, identity, layers);
    path.push(PathSegment::Field(field.into()));
    Ok(WritePlan {
        steps: vec![PlanStep::Write {
            path: encode_path(&path),
            value: encode_value(value).map_err(encode_failed)?,
        }],
    })
}

/// Plan a whole keyed-group-entry write: replace the entry
/// `^root(identity).layer(key…)` with the supplied field `value`s — like a
/// whole-resource write scoped to one group entry (required group fields must be
/// present and typed). Groups carry no generated indexes, so there is no index
/// maintenance. Errors when the resource has no saved root, the identity or key
/// arity is wrong, the layer is unknown or a leaf, or a required field is absent or
/// mistyped.
pub fn plan_layer_group_write(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    layer: &str,
    key: &[SavedKey],
    value: &ResourceValue,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, identity)?;
    let declared = schema
        .layers
        .iter()
        .find(|declared| declared.name == layer)
        .ok_or_else(|| WriteError {
            code: WRITE_UNKNOWN_LAYER,
            message: format!("resource `{}` has no keyed layer `{layer}`", schema.name),
        })?;
    if declared.leaf_type.is_some() {
        return Err(WriteError {
            code: WRITE_NOT_A_GROUP_LAYER,
            message: format!("keyed layer `{layer}` is a leaf, not a group"),
        });
    }
    if key.len() != declared.key_params.len() {
        return Err(WriteError {
            code: WRITE_LAYER_KEY_ARITY,
            message: format!(
                "keyed layer `{layer}` expects {} key(s), got {}",
                declared.key_params.len(),
                key.len()
            ),
        });
    }

    // Validate every group field and collect the ones to write before staging any
    // step — a rejected write must leave no trace.
    let mut to_write = Vec::new();
    for member in &declared.members {
        let LayerMember::Field(field) = member else {
            continue;
        };
        match supplied_value(value, &field.name) {
            Some(FieldValue::Saved(saved)) => {
                check_type(&field.name, &field.ty.text, saved)?;
                to_write.push((field.name.as_str(), saved));
            }
            Some(FieldValue::Absent) | None => {
                if field.required {
                    return Err(WriteError {
                        code: WRITE_REQUIRED_ABSENT,
                        message: format!("required field `{}` is absent", field.name),
                    });
                }
            }
        }
    }

    // Replace semantics: clear the old entry subtree, then write the present fields.
    let mut steps = vec![PlanStep::Delete {
        path: encode_path(&layer_leaf_path(root, identity, layer, key)),
    }];
    for (name, saved) in to_write {
        steps.push(PlanStep::Write {
            path: encode_path(&layer_field_path(root, identity, layer, key, name)),
            value: encode_value(saved).map_err(encode_failed)?,
        });
    }
    Ok(WritePlan { steps })
}

/// Plan a keyed-layer merge: copy every entry of the source layer
/// `^root(from).layer` over the target layer `^root(to).layer`, leaving target
/// entries the source does not supply in place (an overlay, not a replace —
/// docs/language `resources-and-storage.md`). Both records belong to the same
/// resource and layer; the source subtree is read from `store` before any target
/// change. Generated indexes do not span keyed child layers, so there is no index
/// maintenance. Returns a [`WriteError`] if the resource has no saved root, an
/// identity arity is wrong, or the layer is unknown.
pub fn plan_layer_merge(
    schema: &ResourceSchema,
    from: &[SavedKey],
    to: &[SavedKey],
    layer: &str,
    store: &dyn Backend,
) -> Result<WritePlan, WriteError> {
    let root = resolve_saved_root(schema, from)?;
    if to.len() != root.identity_keys.len() {
        return Err(WriteError {
            code: WRITE_IDENTITY_MISMATCH,
            message: format!(
                "resource `{}` expects {} identity key(s), got {}",
                schema.name,
                root.identity_keys.len(),
                to.len()
            ),
        });
    }
    if !schema.layers.iter().any(|declared| declared.name == layer) {
        return Err(WriteError {
            code: WRITE_UNKNOWN_LAYER,
            message: format!("resource `{}` has no keyed layer `{layer}`", schema.name),
        });
    }
    let mut source = identity_path(root, from);
    source.push(PathSegment::ChildLayer(layer.into()));
    let source = encode_path(&source);
    let mut target = identity_path(root, to);
    target.push(PathSegment::ChildLayer(layer.into()));
    let target = encode_path(&target);

    // Overlay: copy each source entry to the matching target path — the suffix
    // after the layer prefix (keys and any nested fields) is identical — and
    // leave target entries the source does not cover untouched.
    let page = store.scan(&source, usize::MAX).map_err(store_failed)?;
    let mut steps = Vec::with_capacity(page.entries.len());
    for (path, value) in page.entries {
        let mut target_path = target.clone();
        target_path.extend_from_slice(&path[source.len()..]);
        steps.push(PlanStep::Write {
            path: target_path,
            value,
        });
    }
    Ok(WritePlan { steps })
}

/// A resource's saved root, or `WRITE_NO_SAVED_ROOT` when it has none (a local or
/// singleton resource). Shared by [`resolve_saved_root`] (which adds an arity
/// check against a supplied identity) and [`next_id`] (which has no identity).
fn saved_root_of(schema: &ResourceSchema) -> Result<&SavedRootSchema, WriteError> {
    schema.saved_root.as_ref().ok_or_else(|| WriteError {
        code: WRITE_NO_SAVED_ROOT,
        message: format!("resource `{}` has no saved root", schema.name),
    })
}

/// Resolve a resource's saved root and check the supplied identity has the
/// expected number of keys.
fn resolve_saved_root<'a>(
    schema: &'a ResourceSchema,
    identity: &[SavedKey],
) -> Result<&'a SavedRootSchema, WriteError> {
    let root = saved_root_of(schema)?;
    if identity.len() != root.identity_keys.len() {
        return Err(WriteError {
            code: WRITE_IDENTITY_MISMATCH,
            message: format!(
                "resource `{}` expects {} identity key(s), got {}",
                schema.name,
                root.identity_keys.len(),
                identity.len()
            ),
        });
    }
    Ok(root)
}

/// Does this saved root qualify for the default `nextId` policy? The policy is
/// available only for a resource with exactly one `int` identity key; composite
/// identities, non-integer identities, and keyless singletons are
/// application-provided (builtins.md:180-183, types.md:262-263). This predicate
/// is the single contract the runtime gate (here) and the checker
/// (`marrow-check`'s `check_next_id`) must agree on; the checker keeps a mirror
/// copy because it cannot depend on this crate.
pub fn single_int_root(root: &SavedRootSchema) -> bool {
    matches!(root.identity_keys.as_slice(), [key] if key.ty.text.trim() == "int")
}

/// The next identity for a single-`int` keyed saved root: one greater than the
/// highest existing integer record key, or `1` when the root is empty. This is
/// the default `nextId` policy (builtins.md:180-183). Non-integer immediate
/// children — such as index names — are ignored.
///
/// The single-`int`-root gate is enforced here, not just documented: a resource
/// with no saved root yields `WRITE_NO_SAVED_ROOT`, and a composite, non-integer,
/// or keyless-singleton root yields [`WRITE_NEXT_ID_UNSUPPORTED`]. Taking the
/// `&ResourceSchema` (rather than a bare root name) lets the function decide the
/// policy from the schema, mirroring `next_layer_pos`.
pub fn next_id(schema: &ResourceSchema, store: &dyn Backend) -> Result<i64, WriteError> {
    let root = saved_root_of(schema)?;
    if !single_int_root(root) {
        return Err(WriteError {
            code: WRITE_NEXT_ID_UNSUPPORTED,
            message: format!(
                "`nextId` has no default allocation policy for `{}`: {}; the default \
                 per-root policy is only available for a resource with one `int` \
                 identity key",
                schema.name,
                next_id_shape(root),
            ),
        });
    }
    let children = store
        .child_keys(&encode_path(&[PathSegment::Root(root.root.clone())]))
        .map_err(|_| WriteError {
            code: WRITE_STORE,
            message: format!("could not read records under `^{}`", root.root),
        })?;
    let highest = children
        .iter()
        .filter_map(|child| match child {
            ChildSegment::Key(SavedKey::Int(value)) => Some(*value),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    next_after(highest)
}

/// Name the identity shape that disqualifies a root from the default `nextId`
/// policy, for the rejection message: a composite identity (two or more keys), a
/// single non-integer key, or a keyless singleton root.
fn next_id_shape(root: &SavedRootSchema) -> String {
    match root.identity_keys.as_slice() {
        [] => "this root is a keyless singleton".into(),
        [key] => format!(
            "its identity key `{}` is `{}`, not `int`",
            key.name,
            key.ty.text.trim()
        ),
        keys => format!("it has a composite identity of {} keys", keys.len()),
    }
}

/// One greater than the highest existing integer key, or a typed overflow when
/// the key space is exhausted (`highest == i64::MAX`). Shared by [`next_id`] and
/// [`next_layer_pos`]; the rest of the runtime is uniformly `checked_*`.
fn next_after(highest: i64) -> Result<i64, WriteError> {
    highest.checked_add(1).ok_or_else(|| WriteError {
        code: WRITE_ID_OVERFLOW,
        message: "the integer key space is exhausted; the highest key is i64::MAX".into(),
    })
}

/// The next 1-based position for an `append` to a keyed layer: one greater than
/// the highest populated positive integer key under `^root(identity).layer`, or
/// `1` when the layer is empty. Appending writes after the highest key and never
/// fills holes (docs/language `resources-and-storage.md`); non-integer and
/// non-positive keys are ignored. This is the append policy for sequence-shaped
/// (integer-keyed) layers, the analogue of [`next_id`] for a root.
pub fn next_layer_pos(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    layer: &str,
    store: &dyn Backend,
) -> Result<i64, WriteError> {
    let root = resolve_saved_root(schema, identity)?;
    if !schema.layers.iter().any(|declared| declared.name == layer) {
        return Err(WriteError {
            code: WRITE_UNKNOWN_LAYER,
            message: format!("resource `{}` has no keyed layer `{layer}`", schema.name),
        });
    }
    let mut prefix = identity_path(root, identity);
    prefix.push(PathSegment::ChildLayer(layer.into()));
    let children = store
        .child_keys(&encode_path(&prefix))
        .map_err(|_| WriteError {
            code: WRITE_STORE,
            message: format!("could not read entries under keyed layer `{layer}`"),
        })?;
    let highest = children
        .iter()
        .filter_map(|child| match child {
            ChildSegment::Key(SavedKey::Int(pos)) if *pos >= 1 => Some(*pos),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    next_after(highest)
}

/// The supplied value for `field` in `value`, if any.
fn supplied_value<'a>(value: &'a ResourceValue, field: &str) -> Option<&'a FieldValue> {
    value
        .fields
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| value)
}

/// The encoded-path segments for `^root(identity)`.
fn identity_path(root: &SavedRootSchema, identity: &[SavedKey]) -> Vec<PathSegment> {
    let mut path = vec![PathSegment::Root(root.root.clone())];
    path.extend(identity.iter().cloned().map(PathSegment::RecordKey));
    path
}

/// The encoded-path segments for `^root(identity).field`.
fn field_path(root: &SavedRootSchema, identity: &[SavedKey], field: &str) -> Vec<PathSegment> {
    let mut path = identity_path(root, identity);
    path.push(PathSegment::Field(field.into()));
    path
}

/// The encoded-path segments for a keyed-leaf entry,
/// `^root(identity).layer(key…)`.
fn layer_leaf_path(
    root: &SavedRootSchema,
    identity: &[SavedKey],
    layer: &str,
    key: &[SavedKey],
) -> Vec<PathSegment> {
    nested_layer_path(root, identity, &[(layer, key)])
}

/// The encoded-path segments for a (possibly nested) keyed group entry,
/// `^root(identity).layer0(key0…).layer1(key1…)…`, appending a `ChildLayer` and
/// its `IndexKey`s for each level of the chain.
fn nested_layer_path(
    root: &SavedRootSchema,
    identity: &[SavedKey],
    layers: &[(&str, &[SavedKey])],
) -> Vec<PathSegment> {
    let mut path = identity_path(root, identity);
    for (layer, key) in layers {
        path.push(PathSegment::ChildLayer((*layer).into()));
        path.extend(key.iter().cloned().map(PathSegment::IndexKey));
    }
    path
}

/// Descend a non-empty chain of keyed group layers, validating that each level
/// names a group (not a leaf) with the right key arity, and return the innermost
/// layer's schema. Level 0 is a direct layer of the resource; each deeper level
/// is a nested layer of the one before it.
fn descend_group_layers<'a>(
    schema: &'a ResourceSchema,
    layers: &[(&str, &[SavedKey])],
) -> Result<&'a LayerSchema, WriteError> {
    let mut current: Option<&LayerSchema> = None;
    for (name, key) in layers {
        let declared = match current {
            None => schema
                .layers
                .iter()
                .find(|layer| &layer.name == name)
                .ok_or_else(|| WriteError {
                    code: WRITE_UNKNOWN_LAYER,
                    message: format!("resource `{}` has no keyed layer `{name}`", schema.name),
                })?,
            Some(parent) => parent
                .members
                .iter()
                .find_map(|member| match member {
                    LayerMember::Layer(layer) if &layer.name == name => Some(layer),
                    _ => None,
                })
                .ok_or_else(|| WriteError {
                    code: WRITE_UNKNOWN_LAYER,
                    message: format!("keyed layer `{}` has no nested layer `{name}`", parent.name),
                })?,
        };
        if declared.leaf_type.is_some() {
            return Err(WriteError {
                code: WRITE_NOT_A_GROUP_LAYER,
                message: format!("keyed layer `{name}` is a leaf, not a group"),
            });
        }
        if key.len() != declared.key_params.len() {
            return Err(WriteError {
                code: WRITE_LAYER_KEY_ARITY,
                message: format!(
                    "keyed layer `{name}` expects {} key(s), got {}",
                    declared.key_params.len(),
                    key.len()
                ),
            });
        }
        current = Some(declared);
    }
    current.ok_or_else(|| WriteError {
        code: WRITE_UNKNOWN_LAYER,
        message: format!(
            "resource `{}` field write needs at least one keyed layer",
            schema.name
        ),
    })
}

/// The encoded-path segments for a group-entry field,
/// `^root(identity).layer(key…).field`.
fn layer_field_path(
    root: &SavedRootSchema,
    identity: &[SavedKey],
    layer: &str,
    key: &[SavedKey],
    field: &str,
) -> Vec<PathSegment> {
    let mut path = layer_leaf_path(root, identity, layer, key);
    path.push(PathSegment::Field(field.into()));
    path
}

/// The marker stored at a non-unique index entry. A non-unique entry records
/// only presence; the resource itself remains the place to read fields.
const INDEX_MARKER: &[u8] = b"1";

/// Resolve an index's argument names to key values from the resource being
/// written: an argument naming an identity key takes that key; one naming a
/// top-level field takes that field's value. Returns `None` when any argument is
/// absent or is a type with no key encoding, so no entry is written.
fn index_keys(
    args: &[String],
    root: &SavedRootSchema,
    identity: &[SavedKey],
    value: &ResourceValue,
) -> Option<Vec<SavedKey>> {
    let mut keys = Vec::with_capacity(args.len());
    for arg in args {
        if let Some(position) = root.identity_keys.iter().position(|key| &key.name == arg) {
            keys.push(identity[position].clone());
        } else {
            match value.fields.iter().find(|(name, _)| name == arg) {
                Some((_, FieldValue::Saved(saved))) => keys.push(saved_value_to_key(saved)?),
                _ => return None,
            }
        }
    }
    Some(keys)
}

/// Resolve a single index argument to the key value currently STORED for this
/// identity: an identity-key argument takes the identity; a field argument reads
/// and decodes the stored field. Returns `None` if the field is absent, has no
/// key encoding, or does not decode.
fn stored_arg_key(
    arg: &str,
    root: &SavedRootSchema,
    identity: &[SavedKey],
    schema: &ResourceSchema,
    store: &dyn Backend,
) -> Result<Option<SavedKey>, StoreError> {
    if let Some(position) = root.identity_keys.iter().position(|key| key.name == arg) {
        return Ok(Some(identity[position].clone()));
    }
    let Some(field) = schema.fields.iter().find(|field| field.name == arg) else {
        return Ok(None);
    };
    let Some(value_type) = ValueType::from_scalar_name(&field.ty.text) else {
        return Ok(None);
    };
    let Some(bytes) = store.read(&encode_path(&field_path(root, identity, arg)))? else {
        return Ok(None);
    };
    Ok(decode_value(&bytes, value_type).and_then(|value| saved_value_to_key(&value)))
}

/// Resolve an index's argument names to the key values currently STORED for this
/// identity, for index teardown. Returns `None` if any argument is absent or
/// undecodable, so nothing is torn down for it.
fn stored_index_keys(
    args: &[String],
    root: &SavedRootSchema,
    identity: &[SavedKey],
    schema: &ResourceSchema,
    store: &dyn Backend,
) -> Result<Option<Vec<SavedKey>>, StoreError> {
    args.iter()
        .map(|arg| stored_arg_key(arg, root, identity, schema, store))
        .collect()
}

/// Resolve an index's argument names to the key values AFTER a field write: the
/// written `field` takes its new `value`; every other argument keeps its stored
/// value (`stored_arg_key`). Returns `None` if any argument is absent or has no
/// key encoding, so no entry is written.
fn field_write_index_keys(
    args: &[String],
    root: &SavedRootSchema,
    identity: &[SavedKey],
    field: &str,
    value: &SavedValue,
    schema: &ResourceSchema,
    store: &dyn Backend,
) -> Result<Option<Vec<SavedKey>>, StoreError> {
    args.iter()
        .map(|arg| {
            if arg == field {
                Ok(saved_value_to_key(value))
            } else {
                stored_arg_key(arg, root, identity, schema, store)
            }
        })
        .collect()
}

/// Resolve an index's argument names to the key values of the EFFECTIVE resource
/// after a merge: a field argument the merge supplies takes that value; any
/// other argument (an identity key, or a field the merge leaves untouched) keeps
/// its stored value. Returns `None` if any argument is absent or has no key
/// encoding, so no entry is written.
fn effective_index_keys(
    args: &[String],
    root: &SavedRootSchema,
    identity: &[SavedKey],
    value: &ResourceValue,
    schema: &ResourceSchema,
    store: &dyn Backend,
) -> Result<Option<Vec<SavedKey>>, StoreError> {
    args.iter()
        .map(|arg| match supplied_value(value, arg) {
            Some(FieldValue::Saved(saved)) => Ok(saved_value_to_key(saved)),
            _ => stored_arg_key(arg, root, identity, schema, store),
        })
        .collect()
}

/// The encoded-path segments for an index entry, `^root.index(keys...)`.
fn index_path(root: &SavedRootSchema, index: &str, keys: &[SavedKey]) -> Vec<PathSegment> {
    let mut path = vec![
        PathSegment::Root(root.root.clone()),
        PathSegment::Index(index.into()),
    ];
    path.extend(keys.iter().cloned().map(PathSegment::IndexKey));
    path
}

/// The stored value of an index entry: a unique entry stores the owning identity
/// (so a lookup yields the record and a re-write can tell itself from a clash); a
/// non-unique entry stores only a presence marker.
fn index_entry_value(unique: bool, identity: &[SavedKey]) -> Vec<u8> {
    if unique {
        encode_identity(identity)
    } else {
        INDEX_MARKER.to_vec()
    }
}

/// Encode a resource identity as the value of a unique index entry: its keys in
/// identity order, self-delimiting so the run decodes back exactly.
fn encode_identity(identity: &[SavedKey]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for key in identity {
        bytes.extend_from_slice(&encode_key_value(key));
    }
    bytes
}

/// Decode a unique index entry's value back to the identity it points to, given
/// the saved root's identity arity. Returns `None` unless the bytes are exactly
/// that many well-formed keys with nothing left over. The runtime reads it back
/// when a unique-index lookup is used in value position.
pub fn decode_identity(bytes: &[u8], root: &SavedRootSchema) -> Option<Vec<SavedKey>> {
    let mut keys = Vec::with_capacity(root.identity_keys.len());
    let mut rest = bytes;
    for _ in 0..root.identity_keys.len() {
        let (key, used) = decode_key_value(rest)?;
        keys.push(key);
        rest = rest.get(used..)?;
    }
    rest.is_empty().then_some(keys)
}

/// Reject a write when a unique index would map `new_keys` to a resource other
/// than `identity`. `new_keys` is `None` when the entry would not exist (an
/// indexed value is absent), which never conflicts. An entry held by `identity`
/// itself is not a conflict (a re-write of its own record); an unreadable entry
/// is a store error, since a real clash cannot be ruled out.
fn check_unique_conflict(
    index: &str,
    root: &SavedRootSchema,
    identity: &[SavedKey],
    new_keys: Option<&[SavedKey]>,
    store: &dyn Backend,
) -> Result<(), WriteError> {
    let Some(new_keys) = new_keys else {
        return Ok(());
    };
    let stored = store
        .read(&encode_path(&index_path(root, index, new_keys)))
        .map_err(store_failed)?;
    let Some(bytes) = stored else {
        return Ok(());
    };
    match decode_identity(&bytes, root) {
        Some(holder) if holder == identity => Ok(()),
        Some(_) => Err(WriteError {
            code: WRITE_UNIQUE_CONFLICT,
            message: format!(
                "unique index `{index}` already holds those key(s) for another resource"
            ),
        }),
        None => Err(WriteError {
            code: WRITE_STORE,
            message: format!("unique index `{index}` has an unreadable entry"),
        }),
    }
}

/// The key form of a saved value, or `None` for a value with no order-preserving
/// key encoding (decimal, for now).
fn saved_value_to_key(value: &SavedValue) -> Option<SavedKey> {
    Some(match value {
        SavedValue::Int(value) => SavedKey::Int(*value),
        SavedValue::Bool(value) => SavedKey::Bool(*value),
        SavedValue::Str(value) | SavedValue::ErrorCode(value) => SavedKey::Str(value.clone()),
        SavedValue::Bytes(value) => SavedKey::Bytes(value.clone()),
        SavedValue::Date(value) => SavedKey::Date(*value),
        SavedValue::Duration(value) => SavedKey::Duration(*value),
        SavedValue::Instant(value) => SavedKey::Instant(*value),
        SavedValue::Decimal { .. } => return None,
    })
}

/// Check that `value` matches the field's declared scalar type name.
fn check_type(field: &str, type_name: &str, value: &SavedValue) -> Result<(), WriteError> {
    if ValueType::from_scalar_name(type_name) == Some(value_type_of(value)) {
        Ok(())
    } else {
        Err(WriteError {
            code: WRITE_TYPE_MISMATCH,
            message: format!("field `{field}` does not hold a `{type_name}`"),
        })
    }
}

/// The [`ValueType`] of a saved value.
fn value_type_of(value: &SavedValue) -> ValueType {
    match value {
        SavedValue::Bool(_) => ValueType::Bool,
        SavedValue::Int(_) => ValueType::Int,
        SavedValue::Str(_) => ValueType::Str,
        SavedValue::Bytes(_) => ValueType::Bytes,
        SavedValue::ErrorCode(_) => ValueType::ErrorCode,
        SavedValue::Date(_) => ValueType::Date,
        SavedValue::Duration(_) => ValueType::Duration,
        SavedValue::Instant(_) => ValueType::Instant,
        SavedValue::Decimal { .. } => ValueType::Decimal,
    }
}
