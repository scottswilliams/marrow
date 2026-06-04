//! Witness-validated evolution apply.

use marrow_check::evolution::{EvolutionWitness, Verdict};
use marrow_check::{
    CatalogEntryKind, CatalogLifecycle, CheckedProgram, CheckedRuntimeProgram, CheckedSavedMember,
    CheckedSavedMemberKind, CheckedSavedPlace, StoreLeafKind, checked_activation_root_places,
};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{ActivationDefaultRecordCount, TreeStore};

use crate::write_plan::{PlanStep, WritePlan};

use super::admission::{expected_retire_counts, gate_obligations};
use super::backfill::{
    stage_default_backfill, stage_default_presence_receipt, stage_index_drop, stage_index_rebuild,
    stage_retire_deletes,
};
use super::transform::{TransformStage, stage_transform};
use super::validate::{assert_commit_pin, validate_witness};
use super::window::{
    ActivationStampFacts, FenceError, StampFacts, current_engine_profile, fence, metadata_stamp,
};

/// The scoped developer decision a destructive retire requires. Each entry names one
/// retired catalog id and the exact populated count the developer approved dropping for
/// it. Admission matches every entry against the witness per-id, so an approval is in
/// scope only when its ids and their counts equal the witness destructive set exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub retires: Vec<(CatalogId, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub committed_commit_id: u64,
    pub catalog_epoch: u64,
    pub records_backfilled: usize,
    pub indexes_rebuilt: usize,
    pub records_retired: usize,
    pub records_transformed: usize,
    pub receipt: ActivationReceipt,
}

/// Evidence for one committed activation. It records the witness fingerprints and
/// committed counts, not executable steps or migration history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationReceipt {
    pub commit_id: u64,
    pub catalog_epoch: u64,
    pub source_digest: String,
    pub evolution_digest: String,
    pub accepted_catalog_digest: String,
    pub proposal_catalog_digest: Option<String>,
    pub store_commit_id_before: Option<u64>,
    pub changed_root_catalog_ids: Vec<CatalogId>,
    pub changed_index_catalog_ids: Vec<CatalogId>,
    pub records_backfilled: usize,
    pub default_records_by_id: Vec<ActivationDefaultRecordCount>,
    pub indexes_rebuilt: usize,
    pub records_retired: usize,
    pub records_retired_by_id: Vec<(CatalogId, usize)>,
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
    let current_epoch = store.read_catalog_epoch()?;

    let mut staged = StagedWork::default();
    let mut steps = Vec::new();
    let places = checked_activation_root_places(program);
    let runtime = program.runtime();
    let ctx = StageCtx {
        places: &places,
        store,
        program,
        runtime: &runtime,
        reject_existing_proposal_defaults: current_epoch != Some(target_epoch),
    };
    let mut index_rebuilds = Vec::new();
    for obligation in &witness.verdicts {
        if matches!(obligation.verdict, Verdict::DerivedRebuild) {
            index_rebuilds.push(obligation.catalog_id.clone());
            continue;
        }
        stage_obligation(
            &ctx,
            &obligation.catalog_id,
            &obligation.verdict,
            &mut steps,
            &mut staged,
        )?;
    }
    for catalog_id in index_rebuilds {
        stage_index_rebuild(&catalog_id, &places, store, &mut steps, &mut staged)?;
    }
    stage_redundant_default_receipts(program, &places, store, &mut staged)?;

    if staged.records_backfilled != witness.counts.records_to_backfill {
        return Err(ApplyError::PlanMismatch {
            expected: witness.counts.records_to_backfill,
            staged: staged.records_backfilled,
        });
    }

    let destructive_retire_counts = expected_retire_counts(witness);
    let approved_retire = destructive_retire_counts
        .iter()
        .map(|(_id, count)| *count)
        .sum::<usize>();
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
    if steps.is_empty() && current_epoch == Some(target_epoch) {
        let committed_commit_id = witness.store_commit_id.unwrap_or(0);
        return Ok(ApplyOutcome {
            committed_commit_id,
            catalog_epoch: target_epoch,
            records_backfilled: staged.records_backfilled,
            indexes_rebuilt: staged.indexes_rebuilt,
            records_retired: staged.records_retired,
            records_transformed: staged.records_transformed,
            receipt: activation_receipt(
                witness,
                committed_commit_id,
                target_epoch,
                &staged,
                &retire_receipt_counts(program, &destructive_retire_counts)?,
            ),
        });
    }

    let commit_id = witness.store_commit_id.unwrap_or(0) + 1;
    let retire_receipt_counts = retire_receipt_counts(program, &destructive_retire_counts)?;
    steps.push(metadata_stamp(StampFacts {
        catalog_epoch: target_epoch,
        commit_id,
        source_digest: witness.source_digest.clone(),
        changed_root_catalog_ids: witness.changed_root_catalog_ids.clone(),
        changed_index_catalog_ids: witness.changed_index_catalog_ids.clone(),
        activation: Some(ActivationStampFacts {
            evolution_digest: witness.evolution_digest.clone(),
            proposal_catalog_digest: witness
                .proposal_catalog
                .as_ref()
                .map(|catalog| catalog.digest.clone()),
            records_backfilled: staged.records_backfilled as u64,
            default_records_by_id: staged.default_records_by_id.clone(),
            indexes_rebuilt: staged.indexes_rebuilt as u64,
            records_retired: staged.records_retired as u64,
            records_retired_by_id: retire_receipt_counts
                .iter()
                .map(|(id, count)| (id.clone(), *count as u64))
                .collect(),
            records_transformed: staged.records_transformed as u64,
        }),
    }));
    commit_apply_plan(witness, store, steps)?;

    Ok(ApplyOutcome {
        committed_commit_id: commit_id,
        catalog_epoch: target_epoch,
        records_backfilled: staged.records_backfilled,
        indexes_rebuilt: staged.indexes_rebuilt,
        records_retired: staged.records_retired,
        records_transformed: staged.records_transformed,
        receipt: activation_receipt(
            witness,
            commit_id,
            target_epoch,
            &staged,
            &retire_receipt_counts,
        ),
    })
}

