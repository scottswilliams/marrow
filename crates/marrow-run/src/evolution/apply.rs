//! Witness-validated apply: stage and atomically commit the durable work an
//! evolution witness describes, refusing any drift between the witness and the live
//! store.
//!
//! Apply is the only writer that crosses the evolution boundary. Its input is an
//! [`EvolutionWitness`] a prior read-only preview produced; its contract is that the
//! store it commits against still matches that witness exactly. It re-runs the same
//! production discharge through [`preview`] and compares the recomputed witness for
//! equality, so source drift, catalog drift, a snapshot data change, and a count
//! change are all caught by one comparison. It then asserts the pinned store commit id
//! explicitly, so a concurrent writer that advanced the store after preview is caught
//! even though witness equality would also catch it.
//!
//! Only an activatable witness applies. A transform obligation or a fail-closed repair
//! aborts. A destructive retire applies only under the maintenance gate with a scoped
//! approval whose catalog ids and populated count match the witness exactly.
//!
//! The work is staged into one [`WritePlan`] and committed in a single transaction
//! together with the metadata stamp, so the store's catalog epoch never advances
//! without the data the new epoch describes. On any error the transaction rolls back,
//! leaving the store byte-for-byte at its pre-apply commit, and a resumed apply
//! re-previews to the same witness and succeeds: backfilling a member a record already
//! carries is a no-op.

use marrow_check::evolution::{EvolutionWitness, Verdict, preview};
use marrow_check::{
    CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, CheckedSavedPlace,
    checked_saved_root_place,
};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, EngineProfile, TreeStore};

use crate::write_plan::{PlanStep, WritePlan};

use super::backfill::{stage_default_backfill, stage_index_rebuild, stage_retire_deletes};
use super::scan::for_each_record;

/// The developer decision a destructive retire requires: the exact catalog ids the
/// retire drops and the populated record count recorded at preview. Apply accepts the
/// drop only when both match the witness, so a store that changed under the decision
/// is refused rather than silently over- or under-dropping.
///
/// The count is a single number rather than one per catalog id because v0.1 activates a
/// single destructive retire per apply; the gate matches that one count against the one
/// destructive obligation and fails closed if a second appears. Independent multi-retire
/// approvals are a later concern and are not approximated here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub catalog_ids: Vec<CatalogId>,
    pub populated: usize,
}

/// What an applied evolution did: the committed store commit id, the catalog epoch the
/// store now reports, and counts of the durable work staged. The counts let a caller
/// confirm the apply matched the witness it consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub committed_commit_id: u64,
    pub catalog_epoch: u64,
    pub records_backfilled: usize,
    pub indexes_rebuilt: usize,
    pub records_retired: usize,
}

/// Why an apply was refused before or during commit. Every variant leaves the store
/// unchanged: the drift and gate checks run before any write is staged, and a store
/// error rolls the whole transaction back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// The witness no longer matches what the live source and store discharge to:
    /// source, catalog, snapshot, or count drift. The store is unchanged.
    Drift,
    /// The store's latest commit id is not the one the witness pinned, so a concurrent
    /// writer advanced the store after the preview.
    StoreCommitDrift {
        pinned: Option<u64>,
        found: Option<u64>,
    },
    /// The witness carries a blocking obligation a transform or a repair must resolve
    /// before activation; apply cannot discharge it.
    NotActivatable,
    /// A destructive retire was reached without the maintenance gate.
    MaintenanceRequired,
    /// A destructive retire was reached with no approval.
    ApprovalRequired {
        catalog_id: CatalogId,
        populated: usize,
    },
    /// An approval did not match the witness: its catalog ids or populated count
    /// differ from what was recorded.
    ApprovalMismatch,
    /// Apply could not stage the exact work the witness counted, so it refuses rather
    /// than commit a partial or mismatched plan. The store is unchanged.
    PlanMismatch { expected: usize, staged: usize },
    /// A store operation failed; the transaction rolled back and the store is unchanged.
    Store(StoreError),
}

impl From<StoreError> for ApplyError {
    fn from(error: StoreError) -> Self {
        ApplyError::Store(error)
    }
}

