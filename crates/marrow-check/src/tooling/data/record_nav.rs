use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

pub(super) fn first_record_child(
    store: &TreeStore,
    store_id: &CatalogId,
    identity_prefix: &[SavedKey],
    identity_arity: usize,
) -> Result<Option<SavedKey>, StoreError> {
    store.record_first_child_at_arity(store_id, identity_prefix, identity_arity)
}

pub(super) fn next_record_child(
    store: &TreeStore,
    store_id: &CatalogId,
    identity_prefix: &[SavedKey],
    identity_arity: usize,
    anchor: &SavedKey,
) -> Result<Option<SavedKey>, StoreError> {
    store.record_next_child_at_arity(store_id, identity_prefix, identity_arity, anchor)
}
