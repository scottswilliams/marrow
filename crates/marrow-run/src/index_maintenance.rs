//! Generated-index maintenance for managed resource writes.

use marrow_check::{
    CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedPlace, StoreIndexKeySource,
    StoredValueMeaning,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_syntax::SourceSpan;

use crate::store::{DataAddress, IndexAddress, read_data};
use crate::value::{LeafValue, diagnostic_saved_key_tuple_preview};
use crate::write::{ResourceValue, WRITE_STORE, WRITE_UNIQUE_CONFLICT, WriteError};
use crate::write_plan::PlanStep;

const INDEX_MARKER: &[u8] = b"1";

pub(crate) trait StagedDataView {
    fn staged_data_value(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Option<&[u8]>;
}

pub(crate) struct EmptyStagedData;

impl StagedDataView for EmptyStagedData {
    fn staged_data_value(
        &self,
        _store: &CatalogId,
        _identity: &[SavedKey],
        _path: &[DataPathSegment],
    ) -> Option<&[u8]> {
        None
    }
}

#[derive(Clone, Copy)]
pub(crate) struct IndexWriteContext<'a> {
    place: &'a CheckedSavedPlace,
    identity: &'a [SavedKey],
    store: &'a TreeStore,
    span: SourceSpan,
}

impl<'a> IndexWriteContext<'a> {
    pub(crate) fn new(
        place: &'a CheckedSavedPlace,
        identity: &'a [SavedKey],
        store: &'a TreeStore,
        span: SourceSpan,
    ) -> Self {
        Self {
            place,
            identity,
            store,
            span,
        }
    }
}

pub(crate) fn reject_resource_unique_conflicts(
    context: IndexWriteContext<'_>,
    value: &ResourceValue,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if index.unique {
            let new_keys = index_keys(&index.keys, context.place, context.identity, value)?;
            check_unique_conflict(index, context, new_keys.as_deref())?;
        }
    }
    Ok(())
}

pub(crate) fn stage_resource_index_rewrites(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    value: &ResourceValue,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if let Some(old_keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
        if let Some(new_keys) = index_keys(&index.keys, context.place, context.identity, value)? {
            steps.push(PlanStep::WriteIndex {
                address: index_address(index, new_keys, context.span)?,
                identity: context.identity.to_vec(),
                value: index_entry_value(index.unique, context.identity),
            });
        }
    }
    Ok(())
}

pub(crate) fn index_rebuild_entry_with_staged(
    index: &CheckedSavedIndex,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
    staged: &dyn StagedDataView,
    span: SourceSpan,
) -> Result<Option<PlanStep>, WriteError> {
    let Some(keys) =
        stored_index_keys_with_staged(&index.keys, place, identity, store, staged, span)?
    else {
        return Ok(None);
    };
    Ok(Some(PlanStep::WriteIndex {
        address: index_address(index, keys, span)?,
        identity: identity.to_vec(),
        value: index_entry_value(index.unique, identity),
    }))
}

pub(crate) fn stage_resource_index_deletes(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if let Some(keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
    }
    Ok(())
}

pub(crate) fn reject_field_unique_conflicts(
    context: IndexWriteContext<'_>,
    field: &str,
    value: &LeafValue,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if index.unique && index.keys.iter().any(|key| key.name == field) {
            let new_keys =
                field_write_index_keys(&index.keys, context, field, FieldIndexValue::Leaf(value))?;
            check_unique_conflict(index, context, new_keys.as_deref())?;
        }
    }
    Ok(())
}

pub(crate) fn reject_identity_field_unique_conflicts(
    context: IndexWriteContext<'_>,
    field: &str,
    keys: &[SavedKey],
) -> Result<(), WriteError> {
    let bytes = encode_identity_payload(keys);
    for index in &context.place.indexes {
        if index.unique && index.keys.iter().any(|key| key.name == field) {
            let new_keys = field_write_index_keys(
                &index.keys,
                context,
                field,
                FieldIndexValue::IdentityBytes(&bytes),
            )?;
            check_unique_conflict(index, context, new_keys.as_deref())?;
        }
    }
    Ok(())
}

