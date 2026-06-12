//! Stage the durable work each applyable verdict implies.
//!
//! Each helper re-derives its work from the live store and the checked saved places,
//! never from an identity list in the witness, and reuses the shared semantic owners:
//! index keys come from managed-write index maintenance, and a member's place in the
//! tree comes from the checked facts. Each obligation re-scans its root independently,
//! paged one identity at a time, so cost is bounded store reads rather than materialized
//! records; fusing the scans is not worth the complexity at apply's maintenance cadence.

use marrow_check::evolution::DefaultValue;
use marrow_check::{CheckedSavedIndex, CheckedSavedPlace, for_each_place_record};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::index_maintenance::{EmptyStagedData, index_rebuild_entry_with_staged};
use crate::write_plan::PlanStep;

use super::apply::{ActivationDefaultRecordCount, ApplyError, StagedWork};
use super::locate::{MemberLocation, PathStep, locate_member, store_id};

/// Walk every default-target cell for `catalog_id` across the source places. The returned
/// target count feeds the in-memory apply receipt; commit metadata persists only the slim
/// stamp.
fn scan_default_cells<F>(
    catalog_id: &CatalogId,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    mut cell: F,
) -> Result<usize, ApplyError>
where
    F: FnMut(&CatalogId, &[SavedKey], &[DataPathSegment]) -> Result<(), StoreError>,
{
    let mut target_count = 0usize;
    for (place, location) in locations(places, catalog_id) {
        let sid = store_id(place)?;
        for_each_place_record(store, place, &mut |identity| {
            visit_member_cell_paths(store, &sid, identity, &location.steps, &mut |path| {
                target_count += 1;
                cell(&sid, identity, path)
            })
        })?;
    }
    Ok(target_count)
}

/// Stage a `WriteData` of the encoded default at every record (or keyed entry) that
/// lacks the defaulted member. Existing cells on accepted optional members are
/// preserved; existing cells on proposal-new members fail closed before commit because
/// the accepted catalog never owned those target paths.
pub(super) fn stage_default_backfill(
    catalog_id: &CatalogId,
    value: &DefaultValue,
    fail_on_existing: bool,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let mut count = 0usize;
    let target_count = scan_default_cells(catalog_id, places, store, |sid, identity, path| {
        if store.data_subtree_exists(sid, identity, path)? {
            if fail_on_existing {
                return Err(StoreError::Corruption {
                    message: "proposal default target already exists before activation".to_string(),
                });
            }
            store
                .read_data_value(sid, identity, path)?
                .ok_or_else(|| StoreError::Corruption {
                    message: "default target presence changed during staging".to_string(),
                })?;
        } else {
            store.write_node(sid, identity)?;
            store.write_data_value(sid, identity, path, value.encoded.clone())?;
            count += 1;
        }
        Ok(())
    })?;
    staged.records_backfilled += count;
    push_default_receipt(staged, catalog_id, count, target_count);
    Ok(())
}

pub(super) fn stage_default_presence_receipt(
    catalog_id: &CatalogId,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let target_count = scan_default_cells(catalog_id, places, store, |sid, identity, path| {
        if store.read_data_value(sid, identity, path)?.is_none() {
            return Err(StoreError::Corruption {
                message: "default receipt target is missing before activation".to_string(),
            });
        }
        Ok(())
    })?;
    push_default_receipt(staged, catalog_id, 0, target_count);
    Ok(())
}

/// Record the per-id counts the CLI can render from the in-memory apply receipt.
fn push_default_receipt(
    staged: &mut StagedWork,
    catalog_id: &CatalogId,
    records_backfilled: usize,
    target_count: usize,
) {
    staged
        .default_records_by_id
        .push(ActivationDefaultRecordCount {
            catalog_id: catalog_id.clone(),
            records_backfilled: records_backfilled as u64,
            target_records: target_count as u64,
        });
}

/// Stage a full clear-and-repopulate of one declared index on `place`: a single
/// `DeleteIndexSubtree` over the whole index followed by the entry each record contributes.
/// A record whose key columns are absent contributes no entry. The caller runs inside an
/// open transaction, so same-apply defaults and transforms are visible through normal
/// store reads before the rebuild derives index keys.
pub(super) fn stage_index_subtree_rebuild(
    index: &CheckedSavedIndex,
    place: &CheckedSavedPlace,
    store: &TreeStore,
) -> Result<(), ApplyError> {
    let index_id = index_catalog_id(index)?;
    store.delete_index_subtree(&index_id, &[])?;
    for_each_place_record(store, place, &mut |identity| {
        if let Some(step) = index_rebuild_entry_with_staged(
            index,
            place,
            identity,
            store,
            &EmptyStagedData,
            Default::default(),
        )
        .map_err(|error| StoreError::Corruption {
            message: error.message,
        })? {
            write_index_step(store, step)?;
        }
        Ok(())
    })?;
    Ok(())
}

fn write_index_step(store: &TreeStore, step: PlanStep) -> Result<(), StoreError> {
    match step {
        PlanStep::WriteIndex {
            address,
            identity,
            value,
        } => store.write_index_entry(&address.index, &address.keys, &identity, value),
        _ => Err(StoreError::Corruption {
            message: "index rebuild produced a non-index write".to_string(),
        }),
    }
}

