//! Crash-resume verification for a stamped activation.
//!
//! The accepted-catalog file is the last activation step. If a crash leaves the store
//! stamped at the proposal epoch while the file still names the prior epoch, resume may
//! publish the stored proposal only after proving the stamped data and index effects are
//! still visible. The receipt fields bind the exact witness identity; the effect checks
//! below use the same staging owners as apply where an operation's final state can be
//! re-derived from the current store.

use std::collections::BTreeMap;

use marrow_check::evolution::{default_value_for_bound_member, preview};
use marrow_check::{
    CatalogEntryKind, CatalogLifecycle, CheckedProgram, CheckedRuntimeProgram, CheckedSavedIndex,
    CheckedSavedPlace, StoreLeafKind,
};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{
    ActivationDefaultRecordCount, CommitMetadata, DataPathSegment, IndexCursor, TreeStore,
};
use marrow_syntax::SourceSpan;

use crate::index_maintenance::{EmptyStagedData, index_rebuild_entry_with_staged};
use crate::value::decode_leaf;
use crate::write_plan::PlanStep;

use super::admission::normalized_retire_approval;
use super::apply::{ApplyError, Approval, PathStep, StagedWork, for_each_place_record, store_id};
use super::backfill::{locations, stage_retire_deletes, visit_member_cell_paths};
use super::transform::{TransformStage, stage_transform};

const INDEX_SCAN_PAGE: usize = 128;

type IndexEntryKey = (Vec<SavedKey>, Vec<SavedKey>);
type IndexEntries = BTreeMap<IndexEntryKey, Vec<u8>>;
type DefaultCellKey = (CatalogId, CatalogId, Vec<SavedKey>, Vec<DataPathSegment>);

/// Prove a store-stamped activation is complete before crash resume publishes the
/// accepted-catalog file. Any missing receipt field, changed witness fingerprint, or
/// absent data/index effect is drift and must fail closed.
pub fn verify_activation_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    commit: &CommitMetadata,
    approval: Option<&Approval>,
) -> Result<(), ApplyError> {
    let (witness, _diagnostics) = preview(program, store)?;
    let Some(proposal) = &witness.proposal_catalog else {
        return Err(ApplyError::Drift);
    };
    if commit.catalog_epoch != proposal.epoch
        || commit.source_digest.is_empty()
        || commit.source_digest != witness.source_digest
        || commit.activation_evolution_digest.is_empty()
        || commit.activation_evolution_digest != witness.evolution_digest
        || commit.activation_proposal_catalog_digest.as_deref() != Some(proposal.digest.as_str())
        || commit.changed_root_catalog_ids != witness.changed_root_catalog_ids
        || commit.changed_index_catalog_ids != witness.changed_index_catalog_ids
    {
        return Err(ApplyError::Drift);
    }

    let runtime = program.runtime();
    let places = marrow_check::checked_activation_root_places(program);
    let defaults = verify_default_completion(program, store, &places)?;
    let records_transformed = verify_transform_completion(program, store, &places)?;
    verify_retire_completion(program, store, commit, &places, approval)?;
    let indexes_rebuilt = verify_index_completion(program, store, commit, &places)?;

    verify_default_receipt(&runtime, store, &defaults, commit)?;
    if commit.activation_records_transformed != records_transformed as u64
        || commit.activation_indexes_rebuilt != indexes_rebuilt as u64
    {
        return Err(ApplyError::Drift);
    }

    Ok(())
}

struct DefaultCompletion {
    catalog_id: CatalogId,
    proposal_new: bool,
    target_records: u64,
    default_value: Vec<u8>,
    target_leaf: StoreLeafKind,
    locations: Vec<DefaultLocation>,
}

struct DefaultLocation {
    place: CheckedSavedPlace,
    store_id: CatalogId,
    steps: Vec<PathStep>,
}

