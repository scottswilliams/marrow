use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataValuePrefix, TreeStore};

use super::record_nav;
use super::render::render_data_path_value_prefix_preview;
use super::shape::stored_key_mismatch;
use super::{
    DataPresence, DataPreviewReadResult, DataReadResult, DebugDataPayload, ResolvedDataPath,
    clamp_value_preview_limit,
};
use crate::{CheckedProgram, CheckedRuntimeProgram};

pub fn read_data_path(
    store: &TreeStore,
    path: &ResolvedDataPath,
) -> Result<DataReadResult, StoreError> {
    let result = read_data_path_with(store, path, |store, path| {
        store.read_data_value(
            &path.storage.store,
            &path.storage.identity,
            &path.storage.data_path,
        )
    })?;
    Ok(DataReadResult {
        payload: result.value.map(DebugDataPayload::new),
        presence: result.presence,
    })
}

pub fn preview_data_path(
    program: &CheckedProgram,
    store: &TreeStore,
    path: &ResolvedDataPath,
    limit: usize,
) -> Result<DataPreviewReadResult, StoreError> {
    preview_data_path_with_prefix_reader(program, store, path, limit, |store, path, limit| {
        store.read_data_value_prefix(
            &path.storage.store,
            &path.storage.identity,
            &path.storage.data_path,
            limit,
        )
    })
}

pub fn preview_runtime_data_path(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    path: &ResolvedDataPath,
    limit: usize,
) -> Result<DataPreviewReadResult, StoreError> {
    preview_data_path_with_prefix_reader(program, store, path, limit, |store, path, limit| {
        store.read_data_value_prefix(
            &path.storage.store,
            &path.storage.identity,
            &path.storage.data_path,
            limit,
        )
    })
}

fn preview_data_path_with_prefix_reader(
    program: &(impl super::DataProgram + ?Sized),
    store: &TreeStore,
    path: &ResolvedDataPath,
    limit: usize,
    read_prefix: impl FnOnce(
        &TreeStore,
        &ResolvedDataPath,
        usize,
    ) -> Result<Option<DataValuePrefix>, StoreError>,
) -> Result<DataPreviewReadResult, StoreError> {
    let limit = clamp_value_preview_limit(limit);
    let result = read_data_path_with(store, path, |store, path| read_prefix(store, path, limit))?;
    let preview = result.value.as_ref().map(|prefix| {
        render_data_path_value_prefix_preview(program, path, &prefix.bytes, prefix.truncated, limit)
    });
    Ok(DataPreviewReadResult {
        preview,
        presence: result.presence,
    })
}

struct ResolvedDataPathRead<T> {
    value: Option<T>,
    presence: DataPresence,
}

fn read_data_path_with<T>(
    store: &TreeStore,
    path: &ResolvedDataPath,
    read_value: impl FnOnce(&TreeStore, &ResolvedDataPath) -> Result<Option<T>, StoreError>,
) -> Result<ResolvedDataPathRead<T>, StoreError> {
    if path.storage.identity.len() < path.storage.identity_arity {
        return children_presence(record_children_present(store, path)?);
    }
    if path.storage.data_path.is_empty() {
        return children_presence(store.data_subtree_exists(
            &path.storage.store,
            &path.storage.identity,
            &path.storage.data_path,
        )?);
    }
    let value = read_value(store, path)?;
    let presence = if value.is_some() {
        DataPresence::ValueOnly
    } else if data_children_present(store, path)? {
        DataPresence::ChildrenOnly
    } else {
        DataPresence::Absent
    };
    Ok(ResolvedDataPathRead { value, presence })
}

fn children_presence<T>(has_children: bool) -> Result<ResolvedDataPathRead<T>, StoreError> {
    let presence = if has_children {
        DataPresence::ChildrenOnly
    } else {
        DataPresence::Absent
    };
    Ok(ResolvedDataPathRead {
        value: None,
        presence,
    })
}

fn record_children_present(store: &TreeStore, path: &ResolvedDataPath) -> Result<bool, StoreError> {
    let child = record_nav::first_record_child(
        store,
        &path.storage.store,
        &path.storage.identity,
        path.storage.identity_arity,
    )?;
    if let Some(key) = child {
        let expected = path
            .storage
            .identity_key_scalars
            .get(path.storage.identity.len())
            .copied()
            .flatten();
        stored_key_mismatch(expected, &key)?;
        return Ok(true);
    }
    Ok(false)
}

fn data_children_present(store: &TreeStore, path: &ResolvedDataPath) -> Result<bool, StoreError> {
    if path.storage.data_key_prefix_len < path.storage.data_key_scalars.len() {
        return keyed_data_children_present(store, path);
    }
    store.data_subtree_exists(
        &path.storage.store,
        &path.storage.identity,
        &path.storage.data_path,
    )
}

fn keyed_data_children_present(
    store: &TreeStore,
    path: &ResolvedDataPath,
) -> Result<bool, StoreError> {
    let child = store.data_first_child(
        &path.storage.store,
        &path.storage.identity,
        &path.storage.data_path,
    )?;
    if let Some(key) = child {
        validate_data_child_key(path, &key)?;
        return Ok(true);
    }
    Ok(false)
}

fn validate_data_child_key(path: &ResolvedDataPath, key: &SavedKey) -> Result<(), StoreError> {
    let expected = path
        .storage
        .data_key_scalars
        .get(path.storage.data_key_prefix_len)
        .copied()
        .flatten();
    stored_key_mismatch(expected, key)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use marrow_store::cell::CatalogId;
    use marrow_store::tree::{DataPathSegment as StoreDataPathSegment, DataValuePrefix};

    use super::super::path::StorageDataPath;
    use super::super::{DataPathSegment, MAX_VALUE_PREVIEW_LIMIT, ResolvedDataPath};
    use super::*;
    use crate::StoreLeafKind;

    #[test]
    fn preview_data_path_clamps_oversized_limit_before_prefix_read() {
        let store_id =
            CatalogId::new("cat_00000000000000000000000000000001".to_string()).expect("store id");
        let member_id =
            CatalogId::new("cat_00000000000000000000000000000002".to_string()).expect("member id");
        let path = ResolvedDataPath::new(
            "^books(1).title".to_string(),
            "books".to_string(),
            vec![
                DataPathSegment::Root("books".to_string()),
                DataPathSegment::Key(SavedKey::Int(1)),
                DataPathSegment::Field("title".to_string()),
            ],
            Some(StoreLeafKind::Scalar(marrow_store::value::ScalarType::Str)),
            StorageDataPath {
                store: store_id,
                identity: vec![SavedKey::Int(1)],
                identity_arity: 1,
                identity_key_scalars: vec![Some(crate::ScalarType::Int)],
                data_path: vec![StoreDataPathSegment::Member(member_id)],
                data_key_scalars: Vec::new(),
                data_key_prefix_len: 0,
            },
        );
        let store = TreeStore::memory();
        let mut observed_limit = None;

        let result = preview_data_path_with_prefix_reader(
            &CheckedProgram::default(),
            &store,
            &path,
            usize::MAX,
            |_, _, limit| {
                observed_limit = Some(limit);
                Ok(Some(DataValuePrefix {
                    bytes: b"abc".to_vec(),
                    truncated: false,
                }))
            },
        )
        .expect("preview read");

        assert_eq!(observed_limit, Some(MAX_VALUE_PREVIEW_LIMIT));
        assert_eq!(result.presence, DataPresence::ValueOnly);
        assert_eq!(result.preview.expect("preview").text, "\"abc\"");
    }
}