/// Apply the durable work `witness` describes against `store`, atomically. The store
/// must still match the witness exactly; a destructive retire additionally requires
/// `maintenance` and a scoped `approval`. On success the store carries the staged data
/// and a commit stamp at the proposal epoch; on any failure the store is unchanged.
pub fn apply(
    witness: &EvolutionWitness,
    program: &CheckedProgram,
    store: &TreeStore,
    maintenance: bool,
    approval: Option<&Approval>,
) -> Result<ApplyOutcome, ApplyError> {
    let current = preview(program, store).map_err(ApplyError::Store)?.0;
    if current != *witness {
        return Err(ApplyError::Drift);
    }
    assert_commit_pin(witness, store)?;
    gate_obligations(witness, maintenance, approval)?;

    // The epoch the store advances to: the proposal epoch when source evolved the
    // catalog, else the accepted epoch when only old data needs to catch up to an
    // already-accepted schema.
    let target_epoch = witness
        .proposal_catalog
        .as_ref()
        .map(|catalog| catalog.epoch)
        .unwrap_or(witness.accepted_catalog.epoch);

    let mut staged = StagedWork::default();
    let mut steps = Vec::new();
    let places = source_places(program);
    for obligation in &witness.verdicts {
        stage_obligation(
            &obligation.catalog_id,
            &obligation.verdict,
            &places,
            store,
            &mut steps,
            &mut staged,
        )?;
    }

    if staged.records_backfilled != witness.counts.records_to_backfill {
        return Err(ApplyError::PlanMismatch {
            expected: witness.counts.records_to_backfill,
            staged: staged.records_backfilled,
        });
    }

    // A destructive retire drops exactly the populated cells the witness recorded and
    // the developer approved. The re-scan that staged the deletes must find that count;
    // a mismatch means the store changed under the decision, so refuse rather than drop
    // a different set of cells. This mirrors the backfill guard on the deleting path.
    let approved_retire = expected_retire_count(witness);
    if staged.records_retired != approved_retire {
        return Err(ApplyError::PlanMismatch {
            expected: approved_retire,
            staged: staged.records_retired,
        });
    }

    let commit_id = witness.store_commit_id.unwrap_or(0) + 1;
    steps.push(metadata_stamp(witness, target_epoch, commit_id));
    WritePlan { steps }.commit(store, false)?;

    Ok(ApplyOutcome {
        committed_commit_id: commit_id,
        catalog_epoch: target_epoch,
        records_backfilled: staged.records_backfilled,
        indexes_rebuilt: staged.indexes_rebuilt,
        records_retired: staged.records_retired,
    })
}

/// The running tally of the durable work staged, used to confirm the plan matches the
/// witness counts and to report the outcome.
#[derive(Default)]
pub(super) struct StagedWork {
    pub(super) records_backfilled: usize,
    pub(super) indexes_rebuilt: usize,
    pub(super) records_retired: usize,
}

/// Assert the store's latest commit id is exactly the one the witness pinned. Witness
/// equality already covers this through the recomputed preview, but pinning the commit
/// id explicitly states the concurrency contract apply relies on.
fn assert_commit_pin(witness: &EvolutionWitness, store: &TreeStore) -> Result<(), ApplyError> {
    let found = store.read_commit_metadata()?.map(|commit| commit.commit_id);
    if found != witness.store_commit_id {
        return Err(ApplyError::StoreCommitDrift {
            pinned: witness.store_commit_id,
            found,
        });
    }
    Ok(())
}

