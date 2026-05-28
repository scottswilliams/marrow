//! Managed writes over the saved store.
//!
//! This layer composes the typed tree shape from `marrow-schema` with the
//! ordered-bytes store from `marrow-store`. A managed write is planned in full —
//! validated against the schema and lowered to encoded paths — before any change
//! is visible, so a rejected write leaves the store untouched and a committed
//! one is internally coherent.
//!
//! This first slice covers a whole-resource write of a resource's top-level
//! fields; keyed layers, indexes, field writes, delete, and merge build on it.

use marrow_schema::{ResourceSchema, SavedRootSchema};
use marrow_store::mem::MemStore;
use marrow_store::path::{ChildSegment, PathSegment, SavedKey, encode_path};
use marrow_store::value::{SavedValue, ValueType, decode_value, encode_value};

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

/// One staged store operation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PlanStep {
    Write { path: Vec<u8>, value: Vec<u8> },
    Delete { path: Vec<u8> },
}

/// A staged, validated set of store operations. Apply it with
/// [`WritePlan::commit`]; drop it to abandon the write with no effect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WritePlan {
    steps: Vec<PlanStep>,
}

impl WritePlan {
    /// Apply the staged operations to `store`, in order.
    pub fn commit(self, store: &mut MemStore) {
        for step in self.steps {
            match step {
                PlanStep::Write { path, value } => store.write(&path, value),
                PlanStep::Delete { path } => store.delete(&path),
            }
        }
    }
}

/// Plan a whole-resource write: replace the resource at `identity` with `value`.
/// Validates required fields and value types against `schema` before staging
/// anything, then plans to clear the old subtree, write each present field, and
/// keep generated non-unique index entries coherent (delete the entries for the
/// currently-stored values, write entries for the new values). `store` is read,
/// not written; apply the returned [`WritePlan`] with [`WritePlan::commit`].
/// Returns a [`WriteError`] if the value does not satisfy the schema.
/// (Unique-index conflict detection is a later slice.)
pub fn plan_resource_write(
    schema: &ResourceSchema,
    identity: &[SavedKey],
    value: &ResourceValue,
    store: &MemStore,
) -> Result<WritePlan, WriteError> {
    let root = schema.saved_root.as_ref().ok_or_else(|| WriteError {
        code: WRITE_NO_SAVED_ROOT,
        message: format!("resource `{}` has no saved root", schema.name),
    })?;
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

    // Replace semantics: clear the old subtree, then write the present fields.
    let mut steps = vec![PlanStep::Delete {
        path: encode_path(&identity_path(root, identity)),
    }];
    for (name, saved) in to_write {
        steps.push(PlanStep::Write {
            path: encode_path(&field_path(root, identity, name)),
            value: encode_value(saved),
        });
    }

    // Keep generated non-unique index entries coherent: delete the entry for the
    // currently-stored values, then write the entry for the new values. An entry
    // exists only when every indexed value is populated. (Unique indexes with
    // conflict detection are a later slice.)
    for index in &schema.indexes {
        if index.unique {
            continue;
        }
        if let Some(old_keys) = stored_index_keys(&index.args, root, identity, schema, store) {
            steps.push(PlanStep::Delete {
                path: encode_path(&index_path(root, &index.name, &old_keys)),
            });
        }
        if let Some(new_keys) = index_keys(&index.args, root, identity, value) {
            steps.push(PlanStep::Write {
                path: encode_path(&index_path(root, &index.name, &new_keys)),
                value: INDEX_MARKER.to_vec(),
            });
        }
    }
    Ok(WritePlan { steps })
}

/// The next identity for a single-`int` keyed saved root: one greater than the
/// highest existing integer record key, or `1` when the root is empty. This is
/// the default `nextId` policy (docs/implementation.md). Non-integer immediate
/// children — such as index names — are ignored.
pub fn next_id(root: &str, store: &MemStore) -> Result<i64, WriteError> {
    let children = store
        .child_keys(&encode_path(&[PathSegment::Root(root.into())]))
        .map_err(|_| WriteError {
            code: WRITE_STORE,
            message: format!("could not read records under `^{root}`"),
        })?;
    let highest = children
        .iter()
        .filter_map(|child| match child {
            ChildSegment::Key(SavedKey::Int(value)) => Some(*value),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    Ok(highest + 1)
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

/// Resolve an index's argument names to the key values currently STORED for this
/// identity, for index teardown: identity-key args take the identity; field args
/// read and decode the stored field. Returns `None` if any field is absent or
/// undecodable, so nothing is torn down for it.
fn stored_index_keys(
    args: &[String],
    root: &SavedRootSchema,
    identity: &[SavedKey],
    schema: &ResourceSchema,
    store: &MemStore,
) -> Option<Vec<SavedKey>> {
    let mut keys = Vec::with_capacity(args.len());
    for arg in args {
        if let Some(position) = root.identity_keys.iter().position(|key| &key.name == arg) {
            keys.push(identity[position].clone());
        } else {
            let field = schema.fields.iter().find(|field| &field.name == arg)?;
            let value_type = value_type_for(&field.ty.text)?;
            let bytes = store.read(&encode_path(&field_path(root, identity, arg)))?;
            keys.push(saved_value_to_key(&decode_value(bytes, value_type)?)?);
        }
    }
    Some(keys)
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
    if value_type_for(type_name) == Some(value_type_of(value)) {
        Ok(())
    } else {
        Err(WriteError {
            code: WRITE_TYPE_MISMATCH,
            message: format!("field `{field}` does not hold a `{type_name}`"),
        })
    }
}

/// The [`ValueType`] a scalar type name denotes, or `None` for identity and
/// other non-scalar types (which this slice does not write as plain fields).
fn value_type_for(type_name: &str) -> Option<ValueType> {
    Some(match type_name {
        "bool" => ValueType::Bool,
        "int" => ValueType::Int,
        "string" => ValueType::Str,
        "bytes" => ValueType::Bytes,
        "ErrorCode" => ValueType::ErrorCode,
        "date" => ValueType::Date,
        "instant" => ValueType::Instant,
        "duration" => ValueType::Duration,
        "decimal" => ValueType::Decimal,
        _ => return None,
    })
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
