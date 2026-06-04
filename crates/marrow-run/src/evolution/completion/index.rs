use marrow_check::{CatalogEntryKind, CheckedProgram, CheckedSavedIndex, CheckedSavedPlace};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, IndexCursor, TreeStore};
use marrow_syntax::SourceSpan;

use crate::index_maintenance::{EmptyStagedData, index_rebuild_entry_with_staged};
use crate::write_plan::PlanStep;

use super::super::apply::{ApplyError, for_each_place_record};
use super::super::evidence::{EvidenceDigest, EvidenceSetDigest};
use super::{catalog_id, retired_ids};

const INDEX_SCAN_PAGE: usize = 128;

pub(super) fn verify_index_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    commit: &CommitMetadata,
    places: &[CheckedSavedPlace],
) -> Result<usize, ApplyError> {
    let mut rebuilt = 0usize;
    for index_id in &commit.changed_index_catalog_ids {
        if let Some((place, index)) = active_index(places, index_id) {
            verify_rebuilt_index(store, place, index)?;
            rebuilt += 1;
        } else if !index_is_empty(store, index_id)? {
            return Err(ApplyError::Drift);
        }
    }
    for index_id in retired_ids(program, CatalogEntryKind::StoreIndex) {
        if !index_is_empty(store, &index_id)? {
            return Err(ApplyError::Drift);
        }
    }
    Ok(rebuilt)
}

fn verify_rebuilt_index(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    index: &CheckedSavedIndex,
) -> Result<(), ApplyError> {
    let Some(index_catalog_id) = index.catalog_id.as_deref() else {
        return Err(ApplyError::Drift);
    };
    let index_id = catalog_id(index_catalog_id)?;
    let mut expected = EvidenceSetDigest::default();
    for_each_place_record(store, place, &mut |identity| {
        if let Some(PlanStep::WriteIndex {
            address,
            identity,
            value,
        }) = index_rebuild_entry_with_staged(
            index,
            place,
            identity,
            store,
            &EmptyStagedData,
            SourceSpan::default(),
        )
        .map_err(|error| StoreError::Corruption {
            message: error.message,
        })? {
            add_index_row(&mut expected, &index_id, &address.keys, &identity, &value);
        }
        Ok(())
    })?;

    let actual = actual_index_digest(store, &index_id, index.keys.len())?;
    if actual.finish("marrow-index-set-v1") != expected.finish("marrow-index-set-v1") {
        return Err(ApplyError::Drift);
    }
    Ok(())
}

fn active_index<'a>(
    places: &'a [CheckedSavedPlace],
    index_id: &CatalogId,
) -> Option<(&'a CheckedSavedPlace, &'a CheckedSavedIndex)> {
    places.iter().find_map(|place| {
        place
            .indexes
            .iter()
            .find(|index| index.catalog_id.as_deref() == Some(index_id.as_str()))
            .map(|index| (place, index))
    })
}

fn index_is_empty(store: &TreeStore, index: &CatalogId) -> Result<bool, ApplyError> {
    Ok(store.index_child_keys(index, &[])?.is_empty()
        && store.scan_index_tuple(index, &[], 1)?.entries.is_empty())
}

fn add_index_row(
    digest: &mut EvidenceSetDigest,
    index: &CatalogId,
    keys: &[SavedKey],
    identity: &[SavedKey],
    value: &[u8],
) {
    let mut row = EvidenceDigest::new("marrow-index-row-v1");
    row.catalog_id(index);
    row.saved_keys(keys);
    row.saved_keys(identity);
    row.bytes(value);
    digest.add(row);
}

fn actual_index_digest(
    store: &TreeStore,
    index: &CatalogId,
    key_len: usize,
) -> Result<EvidenceSetDigest, ApplyError> {
    let mut digest = EvidenceSetDigest::default();
    let mut prefix = Vec::new();
    scan_index_prefix(store, index, key_len, &mut prefix, &mut digest)?;
    Ok(digest)
}

fn scan_index_prefix(
    store: &TreeStore,
    index: &CatalogId,
    key_len: usize,
    prefix: &mut Vec<SavedKey>,
    digest: &mut EvidenceSetDigest,
) -> Result<(), ApplyError> {
    if prefix.len() == key_len {
        scan_index_tuple_entries(store, index, prefix, digest)?;
        return Ok(());
    }
    let mut child = store.index_first_child(index, prefix)?;
    while let Some(key) = child {
        prefix.push(key);
        scan_index_prefix(store, index, key_len, prefix, digest)?;
        let Some(key) = prefix.pop() else {
            return Err(ApplyError::Drift);
        };
        child = store.index_next_child(index, prefix, &key)?;
    }
    Ok(())
}

fn scan_index_tuple_entries(
    store: &TreeStore,
    index: &CatalogId,
    keys: &[SavedKey],
    digest: &mut EvidenceSetDigest,
) -> Result<(), ApplyError> {
    let mut cursor: Option<IndexCursor> = None;
    loop {
        let page = match &cursor {
            Some(cursor) => store.scan_index_tuple_after(index, keys, cursor, INDEX_SCAN_PAGE)?,
            None => store.scan_index_tuple(index, keys, INDEX_SCAN_PAGE)?,
        };
        for entry in page.entries {
            add_index_row(digest, index, keys, &entry.identity, &entry.value);
        }
        let Some(next) = page.cursor else {
            break;
        };
        cursor = Some(next);
    }
    Ok(())
}