fn verify_default_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    places: &[CheckedSavedPlace],
) -> Result<Vec<DefaultCompletion>, ApplyError> {
    let runtime = program.runtime();
    let mut completed = Vec::new();
    for default in &program.catalog.evolve_defaults {
        let target = catalog_id(&default.catalog_id)?;
        let proposal_new = !accepted_resource_member(program, &target);
        let value = default_value_for_bound_member(program, &default.catalog_id, &default.value)
            .ok_or(ApplyError::Drift)?
            .map_err(|_| ApplyError::Drift)?;
        let mut target_leaf = None;
        let mut target_records = 0u64;
        let mut default_locations = Vec::new();
        for (place, location) in locations(places, &target) {
            let sid = store_id(place)?;
            let leaf = location.leaf.clone().ok_or(ApplyError::Drift)?;
            if let Some(existing) = &target_leaf {
                if existing != &leaf {
                    return Err(ApplyError::Drift);
                }
            } else {
                target_leaf = Some(leaf.clone());
            }
            let steps = location.steps;
            for_each_place_record(store, place, &mut |identity| {
                visit_member_cell_paths(store, &sid, identity, &steps, &mut |path| {
                    let current = store.read_data_value(&sid, identity, path)?;
                    target_records += 1;
                    if proposal_new {
                        if current.as_deref() != Some(value.encoded.as_slice()) {
                            return Err(incomplete());
                        }
                    } else if !stored_default_cell_valid(&runtime, &leaf, &current) {
                        return Err(incomplete());
                    }
                    Ok(())
                })?;
                Ok(())
            })?;
            default_locations.push(DefaultLocation {
                place: place.clone(),
                store_id: sid,
                steps,
            });
        }
        let target_leaf = target_leaf.ok_or(ApplyError::Drift)?;
        completed.push(DefaultCompletion {
            catalog_id: target,
            proposal_new,
            target_records,
            default_value: value.encoded,
            target_leaf,
            locations: default_locations,
        });
    }
    Ok(completed)
}

fn stored_default_cell_valid(
    runtime: &CheckedRuntimeProgram,
    leaf: &StoreLeafKind,
    current: &Option<Vec<u8>>,
) -> bool {
    let Some(bytes) = current.as_deref() else {
        return false;
    };
    decode_leaf(runtime, bytes, leaf).is_some()
}

fn verify_default_receipt(
    runtime: &CheckedRuntimeProgram,
    store: &TreeStore,
    defaults: &[DefaultCompletion],
    commit: &CommitMetadata,
) -> Result<(), ApplyError> {
    let expected = sorted_default_targets(defaults);
    let recorded = sorted_default_counts(commit.activation_default_records_by_id.clone());
    if expected.len() != recorded.len() {
        return Err(ApplyError::Drift);
    }
    let mut total_backfilled = 0u64;
    let mut cells_by_target: BTreeMap<CatalogId, u64> = BTreeMap::new();
    let mut backfilled_by_target: BTreeMap<CatalogId, u64> = BTreeMap::new();
    let mut recorded_cells = recorded_default_cells(commit)?;
    for default in &expected {
        for location in &default.locations {
            for_each_place_record(store, &location.place, &mut |identity| {
                visit_member_cell_paths(
                    store,
                    &location.store_id,
                    identity,
                    &location.steps,
                    &mut |path| {
                        let key = (
                            default.catalog_id.clone(),
                            location.store_id.clone(),
                            identity.to_vec(),
                            path.to_vec(),
                        );
                        let Some(backfilled) = recorded_cells.remove(&key) else {
                            return Err(incomplete());
                        };
                        let current = store.read_data_value(&location.store_id, identity, path)?;
                        if backfilled
                            && current.as_deref() != Some(default.default_value.as_slice())
                        {
                            return Err(incomplete());
                        }
                        if !backfilled
                            && !stored_default_cell_valid(runtime, &default.target_leaf, &current)
                        {
                            return Err(incomplete());
                        }
                        *cells_by_target
                            .entry(default.catalog_id.clone())
                            .or_insert(0) += 1;
                        if backfilled {
                            *backfilled_by_target
                                .entry(default.catalog_id.clone())
                                .or_insert(0) += 1;
                        }
                        Ok(())
                    },
                )?;
                Ok(())
            })?;
        }
    }
    if !recorded_cells.is_empty() {
        return Err(ApplyError::Drift);
    }
    for (expected, recorded) in expected.iter().zip(recorded.iter()) {
        if expected.catalog_id != recorded.catalog_id
            || expected.target_records != recorded.target_records
            || cells_by_target
                .get(&expected.catalog_id)
                .copied()
                .unwrap_or(0)
                != recorded.target_records
            || backfilled_by_target
                .get(&expected.catalog_id)
                .copied()
                .unwrap_or(0)
                != recorded.records_backfilled
            || recorded.records_backfilled > recorded.target_records
            || (expected.proposal_new && recorded.records_backfilled != expected.target_records)
        {
            return Err(ApplyError::Drift);
        }
        total_backfilled = total_backfilled
            .checked_add(recorded.records_backfilled)
            .ok_or(ApplyError::Drift)?;
    }
    if commit.activation_records_backfilled != total_backfilled {
        return Err(ApplyError::Drift);
    }
    Ok(())
}

