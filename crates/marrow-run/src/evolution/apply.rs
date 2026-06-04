//! Witness-validated evolution apply.

use marrow_check::evolution::{EvolutionWitness, Verdict};
use marrow_check::{
    CheckedProgram, CheckedRuntimeProgram, CheckedSavedMember, CheckedSavedMemberKind,
    CheckedSavedPlace, checked_saved_root_place,
};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

use crate::write_plan::{PlanStep, WritePlan};

use super::admission::{expected_retire_count, gate_obligations};
use super::backfill::{stage_default_backfill, stage_index_rebuild, stage_retire_deletes};
use super::scan::for_each_record;
use super::transform::stage_transform;
use super::validate::{assert_commit_pin, validate_witness};
use super::window::{FenceError, StampFacts, current_engine_profile, fence, metadata_stamp};

/// The scoped developer decision a destructive retire requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub catalog_ids: Vec<CatalogId>,
    pub populated: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub committed_commit_id: u64,
    pub catalog_epoch: u64,
    pub records_backfilled: usize,
    pub indexes_rebuilt: usize,
    pub records_retired: usize,
    pub records_transformed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// The program accepted no catalog, so it has no baseline epoch to advance from.
    /// Apply moves a store forward from an accepted epoch; with none accepted there is
    /// nothing to activate, and stamping a proposal epoch would invent a baseline.
    NoAcceptedCatalog,
    Drift,
    StoreCommitDrift {
        pinned: Option<u64>,
        found: Option<u64>,
    },
    NotActivatable,
    MaintenanceRequired,
    ApprovalRequired {
        catalog_id: CatalogId,
        populated: usize,
    },
    ApprovalMismatch,
    PlanMismatch {
        expected: usize,
        staged: usize,
    },
    TransformBodyFaulted {
        target: CatalogId,
        reason: String,
    },
    Fenced(FenceError),
    Store(StoreError),
}

impl From<FenceError> for ApplyError {
    fn from(error: FenceError) -> Self {
        ApplyError::Fenced(error)
    }
}

impl From<StoreError> for ApplyError {
    fn from(error: StoreError) -> Self {
        ApplyError::Store(error)
    }
}

/// Apply the durable work `witness` describes against `store`, atomically.
pub fn apply(
    witness: &EvolutionWitness,
    program: &CheckedProgram,
    store: &TreeStore,
    maintenance: bool,
    approval: Option<&Approval>,
) -> Result<ApplyOutcome, ApplyError> {
    let Some(accepted_epoch) = program.catalog.accepted_epoch else {
        return Err(ApplyError::NoAcceptedCatalog);
    };
    validate_witness(witness, program, store)?;
    // Fence against the shape the store already holds, not the shape apply is about to
    // stamp: an evolution that changes shape must verify the store still sits at its
    // pre-apply shape before advancing it, or every shape-changing apply would fence
    // itself as drift. The new shape is what apply stamps once the fence passes.
    let expected_digest = witness.store_source_digest.clone().unwrap_or_default();
    fence(
        Some(accepted_epoch),
        &expected_digest,
        &current_engine_profile(),
        store,
    )?;
    gate_obligations(witness, maintenance, approval)?;

    let target_epoch = witness
        .proposal_catalog
        .as_ref()
        .map(|catalog| catalog.epoch)
        .unwrap_or(witness.accepted_catalog.epoch);

    let mut staged = StagedWork::default();
    let mut steps = Vec::new();
    let places = source_places(program);
    let runtime = program.runtime();
    for obligation in &witness.verdicts {
        stage_obligation(
            &obligation.catalog_id,
            &obligation.verdict,
            &places,
            store,
            &mut steps,
            &mut staged,
            program,
            &runtime,
        )?;
    }

    if staged.records_backfilled != witness.counts.records_to_backfill {
        return Err(ApplyError::PlanMismatch {
            expected: witness.counts.records_to_backfill,
            staged: staged.records_backfilled,
        });
    }

    let approved_retire = expected_retire_count(witness);
    if staged.records_retired != approved_retire {
        return Err(ApplyError::PlanMismatch {
            expected: approved_retire,
            staged: staged.records_retired,
        });
    }

    if staged.records_transformed != witness.counts.records_to_transform {
        return Err(ApplyError::PlanMismatch {
            expected: witness.counts.records_to_transform,
            staged: staged.records_transformed,
        });
    }

    // Nothing to write and the store already sits at the target epoch: applying again
    // would only churn the commit id and restamp the same epoch. Leave the store
    // untouched and report the current commit id rather than advancing it.
    let current_epoch = store.read_catalog_epoch()?;
    if steps.is_empty() && current_epoch == Some(target_epoch) {
        return Ok(ApplyOutcome {
            committed_commit_id: witness.store_commit_id.unwrap_or(0),
            catalog_epoch: target_epoch,
            records_backfilled: staged.records_backfilled,
            indexes_rebuilt: staged.indexes_rebuilt,
            records_retired: staged.records_retired,
            records_transformed: staged.records_transformed,
        });
    }

    let commit_id = witness.store_commit_id.unwrap_or(0) + 1;
    steps.push(metadata_stamp(StampFacts {
        catalog_epoch: target_epoch,
        commit_id,
        source_digest: witness.source_digest.clone(),
        changed_root_catalog_ids: witness.changed_root_catalog_ids.clone(),
        changed_index_catalog_ids: witness.changed_index_catalog_ids.clone(),
    }));
    commit_apply_plan(witness, store, steps)?;

    Ok(ApplyOutcome {
        committed_commit_id: commit_id,
        catalog_epoch: target_epoch,
        records_backfilled: staged.records_backfilled,
        indexes_rebuilt: staged.indexes_rebuilt,
        records_retired: staged.records_retired,
        records_transformed: staged.records_transformed,
    })
}

