//! Witness-validated evolution apply.

use marrow_check::evolution::{EvolutionWitness, Verdict};
use marrow_check::{
    CatalogEntryKind, CheckedProgram, CheckedRuntimeProgram, CheckedSavedPlace,
    checked_activation_root_places,
};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::tree::{ActivationDefaultRecordCount, TreeStore};

use crate::write_plan::{PlanStep, WritePlan};

use super::admission::{catalog_id_order, expected_retire_counts, gate_obligations};
use super::backfill::{
    stage_default_backfill, stage_default_presence_receipt, stage_index_drop, stage_index_rebuild,
    stage_retire_deletes,
};
use super::evidence::retire_evidence_digest;
use super::lifecycle::retired_proposal_ids;
use super::transform::{TransformVisit, visit_transform_writes};
use super::validate::{assert_commit_pin, validate_witness};
use super::window::{
    AppliedActivationEvidence, FenceError, StampFacts, current_engine_profile, fence,
    metadata_stamp,
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
    pub proposal_new_catalog_ids: Vec<CatalogId>,
    pub store_commit_id_before: Option<u64>,
    pub changed_root_catalog_ids: Vec<CatalogId>,
    pub changed_index_catalog_ids: Vec<CatalogId>,
    pub records_backfilled: usize,
    pub default_records_by_id: Vec<ActivationDefaultRecordCount>,
    pub indexes_rebuilt: usize,
    pub records_retired: usize,
    pub retire_evidence_digest: String,
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
    /// The store's published accepted-catalog snapshot is not the one the witness was
    /// built against. The witness fingerprints the accepted catalog it discharged the
    /// obligations over; if the store's published rows drifted from it (a concurrent
    /// activation, or a tampered catalog table), staging against the witness would write
    /// the wrong shape. Apply fails closed before staging.
    CatalogDrift {
        pinned: String,
        found: Option<String>,
    },
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
    // stamp: a shape-changing apply must confirm the store still sits at its pre-apply
    // shape before advancing it, or it would fence itself as drift.
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
    if store_stamped_at_target(witness, target_epoch, current_epoch, store)? {
        let commit_id = witness.store_commit_id.unwrap_or(0);
        let staged = StagedWork::default();
        let parts = receipt_parts(program, commit_id, &staged, &[])?;
        return Ok(build_outcome(
            witness,
            commit_id,
            target_epoch,
            &staged,
            parts,
        ));
    }

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

    let destructive_retire_counts = expected_retire_counts(witness);
    reconcile_counts(&staged, witness, &destructive_retire_counts)?;

    let commit_id = witness.store_commit_id.unwrap_or(0) + 1;
    let parts = receipt_parts(program, commit_id, &staged, &destructive_retire_counts)?;
    steps.push(activation_stamp(
        witness,
        program,
        commit_id,
        target_epoch,
        &staged,
        &parts,
    ));
    commit_apply_plan(witness, store, steps)?;
    Ok(build_outcome(
        witness,
        commit_id,
        target_epoch,
        &staged,
        parts,
    ))
}

/// Whether the store already carries the exact target activation stamp this apply would
/// publish. A matching historical applied-step stamp means any remaining source `evolve`
/// block is stale transition text; staging it again would rewrite current data. This is
/// only a replay-suppression check. Current completion verification recomputes data and
/// index effects from the live store. A same-epoch store with an older source digest is
/// not a target match, so apply still writes a metadata-only restamp for
/// identity-preserving source reorders.
fn store_stamped_at_target(
    witness: &EvolutionWitness,
    target_epoch: u64,
    current_epoch: Option<u64>,
    store: &TreeStore,
) -> Result<bool, ApplyError> {
    if current_epoch != Some(target_epoch) {
        return Ok(false);
    }
    let Some(commit) = store.read_commit_metadata()? else {
        return Ok(false);
    };
    if commit.source_digest != witness.source_digest {
        return Ok(false);
    }
    if commit.activation_evolution_digest != witness.evolution_digest {
        return Ok(false);
    }
    match store.catalog_snapshot_digest()? {
        Some(found) => {
            let target_catalog_digest = witness
                .proposal_catalog
                .as_ref()
                .map(|catalog| catalog.digest.as_str())
                .unwrap_or(witness.accepted_catalog.digest.as_str());
            Ok(found == target_catalog_digest)
        }
        None => Ok(witness.proposal_catalog.is_none()),
    }
}