pub(crate) fn stage_field_index_rewrites(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    field: &str,
    value: &LeafValue,
) -> Result<(), WriteError> {
    stage_field_index_rewrites_for_value(steps, context, field, FieldIndexValue::Leaf(value))
}

pub(crate) fn stage_identity_field_index_rewrites(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    field: &str,
    keys: &[SavedKey],
) -> Result<(), WriteError> {
    let bytes = encode_identity_payload(keys);
    stage_field_index_rewrites_for_value(
        steps,
        context,
        field,
        FieldIndexValue::IdentityBytes(&bytes),
    )
}

pub(crate) fn stage_field_index_deletes(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    field: &str,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if !index.keys.iter().any(|key| key.name == field) {
            continue;
        }
        if let Some(old_keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
    }
    Ok(())
}

fn stage_field_index_rewrites_for_value(
    steps: &mut Vec<PlanStep>,
    context: IndexWriteContext<'_>,
    field: &str,
    value: FieldIndexValue<'_>,
) -> Result<(), WriteError> {
    for index in &context.place.indexes {
        if !index.keys.iter().any(|key| key.name == field) {
            continue;
        }
        if let Some(old_keys) = stored_index_keys(&index.keys, context)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, context.span)?,
                identity: context.identity.to_vec(),
            });
        }
        if let Some(new_keys) = field_write_index_keys(&index.keys, context, field, value)? {
            steps.push(PlanStep::WriteIndex {
                address: index_address(index, new_keys, context.span)?,
                identity: context.identity.to_vec(),
                value: index_entry_value(index.unique, context.identity),
            });
        }
    }
    Ok(())
}

fn index_keys(
    keys: &[CheckedSavedIndexKey],
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    value: &ResourceValue,
) -> Result<Option<Vec<SavedKey>>, WriteError> {
    let mut result = Vec::with_capacity(keys.len());
    for key in keys {
        match key.source {
            StoreIndexKeySource::IdentityKey => {
                let Some(position) = place
                    .identity_keys
                    .iter()
                    .position(|identity_key| identity_key.name == key.name)
                else {
                    return Ok(None);
                };
                result.push(identity[position].clone());
            }
            StoreIndexKeySource::ResourceMember(_) => {
                if let Some((_, saved)) = value.fields.iter().find(|(name, _)| name == &key.name) {
                    let Some(key) = saved.as_key()? else {
                        return Ok(None);
                    };
                    result.push(key);
                } else {
                    let Some(supplied) = value
                        .identities
                        .iter()
                        .find(|supplied| supplied.field == key.name)
                    else {
                        return Ok(None);
                    };
                    let bytes = encode_identity_payload(&supplied.keys);
                    let Some(key) = key.value_meaning.stored_key(&bytes) else {
                        return Ok(None);
                    };
                    result.push(key);
                }
            }
        }
    }
    Ok(Some(result))
}

fn stored_arg_key(
    key: &CheckedSavedIndexKey,
    context: IndexWriteContext<'_>,
) -> Result<Option<SavedKey>, WriteError> {
    stored_arg_key_with_staged(
        key,
        context.place,
        context.identity,
        context.store,
        &EmptyStagedData,
        context.span,
    )
}

fn stored_arg_key_with_staged(
    key: &CheckedSavedIndexKey,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
    staged: &dyn StagedDataView,
    span: SourceSpan,
) -> Result<Option<SavedKey>, WriteError> {
    match key.source {
        StoreIndexKeySource::IdentityKey => {
            let Some(position) = place
                .identity_keys
                .iter()
                .position(|identity_key| identity_key.name == key.name)
            else {
                return Ok(None);
            };
            Ok(Some(identity[position].clone()))
        }
        StoreIndexKeySource::ResourceMember(_) => {
            let field_path = vec![key.name.clone()];
            let address = DataAddress::member_path(place, identity, &[], &field_path, span)
                .map_err(runtime_store_error)?;
            if let Some(bytes) =
                staged.staged_data_value(&address.store, &address.identity, &address.path)
            {
                return stored_index_key(&key.value_meaning, bytes).map(Some);
            }
            let Some(bytes) = read_data(store, &address, span).map_err(runtime_store_error)? else {
                return Ok(None);
            };
            stored_index_key(&key.value_meaning, &bytes).map(Some)
        }
    }
}

