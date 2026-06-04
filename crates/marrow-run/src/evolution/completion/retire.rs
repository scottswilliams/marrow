use marrow_check::{CatalogEntryKind, CheckedProgram, CheckedSavedPlace};
use marrow_store::cell::CatalogId;
use marrow_store::tree::{CommitMetadata, TreeStore};

use super::super::admission::normalized_retire_approval;
use super::super::apply::{ApplyError, Approval, StagedWork};
use super::super::backfill::stage_retire_deletes;
use super::retired_ids;

pub(super) fn verify_retire_completion(
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
