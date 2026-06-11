use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use super::record_nav;
use super::{DataPresence, DataQuery, DebugDataPayload};

pub fn read_data_query(
    store: &TreeStore,
    query: &DataQuery,
) -> Result<(Option<DebugDataPayload>, DataPresence), StoreError> {
    if query.storage.identity.len() < query.storage.identity_arity {
        let has_children = record_nav::first_record_child(
            store,
            &query.storage.store,
            &query.storage.identity,
            query.storage.identity_arity,
        )?
        .is_some();
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
    } else if store.data_subtree_exists(
        &query.storage.store,
        &query.storage.identity,
        &query.storage.data_path,
    )? {
        DataPresence::ChildrenOnly
    } else {
        DataPresence::Absent
    };
    Ok((value.map(DebugDataPayload::new), presence))
}