/// Refuse a witness apply cannot discharge, and enforce the maintenance gate and the
/// scoped approval for every destructive retire. A transform or a repair is blocking;
/// a destructive retire needs maintenance plus an approval matching the witness.
fn gate_obligations(
    witness: &EvolutionWitness,
    maintenance: bool,
    approval: Option<&Approval>,
) -> Result<(), ApplyError> {
    for obligation in &witness.verdicts {
        match &obligation.verdict {
            Verdict::TypedTransformRequired | Verdict::RepairRequired { .. } => {
                return Err(ApplyError::NotActivatable);
            }
            Verdict::DestructiveDecisionRequired { populated } => {
                if !maintenance {
                    return Err(ApplyError::MaintenanceRequired);
                }
                let approval = approval.ok_or_else(|| ApplyError::ApprovalRequired {
                    catalog_id: obligation.catalog_id.clone(),
                    populated: *populated,
                })?;
                if !approval.catalog_ids.contains(&obligation.catalog_id)
                    || approval.populated != *populated
                {
                    return Err(ApplyError::ApprovalMismatch);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Stage the durable work for one obligation. A defaulting obligation backfills the
/// member, a derived rebuild writes the index entries, an approved destructive retire
/// deletes the member subtree per record; every other verdict touches no data.
fn stage_obligation(
    catalog_id: &CatalogId,
    verdict: &Verdict,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    steps: &mut Vec<PlanStep>,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    match verdict {
        Verdict::Default { value } => {
            stage_default_backfill(catalog_id, value, places, store, steps, staged)
        }
        Verdict::DerivedRebuild => stage_index_rebuild(catalog_id, places, store, steps, staged),
        Verdict::DestructiveDecisionRequired { .. } => {
            stage_retire_deletes(catalog_id, places, store, steps, staged)
        }
        Verdict::NoOp
        | Verdict::CatalogOnly
        | Verdict::Deprecated
        | Verdict::DataProof
        | Verdict::TypedTransformRequired
        | Verdict::RepairRequired { .. } => Ok(()),
    }
}

/// The metadata stamp that advances the store: the target catalog epoch, the engine
/// profile the binary expects, and the commit metadata that records the new epoch and
/// the catalog ids the change touched. The discharge already partitioned those ids into
/// data roots and store indexes by catalog entry kind, so the stamp carries them
/// through directly rather than re-classifying a dropped index id from current source.
/// It is the last step so it commits in the same transaction as the data it describes.
fn metadata_stamp(witness: &EvolutionWitness, target_epoch: u64, commit_id: u64) -> PlanStep {
    let layout_epoch = witness.layout_epoch.unwrap_or(0);
    let profile = EngineProfile::new(layout_epoch);
    let commit = CommitMetadata {
        commit_id,
        catalog_epoch: target_epoch,
        layout_epoch,
        engine_profile_digest: profile.digest_bytes(),
        changed_root_catalog_ids: witness.changed_root_catalog_ids.clone(),
        changed_index_catalog_ids: witness.changed_index_catalog_ids.clone(),
    };
    PlanStep::StampMetadata {
        catalog_epoch: target_epoch,
        profile,
        commit,
    }
}

/// The total populated cell count every destructive retire the witness carries
/// recorded. The staged deletes must match this sum, so a store that changed under the
/// approved decision is refused rather than dropping a different set of cells.
fn expected_retire_count(witness: &EvolutionWitness) -> usize {
    witness
        .verdicts
        .iter()
        .map(|obligation| match &obligation.verdict {
            Verdict::DestructiveDecisionRequired { populated } => *populated,
            _ => 0,
        })
        .sum()
}

/// The checked saved place for every saved-store root current source declares, in
/// declaration order. Apply re-derives every data path from these places keyed by
/// catalog id, so it never carries record identities or paths in the witness.
fn source_places(program: &CheckedProgram) -> Vec<CheckedSavedPlace> {
    let mut places = Vec::new();
    for module in &program.modules {
        for store in &module.stores {
            if let Some(place) = checked_saved_root_place(program, &store.root, Default::default())
                && !place.store_catalog_id.is_empty()
            {
                places.push(place);
            }
        }
    }
    places
}

/// The store catalog id of a place, validated once.
pub(super) fn store_id(place: &CheckedSavedPlace) -> Result<CatalogId, ApplyError> {
    CatalogId::new(place.store_catalog_id.clone()).map_err(|_| {
        ApplyError::Store(StoreError::Corruption {
            message: "evolution apply saw an invalid store catalog id".to_string(),
        })
    })
}

/// Find the member with `catalog_id` anywhere in a place's member tree, returning the
/// ordered path of descent steps from the record node down to the member's cell. This
/// reads the already-checked member facts; it does not re-classify a store path.
pub(super) fn locate_member(
    place: &CheckedSavedPlace,
    catalog_id: &CatalogId,
) -> Option<MemberLocation> {
    let mut steps = Vec::new();
    locate_in(&place.root_members, &mut steps, catalog_id).then_some(MemberLocation { steps })
}

/// The path from a record node to a member cell, as the ordered descent steps the
/// store walk follows. A keyed-layer step pages each existing entry; a member step
/// descends a single named cell. The terminal step names the member's own cell.
pub(super) struct MemberLocation {
    pub(super) steps: Vec<PathStep>,
}

/// One step of descent toward a member cell.
pub(super) enum PathStep {
    /// Descend a single named cell (a top-level field, an unkeyed group, or the
    /// terminal leaf).
    Member(CatalogId),
    /// Descend a keyed layer: page each existing entry under this member and recurse
    /// into the entry with its key appended.
    Layer(CatalogId),
}

/// Append the descent steps to `target`, returning whether it was found. A keyed member
/// contributes a `Layer` step; every other member contributes a `Member` step.
fn locate_in(
    members: &[CheckedSavedMember],
    steps: &mut Vec<PathStep>,
    target: &CatalogId,
) -> bool {
    for member in members {
        if member.catalog_id.is_empty() {
            continue;
        }
        let Ok(member_id) = CatalogId::new(member.catalog_id.clone()) else {
            continue;
        };
        let keyed = !member.key_params.is_empty();
        let step = if keyed {
            PathStep::Layer(member_id.clone())
        } else {
            PathStep::Member(member_id.clone())
        };
        steps.push(step);
        if member_id == *target {
            return true;
        }
        if matches!(member.kind, CheckedSavedMemberKind::Group)
            && locate_in(&member.group_members, steps, target)
        {
            return true;
        }
        steps.pop();
    }
    false
}

/// Visit every record identity under a place's store root, one full identity tuple at
/// a time, through the shared paged scan. Apply re-scans rather than trusting any
/// identity list in the witness.
pub(super) fn for_each_place_record(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
) -> Result<(), ApplyError> {
    let store_id = store_id(place)?;
    for_each_record(store, &store_id, place.identity_keys.len(), visit)?;
    Ok(())
}
