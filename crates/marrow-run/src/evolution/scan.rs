//! Paged record traversal shared by the apply staging helpers.
//!
//! Every apply scan visits record identities one full tuple at a time through the
//! store's paged child cursor, so apply never materializes the whole store: only the
//! current identity path is held. This is the mechanical descent the runtime store
//! cursor already supports; the semantic decisions (which member is a leaf, how a
//! stored value decodes to a key) stay with the checked facts and the store value
//! meaning, not here.

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

/// Visit every record identity under `store_id`, descending `arity` key levels and
/// invoking `visit` with each full identity tuple.
pub(super) fn for_each_record(
    store: &TreeStore,
    store_id: &CatalogId,
    arity: usize,
    visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut identity = Vec::new();
    descend(store, store_id, arity.max(1), &mut identity, visit)
}

fn descend(
    store: &TreeStore,
    store_id: &CatalogId,
    remaining: usize,
    identity: &mut Vec<SavedKey>,
    visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut next = store.record_first_child(store_id, identity)?;
    while let Some(key) = next {
        identity.push(key.clone());
        if remaining == 1 {
            visit(identity)?;
        } else {
            descend(store, store_id, remaining - 1, identity, visit)?;
        }
        identity.pop();
        next = store.record_next_child(store_id, identity, &key)?;
    }
    Ok(())
}