fn retire_receipt_counts(
    program: &CheckedProgram,
    destructive_counts: &[(CatalogId, usize)],
) -> Result<Vec<(CatalogId, usize)>, ApplyError> {
    let mut counts = Vec::new();
    for id in retired_resource_member_ids(program)? {
        let count = destructive_counts
            .iter()
            .find_map(|(destructive_id, count)| (destructive_id == &id).then_some(*count))
            .unwrap_or(0);
        counts.push((id, count));
    }
    counts.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    counts.dedup();
    Ok(counts)
}

fn retired_resource_member_ids(program: &CheckedProgram) -> Result<Vec<CatalogId>, ApplyError> {
    let mut ids = Vec::new();
    let Some(proposal) = &program.catalog.proposal else {
        return Ok(ids);
    };
    for entry in &proposal.entries {
        if entry.kind == CatalogEntryKind::ResourceMember
            && retired_this_proposal(
                program,
                entry.stable_id.as_str(),
                entry.lifecycle,
                CatalogEntryKind::ResourceMember,
            )
        {
            ids.push(CatalogId::new(entry.stable_id.clone()).map_err(|_| {
                ApplyError::Store(StoreError::Corruption {
                    message: "evolution apply saw an invalid retired member catalog id".to_string(),
                })
            })?);
        }
    }
    Ok(ids)
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

fn stage_redundant_default_receipts(
    program: &CheckedProgram,
    places: &[CheckedSavedPlace],
    store: &TreeStore,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let mut recorded: std::collections::BTreeSet<_> = staged
        .default_records_by_id
        .iter()
        .map(|count| count.catalog_id.clone())
        .collect();
    for default in &program.catalog.evolve_defaults {
        let catalog_id =
            CatalogId::new(default.catalog_id.clone()).map_err(|_| ApplyError::Drift)?;
        if recorded.insert(catalog_id.clone()) {
            stage_default_presence_receipt(&catalog_id, places, store, staged)?;
        }
    }
    Ok(())
}

fn activation_receipt(
    witness: &EvolutionWitness,
    commit_id: u64,
    catalog_epoch: u64,
    staged: &StagedWork,
    retire_counts: &[(CatalogId, usize)],
) -> ActivationReceipt {
    ActivationReceipt {
        commit_id,
        catalog_epoch,
        source_digest: witness.source_digest.clone(),
        evolution_digest: witness.evolution_digest.clone(),
        accepted_catalog_digest: witness.accepted_catalog.digest.clone(),
        proposal_catalog_digest: witness
            .proposal_catalog
            .as_ref()
            .map(|catalog| catalog.digest.clone()),
        store_commit_id_before: witness.store_commit_id,
        changed_root_catalog_ids: witness.changed_root_catalog_ids.clone(),
        changed_index_catalog_ids: witness.changed_index_catalog_ids.clone(),
        records_backfilled: staged.records_backfilled,
        default_records_by_id: staged.default_records_by_id.clone(),
        indexes_rebuilt: staged.indexes_rebuilt,
        records_retired: staged.records_retired,
        records_retired_by_id: retire_counts.to_vec(),
        records_transformed: staged.records_transformed,
    }
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
    pub(super) default_records_by_id: Vec<ActivationDefaultRecordCount>,
    pub(super) indexes_rebuilt: usize,
    pub(super) records_retired: usize,
    pub(super) records_transformed: usize,
}

/// The read-only context every staging helper consults: the source places to scan, the
/// store snapshot, and the checked program and runtime a transform evaluates against.
/// The mutable accumulators (`steps`, `staged`) stay separate so the staging walk owns
/// them across obligations.
struct StageCtx<'a> {
    places: &'a [CheckedSavedPlace],
    store: &'a TreeStore,
    program: &'a CheckedProgram,
    runtime: &'a CheckedRuntimeProgram,
    reject_existing_proposal_defaults: bool,
}

