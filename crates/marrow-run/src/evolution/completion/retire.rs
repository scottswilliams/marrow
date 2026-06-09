use marrow_check::{CatalogEntryKind, CheckedProgram, CheckedSavedPlace};
use marrow_store::cell::CatalogId;
use marrow_store::tree::{CommitMetadata, TreeStore};

use super::super::apply::{ApplyError, StagedWork};
use super::super::backfill::stage_retire_deletes;
use super::super::evidence::retire_evidence_digest;
use super::super::lifecycle::retired_proposal_ids;

pub(super) fn verify_retire_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    commit: &CommitMetadata,
    places: &[CheckedSavedPlace],
) -> Result<(), ApplyError> {
    let mut retired = retired_proposal_ids(program, CatalogEntryKind::ResourceMember)?;
    retired.sort();
    retired.dedup();
    let expected = exact_retire_counts_u64(commit.activation_records_retired_by_id.clone())?;
    let recorded_ids: Vec<_> = expected.iter().map(|(id, _count)| id.clone()).collect();
    if recorded_ids != retired {
        return Err(ApplyError::Drift);
    }
    let recorded_digest = retire_evidence_digest(
        commit.commit_id,
        commit.activation_records_retired,
        &expected,
    );
    if commit.activation_retire_evidence_digest.is_empty()
        || commit.activation_retire_evidence_digest != recorded_digest
    {
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

fn exact_retire_counts_u64(
    mut counts: Vec<(CatalogId, u64)>,
) -> Result<Vec<(CatalogId, u64)>, ApplyError> {
    counts.sort_by(|a, b| a.0.cmp(&b.0));
    if counts
        .windows(2)
        .any(|pair| pair[0].0.as_str() == pair[1].0.as_str())
    {
        return Err(ApplyError::Drift);
    }
    Ok(counts)
}