fn commit_apply_plan(
    witness: &EvolutionWitness,
    store: &TreeStore,
    steps: Vec<PlanStep>,
) -> Result<(), ApplyError> {
    store.begin()?;
    let result = (|| {
        assert_commit_pin(witness, store)?;
        WritePlan { steps }.commit(store, true)?;
        Ok(())
    })();
    match result {
        Ok(()) => match store.commit() {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = store.rollback();
                Err(ApplyError::Store(error))
            }
        },
        Err(error) => {
            let _ = store.rollback();
            Err(error)
        }
    }
}

#[derive(Default)]
pub(super) struct StagedWork {
    pub(super) records_backfilled: usize,
    pub(super) indexes_rebuilt: usize,
    pub(super) records_retired: usize,
    pub(super) records_transformed: usize,
}

#[allow(clippy::too_many_arguments)]
fn stage_obligation(
    catalog_id: &CatalogId,
    verdict: &Verdict,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    steps: &mut Vec<PlanStep>,
    staged: &mut StagedWork,
    program: &CheckedProgram,
    runtime: &CheckedRuntimeProgram,
) -> Result<(), ApplyError> {
    match verdict {
        Verdict::Default { value } => {
            stage_default_backfill(catalog_id, value, places, store, steps, staged)
        }
        Verdict::DerivedRebuild => stage_index_rebuild(catalog_id, places, store, steps, staged),
        Verdict::DestructiveDecisionRequired { .. } => {
            stage_retire_deletes(catalog_id, places, store, steps, staged)
        }
        Verdict::Transform { .. } => {
            stage_transform(catalog_id, program, runtime, places, store, steps, staged)
        }
        Verdict::NoOp
        | Verdict::CatalogOnly
        | Verdict::Deprecated
        | Verdict::DataProof
        | Verdict::RepairRequired { .. } => Ok(()),
    }
}

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

pub(super) fn locate_member(
    place: &CheckedSavedPlace,
    catalog_id: &CatalogId,
) -> Option<MemberLocation> {
    let mut steps = Vec::new();
    locate_in(&place.root_members, &mut steps, catalog_id).then_some(MemberLocation { steps })
}

pub(super) struct MemberLocation {
    pub(super) steps: Vec<PathStep>,
}

pub(super) enum PathStep {
    Member(CatalogId),
    Layer(CatalogId),
}

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

pub(super) fn for_each_place_record(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
) -> Result<(), ApplyError> {
    let store_id = store_id(place)?;
    for_each_record(store, &store_id, place.identity_keys.len(), visit)?;
    Ok(())
}
