use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

use super::record_nav;
use super::shape::stored_key_mismatch;
use super::{DataPresence, DataQuery, DebugDataPayload};

pub fn read_data_query(
    store: &TreeStore,
    query: &DataQuery,
) -> Result<(Option<DebugDataPayload>, DataPresence), StoreError> {
    if query.storage.identity.len() < query.storage.identity_arity {
        let has_children = record_children_present(store, query)?;
        return Ok((
            None,
            if has_children {
                DataPresence::ChildrenOnly
            } else {
                DataPresence::Absent
            },
        ));
    }
    if query.storage.data_path.is_empty() {
        let present = store.data_subtree_exists(
            &query.storage.store,
            &query.storage.identity,
            &query.storage.data_path,
        )?;
        return Ok((
            None,
            if present {
                DataPresence::ChildrenOnly
            } else {
                DataPresence::Absent
            },
        ));
    }
    let value = store.read_data_value(
        &query.storage.store,
        &query.storage.identity,
        &query.storage.data_path,
    )?;
    let presence = if value.is_some() {
        DataPresence::ValueOnly
    } else if data_children_present(store, query)? {
        DataPresence::ChildrenOnly
    } else {
        DataPresence::Absent
    };
    Ok((value.map(DebugDataPayload::new), presence))
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
