use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use super::{DataPresence, DataQuery, DebugDataPayload};

pub fn read_data_query(
    store: &TreeStore,
    query: &DataQuery,
) -> Result<(Option<DebugDataPayload>, DataPresence), StoreError> {
    if query.storage.identity.len() < query.storage.identity_arity {
        let has_children = store
            .record_first_child(&query.storage.store, &query.storage.identity)?
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

pub fn data_presence_name(presence: DataPresence) -> &'static str {
    match presence {
        DataPresence::Absent => "absent",
        DataPresence::ValueOnly => "value_only",
        DataPresence::ChildrenOnly => "children_only",
    }
}