/// The catalog id of a declared index, or a store-corruption fault when it is missing or
/// malformed. A declared index always carries a stable catalog id once checked, so either
/// failure is an apply/discharge divergence rather than a recoverable condition.
fn index_catalog_id(index: &CheckedSavedIndex) -> Result<CatalogId, ApplyError> {
    let raw = index
        .catalog_id
        .clone()
        .ok_or_else(|| StoreError::Corruption {
            message: "index rebuild saw a missing index catalog id".to_string(),
        })?;
    CatalogId::new(raw).map_err(|_| {
        ApplyError::Store(StoreError::Corruption {
            message: "index rebuild saw an invalid index catalog id".to_string(),
        })
    })
}

/// Stage the index entry every record contributes to the rebuilt index named by
/// `catalog_id`. The index belongs to exactly one place, so the first place that declares
/// it owns the rebuild and the scan stops there. A rebuild obligation the witness proved
/// must resolve to a declared index; finding none is a discharge/apply divergence, so it
/// fails closed rather than stamping success over a silently un-rebuilt index.
pub(super) fn stage_index_rebuild(
    catalog_id: &CatalogId,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    for place in places {
        let Some(index) = place
            .indexes
            .iter()
            .find(|index| index.catalog_id.as_deref() == Some(catalog_id.as_str()))
        else {
            continue;
        };
        stage_index_subtree_rebuild(index, place, store)?;
        staged.indexes_rebuilt += 1;
        return Ok(());
    }
    Err(ApplyError::Store(StoreError::Corruption {
        message: "evolution apply found no declared index for a rebuild obligation".to_string(),
    }))
}

/// Stage a `DeleteIndexSubtree` of every cell under a source-dropped index. The index
/// is gone from current source, so it is addressed directly by its catalog id rather
/// than located in a place: an empty key prefix names the whole index-cell subtree.
/// Catalog ids are globally unique, so this drops exactly the dropped index's cells and
/// nothing else. The delete is idempotent — a re-apply over an already-cleared
/// index subtree deletes nothing.
pub(super) fn stage_index_drop(
    catalog_id: &CatalogId,
    store: &TreeStore,
) -> Result<(), ApplyError> {
    store.delete_index_subtree(catalog_id, &[])?;
    Ok(())
}

/// Stage a `DeleteData` of the retired member subtree at every record that carries it.
/// A retired member is gone from current source, so it is addressed directly by its
/// catalog id rather than located in the member tree: its cells were written under that
/// id at a top-level member path. Catalog ids are globally unique, so the cell exists
/// under exactly the store that owns the member. The retire was approved with a count
/// matching the witness, so the deletes drop exactly that data.
pub(super) fn stage_retire_deletes(
    catalog_id: &CatalogId,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let path = [DataPathSegment::Member(catalog_id.clone())];
    let mut count = 0usize;
    for place in places {
        let sid = store_id(place)?;
        for_each_place_record(store, place, &mut |identity| {
            if store.data_subtree_exists(&sid, identity, &path)? {
                store.delete_data_subtree(&sid, identity, &path)?;
                count += 1;
            }
            Ok(())
        })?;
    }
    staged.records_retired += count;
    Ok(())
}

/// Find every place and member location for `catalog_id` across the source places.
pub(super) fn locations<'a>(
    places: &'a [CheckedSavedPlace],
    catalog_id: &CatalogId,
) -> Vec<(&'a CheckedSavedPlace, MemberLocation)> {
    places
        .iter()
        .filter_map(|place| locate_member(place, catalog_id).map(|location| (place, location)))
        .collect()
}

/// A `Member` step appends one named segment; a `Layer` step pages each existing
/// keyed entry and recurses with its key appended, so a member under a keyed
/// layer yields one path per existing entry and a direct member yields exactly
/// one path.
pub(super) fn visit_member_cell_paths<F>(
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    steps: &[PathStep],
    visit: &mut F,
) -> Result<(), StoreError>
where
    F: FnMut(&[DataPathSegment]) -> Result<(), StoreError>,
{
    descend(store, store_id, identity, &mut Vec::new(), steps, visit)
}

fn descend<F>(
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    prefix: &mut Vec<DataPathSegment>,
    steps: &[PathStep],
    visit: &mut F,
) -> Result<(), StoreError>
where
    F: FnMut(&[DataPathSegment]) -> Result<(), StoreError>,
{
    let Some((step, rest)) = steps.split_first() else {
        return visit(prefix);
    };
    match step {
        PathStep::Member(id) => {
            prefix.push(DataPathSegment::Member(id.clone()));
            descend(store, store_id, identity, prefix, rest, visit)?;
            prefix.pop();
        }
        PathStep::Layer(id) => {
            prefix.push(DataPathSegment::Member(id.clone()));
            let mut child = store.data_first_child(store_id, identity, prefix)?;
            while let Some(entry_key) = child {
                prefix.push(DataPathSegment::Key(entry_key));
                descend(store, store_id, identity, prefix, rest, visit)?;
                let Some(DataPathSegment::Key(entry_key)) = prefix.pop() else {
                    return Err(StoreError::Corruption {
                        message: "evolution default traversal lost its keyed cursor".to_string(),
                    });
                };
                child = store.data_next_child(store_id, identity, prefix, &entry_key)?;
            }
            prefix.pop();
        }
    }
    Ok(())
}
