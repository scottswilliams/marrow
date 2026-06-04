use marrow_store::tree::{ActivationDefaultRecordCount, CommitMetadata};

use super::super::apply::ApplyError;
use super::default::DefaultCompletion;

pub(super) fn verify_default_receipt(
    defaults: &[DefaultCompletion],
    commit: &CommitMetadata,
) -> Result<(), ApplyError> {
    let expected = sorted_default_targets(defaults);
    let recorded = sorted_default_counts(&commit.activation_default_records_by_id);
    if expected.len() != recorded.len() {
        return Err(ApplyError::Drift);
    }
    let mut total_backfilled = 0u64;
    for (expected, recorded) in expected.iter().zip(recorded.iter()) {
        let mut digest = expected.cell_digest.clone();
        digest.u64(recorded.records_backfilled);
        digest.u64(expected.target_records);
        if expected.catalog_id != recorded.catalog_id
            || expected.target_records != recorded.target_records
            || recorded.records_backfilled > recorded.target_records
            || (expected.proposal_new && recorded.records_backfilled != expected.target_records)
            || digest.finish() != recorded.evidence_digest
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

fn sorted_default_targets(defaults: &[DefaultCompletion]) -> Vec<&DefaultCompletion> {
    let mut defaults: Vec<_> = defaults.iter().collect();
    defaults.sort_by(|a, b| a.catalog_id.as_str().cmp(b.catalog_id.as_str()));
    defaults
}

fn sorted_default_counts(
    counts: &[ActivationDefaultRecordCount],
) -> Vec<&ActivationDefaultRecordCount> {
    let mut counts: Vec<_> = counts.iter().collect();
    counts.sort_by(|a, b| a.catalog_id.as_str().cmp(b.catalog_id.as_str()));
    counts
}
