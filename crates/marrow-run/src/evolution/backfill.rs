//! Stage the durable work each applyable verdict implies.
//!
//! Each helper here re-derives its work from the live store and the checked saved
//! places, never from an identity list in the witness. A defaulting obligation writes
//! the encoded constant at every record (or keyed entry) that lacks the member; a
//! derived rebuild writes the index entry every record contributes; an approved
//! destructive retire deletes the retired member subtree per record. The semantic
//! owners are shared: index keys come from the managed-write index maintenance, and a
//! member's place in the tree comes from the checked facts.
//!
//! Each obligation re-scans its root independently rather than sharing one pass across
//! obligations. The scan is paged one identity at a time, so the cost is bounded store
//! reads, not materialized records, and apply runs in a maintenance window where a
//! second pass per obligation is cheaper than the complexity of fusing them. Fusing the
//! scans would trade that clarity for a saving that does not matter at this cadence.

use marrow_check::CheckedSavedPlace;
use marrow_check::evolution::DefaultValue;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::index_maintenance::stage_index_rebuild_entry;
use crate::store::{DataAddress, IndexAddress};
use crate::write_plan::PlanStep;

use super::apply::{
    ApplyError, MemberLocation, PathStep, StagedWork, for_each_place_record, locate_member,
    store_id,
};

/// Stage a `WriteData` of the encoded default at every record (or keyed entry) that
/// lacks the defaulted member. Backfilling a member a record already carries is a
/// no-op, so a resumed apply over an already-applied store stages nothing.
pub(super) fn stage_default_backfill(
    catalog_id: &CatalogId,
    value: &DefaultValue,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    steps: &mut Vec<PlanStep>,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let Some((place, location)) = locate(places, catalog_id) else {
        return Ok(());
    };
    let sid = store_id(place)?;
    let mut count = 0usize;
    for_each_place_record(store, place, &mut |identity| {
        for path in member_cell_paths(store, &sid, identity, &location.steps)? {
            if !store.data_subtree_exists(&sid, identity, &path)? {
                steps.push(PlanStep::WriteData {
                    address: DataAddress::raw(sid.clone(), identity.to_vec(), path),
                    value: value.encoded.clone(),
                });
                count += 1;
            }
        }
        Ok(())
    })?;
    staged.records_backfilled += count;
    Ok(())
}

/// Stage the index entry every record contributes to the rebuilt index. A record whose
/// key columns are absent contributes no entry. The index-key derivation and the entry
/// value are the managed-write owners, so the rebuilt index matches a maintained one.
pub(super) fn stage_index_rebuild(
    catalog_id: &CatalogId,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    steps: &mut Vec<PlanStep>,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    for place in places {
        let Some(index) = place
            .indexes
            .iter()
            .find(|index| index.catalog_id == catalog_id.as_str())
        else {
            continue;
        };
        let index = index.clone();
        let index_id =
            CatalogId::new(index.catalog_id.clone()).map_err(|_| StoreError::Corruption {
                message: "evolution apply saw an invalid index catalog id".to_string(),
            })?;
        steps.push(PlanStep::DeleteIndexSubtree {
            address: IndexAddress {
                index: index_id,
                keys: Vec::new(),
            },
        });
        for_each_place_record(store, place, &mut |identity| {
            stage_index_rebuild_entry(steps, &index, place, identity, store, Default::default())
                .map(|_| ())
                .map_err(|error| StoreError::Corruption {
                    message: error.message,
                })
        })?;
        staged.indexes_rebuilt += 1;
        return Ok(());
    }
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
    steps: &mut Vec<PlanStep>,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let path = [DataPathSegment::Member(catalog_id.clone())];
    let mut count = 0usize;
    for place in places {
        let sid = store_id(place)?;
        for_each_place_record(store, place, &mut |identity| {
            if store.data_subtree_exists(&sid, identity, &path)? {
                steps.push(PlanStep::DeleteData {
                    address: DataAddress::raw(sid.clone(), identity.to_vec(), path.to_vec()),
                });
                count += 1;
            }
            Ok(())
        })?;
    }
    staged.records_retired += count;
    Ok(())
}

/// Find the place and member location for `catalog_id` across the source places.
fn locate<'a>(
    places: &'a [CheckedSavedPlace],
    catalog_id: &CatalogId,
) -> Option<(&'a CheckedSavedPlace, MemberLocation)> {
    places
        .iter()
        .find_map(|place| locate_member(place, catalog_id).map(|location| (place, location)))
}

/// Every concrete data path a member's descent steps reach for one record. A `Member`
/// step appends one named segment; a `Layer` step pages each existing keyed entry and
/// recurses with its key appended, so a member under a keyed layer yields one path per
/// existing entry and a direct member yields exactly one path. The store cursor pages
/// the entries, so only the current record's paths are held.
fn member_cell_paths(
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    steps: &[PathStep],
) -> Result<Vec<Vec<DataPathSegment>>, StoreError> {
    let mut paths = Vec::new();
    descend(
        store,
        store_id,
        identity,
        &mut Vec::new(),
        steps,
        &mut paths,
    )?;
    Ok(paths)
}

fn descend(
    store: &TreeStore,
    store_id: &CatalogId,
    identity: &[SavedKey],
    prefix: &mut Vec<DataPathSegment>,
    steps: &[PathStep],
    paths: &mut Vec<Vec<DataPathSegment>>,
) -> Result<(), StoreError> {
    let Some((step, rest)) = steps.split_first() else {
        paths.push(prefix.clone());
        return Ok(());
    };
    match step {
        PathStep::Member(id) => {
            prefix.push(DataPathSegment::Member(id.clone()));
            descend(store, store_id, identity, prefix, rest, paths)?;
            prefix.pop();
        }
        PathStep::Layer(id) => {
            prefix.push(DataPathSegment::Member(id.clone()));
            for entry_key in store.data_child_keys(store_id, identity, prefix)? {
                prefix.push(DataPathSegment::Key(entry_key));
                descend(store, store_id, identity, prefix, rest, paths)?;
                prefix.pop();
            }
            prefix.pop();
        }
    }
    Ok(())
}
