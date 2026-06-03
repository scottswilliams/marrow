//! Generated-index maintenance for managed resource writes.

use marrow_check::{
    CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedPlace, StoreIndexKeySource,
    StoredValueMeaning,
};
use marrow_store::key::{SavedKey, decode_key_value, encode_key_value};
use marrow_store::tree::{TreeStore, decode_tree_enum_member};
use marrow_store::value::decode_value;
use marrow_syntax::SourceSpan;

use crate::store::{DataAddress, IndexAddress, read_data};
use crate::value::LeafValue;
use crate::write::{ResourceValue, WRITE_STORE, WRITE_UNIQUE_CONFLICT, WriteError};
use crate::write_plan::PlanStep;

const INDEX_MARKER: &[u8] = b"1";

pub(crate) fn reject_resource_unique_conflicts(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    value: &ResourceValue,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    for index in &place.indexes {
        if index.unique {
            let new_keys = index_keys(&index.keys, place, identity, value);
            check_unique_conflict(index, identity, new_keys.as_deref(), store, span)?;
        }
    }
    Ok(())
}

pub(crate) fn stage_resource_index_rewrites(
    steps: &mut Vec<PlanStep>,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    value: &ResourceValue,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    for index in &place.indexes {
        if let Some(old_keys) = stored_index_keys(&index.keys, place, identity, store, span)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, span)?,
                identity: identity.to_vec(),
            });
        }
        if let Some(new_keys) = index_keys(&index.keys, place, identity, value) {
            steps.push(PlanStep::WriteIndex {
                address: index_address(index, new_keys, span)?,
                identity: identity.to_vec(),
                value: index_entry_value(index.unique, identity),
            });
        }
    }
    Ok(())
}

pub(crate) fn stage_resource_index_deletes(
    steps: &mut Vec<PlanStep>,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    for index in &place.indexes {
        if let Some(keys) = stored_index_keys(&index.keys, place, identity, store, span)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, keys, span)?,
                identity: identity.to_vec(),
            });
        }
    }
    Ok(())
}

pub(crate) fn reject_field_unique_conflicts(
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    field: &str,
    value: &LeafValue,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    for index in &place.indexes {
        if index.unique && index.keys.iter().any(|key| key.name == field) {
            let new_keys = field_write_index_keys(FieldWriteIndexKeys {
                keys: &index.keys,
                place,
                identity,
                field,
                value,
                store,
                span,
            })?;
            check_unique_conflict(index, identity, new_keys.as_deref(), store, span)?;
        }
    }
    Ok(())
}

pub(crate) struct FieldIndexRewrite<'a> {
    pub(crate) place: &'a CheckedSavedPlace,
    pub(crate) identity: &'a [SavedKey],
    pub(crate) field: &'a str,
    pub(crate) value: &'a LeafValue,
    pub(crate) store: &'a TreeStore,
    pub(crate) span: SourceSpan,
}

pub(crate) fn stage_field_index_rewrites(
    steps: &mut Vec<PlanStep>,
    rewrite: FieldIndexRewrite<'_>,
) -> Result<(), WriteError> {
    for index in &rewrite.place.indexes {
        if !index.keys.iter().any(|key| key.name == rewrite.field) {
            continue;
        }
        if let Some(old_keys) = stored_index_keys(
            &index.keys,
            rewrite.place,
            rewrite.identity,
            rewrite.store,
            rewrite.span,
        )? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, rewrite.span)?,
                identity: rewrite.identity.to_vec(),
            });
        }
        if let Some(new_keys) = field_write_index_keys(FieldWriteIndexKeys {
            keys: &index.keys,
            place: rewrite.place,
            identity: rewrite.identity,
            field: rewrite.field,
            value: rewrite.value,
            store: rewrite.store,
            span: rewrite.span,
        })? {
            steps.push(PlanStep::WriteIndex {
                address: index_address(index, new_keys, rewrite.span)?,
                identity: rewrite.identity.to_vec(),
                value: index_entry_value(index.unique, rewrite.identity),
            });
        }
    }
    Ok(())
}