fn stage_obligation(
    ctx: &StageCtx,
    catalog_id: &CatalogId,
    verdict: &Verdict,
    steps: &mut Vec<PlanStep>,
    staged: &mut StagedWork,
) -> Result<(), ApplyError> {
    let StageCtx {
        places,
        store,
        program,
        runtime,
        reject_existing_proposal_defaults,
    } = ctx;
    match verdict {
        Verdict::Default { value } => stage_default_backfill(
            catalog_id,
            value,
            *reject_existing_proposal_defaults && !accepted_resource_member(program, catalog_id),
            places,
            store,
            steps,
            staged,
        ),
        Verdict::DerivedRebuild => stage_index_rebuild(catalog_id, places, store, steps, staged),
        Verdict::IndexDropped => stage_index_drop(catalog_id, steps),
        Verdict::DestructiveDecisionRequired { .. } => {
            stage_retire_deletes(catalog_id, places, store, steps, staged)
        }
        Verdict::Transform { reads } => stage_transform(TransformStage {
            target_id: catalog_id,
            witness_reads: reads,
            program,
            runtime,
            places,
            store,
            steps,
            staged,
        }),
        Verdict::NoOp
        | Verdict::CatalogOnly
        | Verdict::DataProof
        | Verdict::RepairRequired { .. } => Ok(()),
    }
}

fn accepted_resource_member(program: &CheckedProgram, catalog_id: &CatalogId) -> bool {
    program.catalog.accepted_entries.iter().any(|entry| {
        entry.kind == CatalogEntryKind::ResourceMember && entry.stable_id == catalog_id.as_str()
    })
}

/// The store catalog id of a place, validated once.
pub(super) fn store_id(place: &CheckedSavedPlace) -> Result<CatalogId, ApplyError> {
    let Some(raw) = &place.store_catalog_id else {
        return Err(ApplyError::Store(StoreError::Corruption {
            message: "evolution apply saw a missing store catalog id".to_string(),
        }));
    };
    CatalogId::new(raw.clone()).map_err(|_| {
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
    let leaf = locate_in(&place.root_members, &mut steps, catalog_id)?;
    Some(MemberLocation { steps, leaf })
}

pub(super) struct MemberLocation {
    pub(super) steps: Vec<PathStep>,
    pub(super) leaf: Option<StoreLeafKind>,
}

pub(super) enum PathStep {
    Member(CatalogId),
    Layer(CatalogId),
}

fn locate_in(
    members: &[CheckedSavedMember],
    steps: &mut Vec<PathStep>,
    target: &CatalogId,
) -> Option<Option<StoreLeafKind>> {
    for member in members {
        let Some(raw_id) = &member.catalog_id else {
            continue;
        };
        let Ok(member_id) = CatalogId::new(raw_id.clone()) else {
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
            return Some(member.leaf.clone());
        }
        if matches!(member.kind, CheckedSavedMemberKind::Group)
            && let Some(leaf) = locate_in(&member.group_members, steps, target)
        {
            return Some(leaf);
        }
        steps.pop();
    }
    None
}

pub(super) fn for_each_place_record(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
) -> Result<(), ApplyError> {
    let store_id = store_id(place)?;
    store.for_each_record(&store_id, place.identity_keys.len(), visit)?;
    Ok(())
}
