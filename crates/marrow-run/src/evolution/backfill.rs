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
use marrow_store::tree::ActivationDefaultRecordCount;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::index_maintenance::{PlanStepStagedData, index_rebuild_entry_with_staged};
use crate::store::{DataAddress, IndexAddress};
use crate::write_plan::PlanStep;

use super::apply::{
    ApplyError, MemberLocation, PathStep, StagedWork, for_each_place_record, locate_member,
    store_id,
};
use super::evidence::EvidenceDigest;

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
    steps: &mut Vec<PlanStep>,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let mut count = 0usize;
    let mut target_count = 0usize;
    let mut digest = EvidenceDigest::new("marrow-activation-default-v1");
    digest.catalog_id(catalog_id);
    for (place, location) in locations(places, catalog_id) {
        let sid = store_id(place)?;
        for_each_place_record(store, place, &mut |identity| {
            visit_member_cell_paths(store, &sid, identity, &location.steps, &mut |path| {
                target_count += 1;
                digest.catalog_id(&sid);
                digest.saved_keys(identity);
                digest.data_path(path);
                if store.data_subtree_exists(&sid, identity, path)? {
                    if fail_on_existing {
                        return Err(StoreError::Corruption {
                            message: "proposal default target already exists before activation"
                                .to_string(),
                        });
                    }
                    let current =
                        store
                            .read_data_value(&sid, identity, path)?
                            .ok_or_else(|| StoreError::Corruption {
                                message: "default target presence changed during staging"
                                    .to_string(),
                            })?;
                    digest.bytes(&current);
                } else {
                    steps.push(PlanStep::WriteData {
                        address: DataAddress::raw(sid.clone(), identity.to_vec(), path.to_vec()),
                        value: value.encoded.clone(),
                    });
                    digest.bytes(&value.encoded);
                    count += 1;
                }
                Ok(())
            })?;
            Ok(())
        })?;
    }
    staged.records_backfilled += count;
    digest.u64(count as u64);
    digest.u64(target_count as u64);
    staged
        .default_records_by_id
        .push(ActivationDefaultRecordCount {
            catalog_id: catalog_id.clone(),
            records_backfilled: count as u64,
            target_records: target_count as u64,
            evidence_digest: digest.finish(),
        });
    Ok(())
}

pub(super) fn stage_default_presence_receipt(
    catalog_id: &CatalogId,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let mut target_count = 0usize;
    let mut digest = EvidenceDigest::new("marrow-activation-default-v1");
    digest.catalog_id(catalog_id);
    for (place, location) in locations(places, catalog_id) {
        let sid = store_id(place)?;
        for_each_place_record(store, place, &mut |identity| {
            visit_member_cell_paths(store, &sid, identity, &location.steps, &mut |path| {
                target_count += 1;
                let Some(current) = store.read_data_value(&sid, identity, path)? else {
                    return Err(StoreError::Corruption {
                        message: "default receipt target is missing before activation".to_string(),
                    });
                };
                digest.catalog_id(&sid);
                digest.saved_keys(identity);
                digest.data_path(path);
                digest.bytes(&current);
                Ok(())
            })?;
            Ok(())
        })?;
    }
    digest.u64(0);
    digest.u64(target_count as u64);
    staged
        .default_records_by_id
        .push(ActivationDefaultRecordCount {
            catalog_id: catalog_id.clone(),
            records_backfilled: 0,
            target_records: target_count as u64,
            evidence_digest: digest.finish(),
        });
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
            .find(|index| index.catalog_id.as_deref() == Some(catalog_id.as_str()))
        else {
            continue;
        };
        let index = index.clone();
        let index_id =
            CatalogId::new(
                index
                    .catalog_id
                    .clone()
                    .ok_or_else(|| StoreError::Corruption {
                        message: "evolution apply saw a missing index catalog id".to_string(),
                    })?,
            )
            .map_err(|_| StoreError::Corruption {
                message: "evolution apply saw an invalid index catalog id".to_string(),
            })?;
        let mut index_steps = vec![PlanStep::DeleteIndexSubtree {
            address: IndexAddress {
                index: index_id,
                keys: Vec::new(),
            },
        }];
        let staged_data = PlanStepStagedData {
            steps: steps.as_slice(),
        };
        for_each_place_record(store, place, &mut |identity| {
            if let Some(step) = index_rebuild_entry_with_staged(
                &index,
                place,
                identity,
                store,
                &staged_data,
                Default::default(),
            )
            .map_err(|error| StoreError::Corruption {
                message: error.message,
            })? {
                index_steps.push(step);
            }
            Ok(())
        })?;
        steps.extend(index_steps);
        staged.indexes_rebuilt += 1;
        return Ok(());
    }
    Ok(())
}

/// Stage a `DeleteIndexSubtree` of every cell under a source-dropped index. The index
/// is gone from current source, so it is addressed directly by its catalog id rather
/// than located in a place: an empty key prefix names the whole index-cell subtree.
/// Catalog ids are globally unique, so this drops exactly the dropped index's cells and
/// nothing else. The delete is idempotent — a resumed apply over an already-cleared
/// index subtree deletes nothing.
pub(super) fn stage_index_drop(
    catalog_id: &CatalogId,
    steps: &mut Vec<PlanStep>,
) -> Result<(), ApplyError> {
    steps.push(PlanStep::DeleteIndexSubtree {
        address: IndexAddress {
            index: catalog_id.clone(),
            keys: Vec::new(),
        },
    });
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