fn recorded_default_cells(
    commit: &CommitMetadata,
) -> Result<BTreeMap<DefaultCellKey, bool>, ApplyError> {
    let mut cells = BTreeMap::new();
    for cell in &commit.activation_default_backfill_cells {
        let key = (
            cell.catalog_id.clone(),
            cell.store_id.clone(),
            cell.identity.clone(),
            cell.path.clone(),
        );
        if cells.insert(key, cell.backfilled).is_some() {
            return Err(ApplyError::Drift);
        }
    }
    Ok(cells)
}

fn verify_transform_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    places: &[CheckedSavedPlace],
) -> Result<usize, ApplyError> {
    let runtime = program.runtime();
    let mut completed = 0usize;
    for transform in &program.catalog.evolve_transforms {
        let Some(transform_catalog_id) = transform.catalog_id.as_deref() else {
            continue;
        };
        let target = catalog_id(transform_catalog_id)?;
        let mut steps = Vec::new();
        let mut staged = StagedWork::default();
        stage_transform(TransformStage {
            target_id: &target,
            witness_reads: &transform
                .reads
                .iter()
                .map(|read| catalog_id(read))
                .collect::<Result<Vec<_>, _>>()?,
            program,
            runtime: &runtime,
            places,
            store,
            steps: &mut steps,
            staged: &mut staged,
        })?;
        for step in steps {
            verify_write_data(store, step)?;
        }
        completed += staged.records_transformed;
    }
    Ok(completed)
}

fn verify_retire_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    commit: &CommitMetadata,
    places: &[CheckedSavedPlace],
    approval: Option<&Approval>,
) -> Result<(), ApplyError> {
    let retired = sorted_catalog_ids(retired_ids(program, CatalogEntryKind::ResourceMember));
    let expected = exact_retire_counts_u64(commit.activation_records_retired_by_id.clone())?;
    let recorded_ids: Vec<_> = expected.iter().map(|(id, _count)| id.clone()).collect();
    if recorded_ids != retired {
        return Err(ApplyError::Drift);
    }
    let destructive: Vec<_> = expected
        .iter()
        .filter(|(_id, count)| *count > 0)
        .cloned()
        .collect();
    let approved = approval
        .map(|approval| {
            normalized_retire_approval(approval)
                .into_iter()
                .map(|(id, count)| (id, count as u64))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if approved != destructive {
        return Err(ApplyError::Drift);
    }
    let expected_total = expected
        .iter()
        .try_fold(0u64, |total, (_id, count)| total.checked_add(*count))
        .ok_or(ApplyError::Drift)?;
    if commit.activation_records_retired != expected_total {
        return Err(ApplyError::Drift);
    }
    for id in &retired {
        let mut steps = Vec::new();
        let mut staged = StagedWork::default();
        stage_retire_deletes(id, places, store, &mut steps, &mut staged)?;
        if !steps.is_empty() || staged.records_retired != 0 {
            return Err(ApplyError::Drift);
        }
    }
    Ok(())
}

fn verify_index_completion(
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
    let mut expected = BTreeMap::new();
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
            expected.insert((address.keys, identity), value);
        }
        Ok(())
    })?;

    let actual = scan_index_entries(store, &index_id, index.keys.len())?;
    if actual != expected {
        return Err(ApplyError::Drift);
    }
    Ok(())
}