/// Confirm the staged work matches the counts the witness proved, failing closed before any
/// commit. The backfill and transform totals come straight from the witness; the retire
/// total is the sum the approved destructive set authorizes, so a divergence on any of the
/// three means staging derived different work than the witness was discharged against.
fn reconcile_counts(
    staged: &StagedWork,
    witness: &EvolutionWitness,
    destructive_retire_counts: &[(CatalogId, usize)],
) -> Result<(), ApplyError> {
    let approved_retire = destructive_retire_counts
        .iter()
        .map(|(_id, count)| *count)
        .sum::<usize>();
    let checks = [
        (
            staged.records_backfilled,
            witness.counts.records_to_backfill,
        ),
        (staged.records_retired, approved_retire),
        (
            staged.records_transformed,
            witness.counts.records_to_transform,
        ),
    ];
    for (staged_count, expected) in checks {
        if staged_count != expected {
            return Err(ApplyError::PlanMismatch {
                expected,
                staged: staged_count,
            });
        }
    }
    Ok(())
}

/// The receipt fields a committed (or already-committed no-op) activation shares between its
/// stamp and its outcome: the per-id retire counts, their digest at this commit, and the
/// proposal-new catalog ids. Computed once so the stamp and the returned receipt agree by
/// construction on the same commit id.
struct ReceiptParts {
    retire_counts: Vec<(CatalogId, usize)>,
    retire_counts_u64: Vec<(CatalogId, u64)>,
    retire_digest: String,
    proposal_new_catalog_ids: Vec<CatalogId>,
}

fn receipt_parts(
    program: &CheckedProgram,
    commit_id: u64,
    staged: &StagedWork,
    destructive_retire_counts: &[(CatalogId, usize)],
) -> Result<ReceiptParts, ApplyError> {
    let retire_counts = retire_receipt_counts(program, destructive_retire_counts)?;
    let retire_counts_u64 = retire_counts_u64(&retire_counts)?;
    let retire_digest =
        retire_evidence_digest(commit_id, staged.records_retired as u64, &retire_counts_u64);
    Ok(ReceiptParts {
        retire_counts,
        retire_counts_u64,
        retire_digest,
        proposal_new_catalog_ids: proposal_new_catalog_ids(program),
    })
}

/// The metadata stamp a committing activation appends to its plan, carrying the activation
/// facts the commit records at `commit_id` and the activated catalog snapshot it publishes.
///
/// The snapshot is the proposal catalog the witness activates: present exactly when the
/// witness carries a `proposal_catalog`, so it publishes atomically with the epoch it
/// belongs to. An apply that does not advance the accepted catalog (a pure backfill at the
/// same epoch) carries no proposal, so it publishes nothing and the accepted catalog the
/// store holds stays. The full snapshot rows live on the checked program; the witness
/// fingerprint, re-verified before staging, pins which catalog those rows must be.
fn activation_stamp(
    witness: &EvolutionWitness,
    program: &CheckedProgram,
    commit_id: u64,
    target_epoch: u64,
    staged: &StagedWork,
    parts: &ReceiptParts,
) -> PlanStep {
    let catalog_snapshot = witness
        .proposal_catalog
        .as_ref()
        .and(program.catalog.proposal.clone())
        .map(Box::new);
    metadata_stamp(StampFacts {
        catalog_epoch: target_epoch,
        catalog_snapshot,
        commit_id,
        source_digest: witness.source_digest.clone(),
        changed_root_catalog_ids: witness.changed_root_catalog_ids.clone(),
        changed_index_catalog_ids: witness.changed_index_catalog_ids.clone(),
        applied_activation_evidence: Some(AppliedActivationEvidence {
            evolution_digest: witness.evolution_digest.clone(),
            proposal_catalog_digest: witness
                .proposal_catalog
                .as_ref()
                .map(|catalog| catalog.digest.clone()),
            proposal_new_catalog_ids: parts.proposal_new_catalog_ids.clone(),
            records_backfilled: staged.records_backfilled as u64,
            default_records_by_id: staged.default_records_by_id.clone(),
            indexes_rebuilt: staged.indexes_rebuilt as u64,
            records_retired: staged.records_retired as u64,
            retire_evidence_digest: parts.retire_digest.clone(),
            records_retired_by_id: parts.retire_counts_u64.clone(),
            records_transformed: staged.records_transformed as u64,
        }),
    })
}