fn stored_index_keys(
    keys: &[CheckedSavedIndexKey],
    context: IndexWriteContext<'_>,
) -> Result<Option<Vec<SavedKey>>, WriteError> {
    stored_index_keys_with_staged(
        keys,
        context.place,
        context.identity,
        context.store,
        &EmptyStagedData,
        context.span,
    )
}

fn stored_index_keys_with_staged(
    keys: &[CheckedSavedIndexKey],
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
    staged: &dyn StagedDataView,
    span: SourceSpan,
) -> Result<Option<Vec<SavedKey>>, WriteError> {
    keys.iter()
        .map(|key| stored_arg_key_with_staged(key, place, identity, store, staged, span))
        .collect()
}

#[derive(Clone, Copy)]
enum FieldIndexValue<'a> {
    Leaf(&'a LeafValue),
    IdentityBytes(&'a [u8]),
}

impl FieldIndexValue<'_> {
    fn key_for(self, key: &CheckedSavedIndexKey) -> Result<Option<SavedKey>, WriteError> {
        match self {
            Self::Leaf(value) => value.as_key().map_err(WriteError::from),
            Self::IdentityBytes(bytes) => stored_index_key(&key.value_meaning, bytes).map(Some),
        }
    }
}

fn field_write_index_keys(
    keys: &[CheckedSavedIndexKey],
    context: IndexWriteContext<'_>,
    field: &str,
    value: FieldIndexValue<'_>,
) -> Result<Option<Vec<SavedKey>>, WriteError> {
    keys.iter()
        .map(|key| {
            if key.name == field {
                value.key_for(key)
            } else {
                stored_arg_key(key, context)
            }
        })
        .collect()
}

fn stored_index_key(meaning: &StoredValueMeaning, bytes: &[u8]) -> Result<SavedKey, WriteError> {
    meaning.stored_key(bytes).ok_or_else(|| WriteError {
        code: WRITE_STORE,
        message: "stored indexed value is not valid under its declared type".to_string(),
    })
}

fn index_address(
    index: &CheckedSavedIndex,
    keys: Vec<SavedKey>,
    span: SourceSpan,
) -> Result<IndexAddress, WriteError> {
    IndexAddress::from_checked(&index.catalog_id, keys, span).map_err(runtime_store_error)
}

fn index_entry_value(unique: bool, identity: &[SavedKey]) -> Vec<u8> {
    if unique {
        encode_identity_payload(identity)
    } else {
        INDEX_MARKER.to_vec()
    }
}

fn check_unique_conflict(
    index: &CheckedSavedIndex,
    context: IndexWriteContext<'_>,
    new_keys: Option<&[SavedKey]>,
) -> Result<(), WriteError> {
    let Some(new_keys) = new_keys else {
        return Ok(());
    };
    let address = IndexAddress::from_checked(&index.catalog_id, new_keys.to_vec(), context.span)
        .map_err(runtime_store_error)?;
    let page = context
        .store
        .scan_index_tuple(&address.index, &address.keys, 2)
        .map_err(WriteError::from)?;
    if page
        .entries
        .iter()
        .any(|entry| entry.identity != context.identity)
    {
        return Err(WriteError {
            code: WRITE_UNIQUE_CONFLICT,
            message: format!(
                "unique index `{}` already holds key(s) {} for another identity",
                index.name,
                diagnostic_saved_key_tuple_preview(new_keys)
            ),
        });
    }
    Ok(())
}

fn runtime_store_error(error: crate::error::RuntimeError) -> WriteError {
    WriteError {
        code: WRITE_STORE,
        message: error.message,
    }
}
