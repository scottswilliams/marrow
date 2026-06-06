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

use marrow_check::evolution::DefaultValue;
use marrow_check::{CheckedSavedIndex, CheckedSavedPlace};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::ActivationDefaultRecordCount;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::index_maintenance::{
    PlanStepStagedData, StagedDataView, index_rebuild_entry_with_staged,
};
use crate::store::{DataAddress, IndexAddress};
use crate::write_plan::PlanStep;

use super::apply::{
    ApplyError, MemberLocation, PathStep, StagedWork, for_each_place_record, locate_member,
    store_id,
};
use super::evidence::{ACTIVATION_DEFAULT_DIGEST, EvidenceDigest};

/// Fold one default-target cell's identity into the activation-default evidence digest:
/// the store id, the record identity, the cell path, and the cell's bytes, in that fixed
/// order. Backfill staging, the read-only presence receipt, and crash-resume completion
/// all build their digest from this one recipe so the staged and verified digests cannot
/// drift apart by a reordering or a missed field.
pub(super) fn fold_default_cell(
    digest: &mut EvidenceDigest,
    store_id: &CatalogId,
    identity: &[SavedKey],
    path: &[DataPathSegment],
    bytes: &[u8],
) {
    digest.catalog_id(store_id);
    digest.saved_keys(identity);
    digest.data_path(path);
    digest.bytes(bytes);
}

/// Walk every default-target cell for `catalog_id` across the source places, seeding the
/// shared activation-default digest and invoking `cell` per cell with that digest plus the
/// resolved store id, record identity, and cell path. The returned `(digest, target_count)`
/// lets each caller finish the digest with its own backfilled count and push the receipt;
/// the scan itself is identical whether the caller writes defaults or only reads cells.
fn scan_default_cells<F>(
    catalog_id: &CatalogId,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    mut cell: F,
) -> Result<(EvidenceDigest, usize), ApplyError>
where
    F: FnMut(
        &mut EvidenceDigest,
        &CatalogId,
        &[SavedKey],
        &[DataPathSegment],
    ) -> Result<(), StoreError>,
{
    let mut digest = EvidenceDigest::new(ACTIVATION_DEFAULT_DIGEST);
    digest.catalog_id(catalog_id);
    let mut target_count = 0usize;
    for (place, location) in locations(places, catalog_id) {
        let sid = store_id(place)?;
        for_each_place_record(store, place, &mut |identity| {
            visit_member_cell_paths(store, &sid, identity, &location.steps, &mut |path| {
                target_count += 1;
                cell(&mut digest, &sid, identity, path)
            })
        })?;
    }
    Ok((digest, target_count))
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
    steps: &mut Vec<PlanStep>,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let mut count = 0usize;
    let (mut digest, target_count) =
        scan_default_cells(catalog_id, places, store, |digest, sid, identity, path| {
            if store.data_subtree_exists(sid, identity, path)? {
                if fail_on_existing {
                    return Err(StoreError::Corruption {
                        message: "proposal default target already exists before activation"
                            .to_string(),
                    });
                }
                let current = store.read_data_value(sid, identity, path)?.ok_or_else(|| {
                    StoreError::Corruption {
                        message: "default target presence changed during staging".to_string(),
                    }
                })?;
                fold_default_cell(digest, sid, identity, path, &current);
            } else {
                steps.push(PlanStep::WriteData {
                    address: DataAddress::from_resolved_parts(
                        sid.clone(),
                        identity.to_vec(),
                        path.to_vec(),
                    ),
                    value: value.encoded.clone(),
                });
                fold_default_cell(digest, sid, identity, path, &value.encoded);
                count += 1;
            }
            Ok(())
        })?;
    staged.records_backfilled += count;
    push_default_receipt(staged, catalog_id, &mut digest, count, target_count);
    Ok(())
}

pub(super) fn stage_default_presence_receipt(
    catalog_id: &CatalogId,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let (mut digest, target_count) =
        scan_default_cells(catalog_id, places, store, |digest, sid, identity, path| {
            let Some(current) = store.read_data_value(sid, identity, path)? else {
                return Err(StoreError::Corruption {
                    message: "default receipt target is missing before activation".to_string(),
                });
            };
            fold_default_cell(digest, sid, identity, path, &current);
            Ok(())
        })?;
    push_default_receipt(staged, catalog_id, &mut digest, 0, target_count);
    Ok(())
}

/// Finish the activation-default digest with the backfilled and target counts and record
/// the per-id receipt. The two counts close the digest in the same order both the staging
/// helpers and crash-resume completion expect, so a receipt verifies against its stamp.
fn push_default_receipt(
    staged: &mut StagedWork,
    catalog_id: &CatalogId,
    digest: &mut EvidenceDigest,
    records_backfilled: usize,
    target_count: usize,
) {
    digest.u64(records_backfilled as u64);
    digest.u64(target_count as u64);
    staged
        .default_records_by_id
        .push(ActivationDefaultRecordCount {
            catalog_id: catalog_id.clone(),
            records_backfilled: records_backfilled as u64,
            target_records: target_count as u64,
            evidence_digest: digest.finish(),
        });
}

/// Stage a full clear-and-repopulate of one declared index on `place`: a single
/// `DeleteIndexSubtree` over the whole index followed by the entry each record contributes.
/// A record whose key columns are absent contributes no entry. `staged` lets a rebuild see
/// data writes already staged in the same apply (so a defaulted member is indexed at its
/// staged value); restore passes an empty view because it replays committed data only. The
/// index-key derivation and entry value are the managed-write owners, so a rebuilt index is
/// byte-identical to a maintained one. The returned steps are appended after the existing
/// plan, never threaded back into the staged view that read it.
pub(super) fn stage_index_subtree_rebuild(
    index: &CheckedSavedIndex,
    place: &CheckedSavedPlace,
    store: &TreeStore,
    staged: &dyn StagedDataView,
) -> Result<Vec<PlanStep>, ApplyError> {
    let index_id = index_catalog_id(index)?;
    let mut index_steps = vec![PlanStep::DeleteIndexSubtree {
        address: IndexAddress {
            index: index_id,
            keys: Vec::new(),
        },
    }];
    for_each_place_record(store, place, &mut |identity| {
        if let Some(step) = index_rebuild_entry_with_staged(
            index,
            place,
            identity,
            store,
            staged,
            Default::default(),
        )
        .map_err(|error| StoreError::Corruption {
            message: error.message,
        })? {
            index_steps.push(step);
        }
        Ok(())
    })?;
    Ok(index_steps)
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
/// it owns the rebuild and the scan stops there.
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
        let staged_data = PlanStepStagedData {
            steps: steps.as_slice(),
        };
        let index_steps = stage_index_subtree_rebuild(index, place, store, &staged_data)?;
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
                    address: DataAddress::from_resolved_parts(
                        sid.clone(),
                        identity.to_vec(),
                        path.to_vec(),
                    ),
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