fn build_outcome(
    witness: &EvolutionWitness,
    commit_id: u64,
    target_epoch: u64,
    staged: &StagedWork,
    parts: ReceiptParts,
) -> ApplyOutcome {
    ApplyOutcome {
        receipt: activation_receipt(
            witness,
            commit_id,
            target_epoch,
            staged,
            &parts.retire_counts,
            parts.proposal_new_catalog_ids,
            parts.retire_digest,
        ),
    }
}

fn proposal_new_catalog_ids(program: &CheckedProgram) -> Vec<CatalogId> {
    let accepted: std::collections::HashSet<_> = program
        .catalog
        .accepted_entries
        .iter()
        .map(|entry| entry.stable_id.as_str())
        .collect();
    program
        .catalog
        .proposal
        .as_ref()
        .into_iter()
        .flat_map(|catalog| catalog.entries.iter())
        .filter(|entry| !accepted.contains(entry.stable_id.as_str()))
        .filter_map(|entry| CatalogId::new(entry.stable_id.clone()).ok())
        .collect()
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
    counts.sort_by(catalog_id_order);
    counts.dedup();
    Ok(counts)
}

fn retire_counts_u64(counts: &[(CatalogId, usize)]) -> Result<Vec<(CatalogId, u64)>, ApplyError> {
    counts
        .iter()
        .map(|(id, count)| {
            Ok((
                id.clone(),
                u64::try_from(*count).map_err(|_| ApplyError::Drift)?,
            ))
        })
        .collect()
}

fn retired_resource_member_ids(program: &CheckedProgram) -> Result<Vec<CatalogId>, ApplyError> {
    retired_proposal_ids(program, marrow_check::CatalogEntryKind::ResourceMember)
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
    proposal_new_catalog_ids: Vec<CatalogId>,
    retire_evidence_digest: String,
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
        proposal_new_catalog_ids,
        store_commit_id_before: witness.store_commit_id,
        changed_root_catalog_ids: witness.changed_root_catalog_ids.clone(),
        changed_index_catalog_ids: witness.changed_index_catalog_ids.clone(),
        records_backfilled: staged.records_backfilled,
        default_records_by_id: staged.default_records_by_id.clone(),
        indexes_rebuilt: staged.indexes_rebuilt,
        records_retired: staged.records_retired,
        retire_evidence_digest,
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
        // Derived rebuilds are staged in a second pass after every data obligation, so a
        // rebuilt index sees the defaults and transforms this apply also writes. The apply
        // loop diverts them before they reach here; one arriving means the loop's deferral
        // broke, which is a fail-closed internal divergence rather than a silent skip.
        Verdict::DerivedRebuild => Err(ApplyError::Store(StoreError::Corruption {
            message: "evolution apply staged a derived rebuild outside its deferred pass"
                .to_string(),
        })),
        Verdict::IndexDropped => stage_index_drop(catalog_id, steps),
        Verdict::DestructiveDecisionRequired { .. } => {
            stage_retire_deletes(catalog_id, places, store, steps, staged)
        }
        Verdict::Transform { reads } => {
            let mut count = 0usize;
            let mut stage = |address, value| {
                steps.push(PlanStep::WriteData { address, value });
                count += 1;
                Ok(())
            };
            visit_transform_writes(TransformVisit {
                target_id: catalog_id,
                witness_reads: reads,
                program,
                runtime,
                places,
                store,
                visit: &mut stage,
            })?;
            staged.records_transformed += count;
            Ok(())
        }
        Verdict::NoOp
        | Verdict::CatalogOnly
        | Verdict::DataProof
        | Verdict::RepairRequired { .. } => Ok(()),
    }
}

/// Whether `catalog_id` names a resource member the accepted catalog already owns. A
/// default whose target is not yet accepted is proposal-new: apply must fail closed on an
/// existing target cell, and completion verification holds it to the exact constant. The
/// single owner of that classification so staging and completion cannot disagree.
pub(super) fn accepted_resource_member(program: &CheckedProgram, catalog_id: &CatalogId) -> bool {
    program.catalog.accepted_entries.iter().any(|entry| {
        entry.kind == CatalogEntryKind::ResourceMember && entry.stable_id == catalog_id.as_str()
    })
}