fn verify_write_data(store: &TreeStore, step: PlanStep) -> Result<(), ApplyError> {
    let PlanStep::WriteData { address, value } = step else {
        return Err(ApplyError::Drift);
    };
    let current = store.read_data_value(&address.store, &address.identity, &address.path)?;
    if current.as_deref() != Some(value.as_slice()) {
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

fn retired_ids(program: &CheckedProgram, kind: CatalogEntryKind) -> Vec<CatalogId> {
    program
        .catalog
        .proposal
        .as_ref()
        .into_iter()
        .flat_map(|proposal| proposal.entries.iter())
        .filter(|entry| {
            entry.kind == kind
                && retired_this_proposal(program, entry.stable_id.as_str(), entry.lifecycle, kind)
        })
        .filter_map(|entry| CatalogId::new(entry.stable_id.clone()).ok())
        .collect()
}

fn retired_this_proposal(
    program: &CheckedProgram,
    stable_id: &str,
    lifecycle: CatalogLifecycle,
    kind: CatalogEntryKind,
) -> bool {
    lifecycle == CatalogLifecycle::Reserved
        && program.catalog.proposal.is_some()
        && program.catalog.accepted_entries.iter().any(|accepted| {
            accepted.kind == kind
                && accepted.stable_id == stable_id
                && accepted.lifecycle == CatalogLifecycle::Active
        })
}

fn accepted_resource_member(program: &CheckedProgram, catalog_id: &CatalogId) -> bool {
    program.catalog.accepted_entries.iter().any(|entry| {
        entry.kind == CatalogEntryKind::ResourceMember && entry.stable_id == catalog_id.as_str()
    })
}

fn sorted_catalog_ids(mut ids: Vec<CatalogId>) -> Vec<CatalogId> {
    ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    ids.dedup();
    ids
}

fn exact_retire_counts_u64(
    mut counts: Vec<(CatalogId, u64)>,
) -> Result<Vec<(CatalogId, u64)>, ApplyError> {
    counts.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    if counts
        .windows(2)
        .any(|pair| pair[0].0.as_str() == pair[1].0.as_str())
    {
        return Err(ApplyError::Drift);
    }
    Ok(counts)
}

fn sorted_default_targets(defaults: &[DefaultCompletion]) -> Vec<&DefaultCompletion> {
    let mut defaults: Vec<_> = defaults.iter().collect();
    defaults.sort_by(|a, b| a.catalog_id.as_str().cmp(b.catalog_id.as_str()));
    defaults
}

fn sorted_default_counts(
    mut counts: Vec<ActivationDefaultRecordCount>,
) -> Vec<ActivationDefaultRecordCount> {
    counts.sort_by(|a, b| a.catalog_id.as_str().cmp(b.catalog_id.as_str()));
    counts
}

fn index_is_empty(store: &TreeStore, index: &CatalogId) -> Result<bool, ApplyError> {
    Ok(store.index_child_keys(index, &[])?.is_empty()
        && store.scan_index_tuple(index, &[], 1)?.entries.is_empty())
}

fn scan_index_entries(
    store: &TreeStore,
    index: &CatalogId,
    key_len: usize,
) -> Result<IndexEntries, ApplyError> {
    let mut entries = BTreeMap::new();
    let mut prefix = Vec::new();
    scan_index_prefix(store, index, key_len, &mut prefix, &mut entries)?;
    Ok(entries)
}

fn scan_index_prefix(
    store: &TreeStore,
    index: &CatalogId,
    key_len: usize,
    prefix: &mut Vec<SavedKey>,
    entries: &mut IndexEntries,
) -> Result<(), ApplyError> {
    if prefix.len() == key_len {
        scan_index_tuple_entries(store, index, prefix, entries)?;
        return Ok(());
    }
    for key in store.index_child_keys(index, prefix)? {
        prefix.push(key);
        scan_index_prefix(store, index, key_len, prefix, entries)?;
        prefix.pop();
    }
    Ok(())
}

fn scan_index_tuple_entries(
    store: &TreeStore,
    index: &CatalogId,
    keys: &[SavedKey],
    entries: &mut IndexEntries,
) -> Result<(), ApplyError> {
    let mut cursor: Option<IndexCursor> = None;
    loop {
        let page = match &cursor {
            Some(cursor) => store.scan_index_tuple_after(index, keys, cursor, INDEX_SCAN_PAGE)?,
            None => store.scan_index_tuple(index, keys, INDEX_SCAN_PAGE)?,
        };
        for entry in page.entries {
            entries.insert((keys.to_vec(), entry.identity), entry.value);
        }
        let Some(next) = page.cursor else {
            break;
        };
        cursor = Some(next);
    }
    Ok(())
}

fn catalog_id(raw: &str) -> Result<CatalogId, ApplyError> {
    CatalogId::new(raw.to_string()).map_err(|_| {
        ApplyError::Store(StoreError::Corruption {
            message: "activation completion saw an invalid catalog id".to_string(),
        })
    })
}

fn incomplete() -> StoreError {
    StoreError::Corruption {
        message: "activation completion evidence is missing a committed effect".to_string(),
    }
}