pub(crate) fn stage_field_index_deletes(
    steps: &mut Vec<PlanStep>,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    field: &str,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    for index in &place.indexes {
        if !index.keys.iter().any(|key| key.name == field) {
            continue;
        }
        if let Some(old_keys) = stored_index_keys(&index.keys, place, identity, store, span)? {
            steps.push(PlanStep::DeleteIndex {
                address: index_address(index, old_keys, span)?,
                identity: identity.to_vec(),
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
) -> Option<Vec<SavedKey>> {
    let mut result = Vec::with_capacity(keys.len());
    for key in keys {
        match key.source {
            StoreIndexKeySource::IdentityKey => {
                let position = place
                    .identity_keys
                    .iter()
                    .position(|identity_key| identity_key.name == key.name)?;
                result.push(identity[position].clone());
            }
            StoreIndexKeySource::ResourceMember(_) => {
                let (_, saved) = value.fields.iter().find(|(name, _)| name == &key.name)?;
                result.push(saved.as_key()?);
            }
        }
    }
    Some(result)
}

fn stored_arg_key(
    key: &CheckedSavedIndexKey,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
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
            let Some(bytes) = read_data(store, &address, span).map_err(runtime_store_error)? else {
                return Ok(None);
            };
            Ok(stored_field_key(&key.value_meaning, &bytes))
        }
    }
}

fn stored_field_key(meaning: &StoredValueMeaning, bytes: &[u8]) -> Option<SavedKey> {
    match meaning {
        StoredValueMeaning::Enum { .. } => decode_tree_enum_member(bytes)
            .ok()
            .map(|member| SavedKey::Str(member.member_id().as_str().to_string())),
        StoredValueMeaning::Scalar(scalar) => {
            decode_value(bytes, *scalar).and_then(|value| value.as_key())
        }
        StoredValueMeaning::Identity(_) => None,
    }
}

fn stored_index_keys(
    keys: &[CheckedSavedIndexKey],
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    store: &TreeStore,
    span: SourceSpan,
) -> Result<Option<Vec<SavedKey>>, WriteError> {
    keys.iter()
        .map(|key| stored_arg_key(key, place, identity, store, span))
        .collect()
}

struct FieldWriteIndexKeys<'a> {
    keys: &'a [CheckedSavedIndexKey],
    place: &'a CheckedSavedPlace,
    identity: &'a [SavedKey],
    field: &'a str,
    value: &'a LeafValue,
    store: &'a TreeStore,
    span: SourceSpan,
}

fn field_write_index_keys(
    input: FieldWriteIndexKeys<'_>,
) -> Result<Option<Vec<SavedKey>>, WriteError> {
    input
        .keys
        .iter()
        .map(|key| {
            if key.name == input.field {
                Ok(input.value.as_key())
            } else {
                stored_arg_key(key, input.place, input.identity, input.store, input.span)
            }
        })
        .collect()
}

fn index_address(
    index: &CheckedSavedIndex,
    keys: Vec<SavedKey>,
    span: SourceSpan,
) -> Result<IndexAddress, WriteError> {
    IndexAddress::new(&index.catalog_id, keys, span).map_err(runtime_store_error)
}

fn index_entry_value(unique: bool, identity: &[SavedKey]) -> Vec<u8> {
    if unique {
        encode_identity(identity)
    } else {
        INDEX_MARKER.to_vec()
    }
}

pub(crate) fn encode_identity(identity: &[SavedKey]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for key in identity {
        bytes.extend_from_slice(&encode_key_value(key));
    }
    bytes
}

pub fn decode_identity_arity(bytes: &[u8], arity: usize) -> Option<Vec<SavedKey>> {
    let mut keys = Vec::with_capacity(arity);
    let mut rest = bytes;
    for _ in 0..arity {
        let (key, used) = decode_key_value(rest)?;
        keys.push(key);
        rest = rest.get(used..)?;
    }
    rest.is_empty().then_some(keys)
}

fn check_unique_conflict(
    index: &CheckedSavedIndex,
    identity: &[SavedKey],
    new_keys: Option<&[SavedKey]>,
    store: &TreeStore,
    span: SourceSpan,
) -> Result<(), WriteError> {
    let Some(new_keys) = new_keys else {
        return Ok(());
    };
    let address = IndexAddress::new(&index.catalog_id, new_keys.to_vec(), span)
        .map_err(runtime_store_error)?;
    let page = store
        .scan_index_tuple(&address.index, &address.keys, 2)
        .map_err(WriteError::from)?;
    if page.entries.iter().any(|entry| entry.identity != identity) {
        return Err(WriteError {
            code: WRITE_UNIQUE_CONFLICT,
            message: format!(
                "unique index `{}` already holds those key(s) for another identity",
                index.name
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
