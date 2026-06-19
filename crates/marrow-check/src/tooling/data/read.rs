use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataValuePrefix, TreeStore};

use super::record_nav;
use super::render::render_data_query_value_prefix_preview;
use super::shape::stored_key_mismatch;
use super::{
    DataPresence, DataPreviewReadResult, DataQuery, DataReadResult, DebugDataPayload,
    clamp_value_preview_limit,
};
use crate::CheckedProgram;

pub fn read_data_query(store: &TreeStore, query: &DataQuery) -> Result<DataReadResult, StoreError> {
    let result = read_data_query_with(store, query, |store, query| {
        store.read_data_value(
            &query.storage.store,
            &query.storage.identity,
            &query.storage.data_path,
        )
    })?;
    Ok(DataReadResult {
        payload: result.value.map(DebugDataPayload::new),
        presence: result.presence,
    })
}

pub fn preview_data_query(
    program: &CheckedProgram,
    store: &TreeStore,
    query: &DataQuery,
    limit: usize,
) -> Result<DataPreviewReadResult, StoreError> {
    preview_data_query_with_prefix_reader(program, store, query, limit, |store, query, limit| {
        store.read_data_value_prefix(
            &query.storage.store,
            &query.storage.identity,
            &query.storage.data_path,
            limit,
        )
    })
}

fn preview_data_query_with_prefix_reader(
    program: &CheckedProgram,
    store: &TreeStore,
    query: &DataQuery,
    limit: usize,
    read_prefix: impl FnOnce(
        &TreeStore,
        &DataQuery,
        usize,
    ) -> Result<Option<DataValuePrefix>, StoreError>,
) -> Result<DataPreviewReadResult, StoreError> {
    let limit = clamp_value_preview_limit(limit);
    let result = read_data_query_with(store, query, |store, query| {
        read_prefix(store, query, limit)
    })?;
    let preview = result.value.as_ref().map(|prefix| {
        render_data_query_value_prefix_preview(
            program,
            query,
            &prefix.bytes,
            prefix.truncated,
            limit,
        )
    });
    Ok(DataPreviewReadResult {
        preview,
        presence: result.presence,
    })
}

struct DataQueryRead<T> {
    value: Option<T>,
    presence: DataPresence,
}

fn read_data_query_with<T>(
    store: &TreeStore,
    query: &DataQuery,
    read_value: impl FnOnce(&TreeStore, &DataQuery) -> Result<Option<T>, StoreError>,
) -> Result<DataQueryRead<T>, StoreError> {
    if query.storage.identity.len() < query.storage.identity_arity {
        return children_presence(record_children_present(store, query)?);
    }
    if query.storage.data_path.is_empty() {
        return children_presence(store.data_subtree_exists(
            &query.storage.store,
            &query.storage.identity,
            &query.storage.data_path,
        )?);
    }
    let value = read_value(store, query)?;
    let presence = if value.is_some() {
        DataPresence::ValueOnly
    } else if data_children_present(store, query)? {
        DataPresence::ChildrenOnly
    } else {
        DataPresence::Absent
    };
    Ok(DataQueryRead { value, presence })
}

fn children_presence<T>(has_children: bool) -> Result<DataQueryRead<T>, StoreError> {
    let presence = if has_children {
        DataPresence::ChildrenOnly
    } else {
        DataPresence::Absent
    };
    Ok(DataQueryRead {
        value: None,
        presence,
    })
}

fn record_children_present(store: &TreeStore, query: &DataQuery) -> Result<bool, StoreError> {
    let mut child = record_nav::first_record_child(
        store,
        &query.storage.store,
        &query.storage.identity,
        query.storage.identity_arity,
    )?;
    let mut present = false;
    while let Some(key) = child {
        let expected = query
            .storage
            .identity_key_scalars
            .get(query.storage.identity.len())
            .copied()
            .flatten();
        stored_key_mismatch(expected, &key)?;
        present = true;
        child = record_nav::next_record_child(
            store,
            &query.storage.store,
            &query.storage.identity,
            query.storage.identity_arity,
            &key,
        )?;
    }
    Ok(present)
}

fn data_children_present(store: &TreeStore, query: &DataQuery) -> Result<bool, StoreError> {
    if query.storage.data_key_prefix_len < query.storage.data_key_scalars.len() {
        return keyed_data_children_present(store, query);
    }
    store.data_subtree_exists(
        &query.storage.store,
        &query.storage.identity,
        &query.storage.data_path,
    )
}

fn keyed_data_children_present(store: &TreeStore, query: &DataQuery) -> Result<bool, StoreError> {
    let mut child = store.data_first_child(
        &query.storage.store,
        &query.storage.identity,
        &query.storage.data_path,
    )?;
    let mut present = false;
    while let Some(key) = child {
        validate_data_child_key(query, &key)?;
        present = true;
        child = store.data_next_child(
            &query.storage.store,
            &query.storage.identity,
            &query.storage.data_path,
            &key,
        )?;
    }
    Ok(present)
}

fn validate_data_child_key(query: &DataQuery, key: &SavedKey) -> Result<(), StoreError> {
    let expected = query
        .storage
        .data_key_scalars
        .get(query.storage.data_key_prefix_len)
        .copied()
        .flatten();
    stored_key_mismatch(expected, key)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use marrow_store::cell::CatalogId;
    use marrow_store::tree::{DataPathSegment, DataValuePrefix};

    use super::super::query::StorageDataQuery;
    use super::super::{DataQuery, DataQuerySegment, MAX_VALUE_PREVIEW_LIMIT};
    use super::*;
    use crate::StoreLeafKind;

    #[test]
    fn preview_data_query_clamps_oversized_limit_before_prefix_read() {
        let store_id =
            CatalogId::new("cat_00000000000000000000000000000001".to_string()).expect("store id");
        let member_id =
            CatalogId::new("cat_00000000000000000000000000000002".to_string()).expect("member id");
        let query = DataQuery::new(
            "^books(1).title".to_string(),
            "books".to_string(),
            vec![
                DataQuerySegment::Root("books".to_string()),
                DataQuerySegment::Key(SavedKey::Int(1)),
                DataQuerySegment::Field("title".to_string()),
            ],
            Some(StoreLeafKind::Scalar(marrow_store::value::ScalarType::Str)),
            StorageDataQuery {
                store: store_id,
                identity: vec![SavedKey::Int(1)],
                identity_arity: 1,
                identity_key_scalars: vec![Some(crate::ScalarType::Int)],
                data_path: vec![DataPathSegment::Member(member_id)],
                data_key_scalars: Vec::new(),
                data_key_prefix_len: 0,
            },
        );
        let store = TreeStore::memory();
        let mut observed_limit = None;

        let result = preview_data_query_with_prefix_reader(
            &CheckedProgram::default(),
            &store,
            &query,
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
