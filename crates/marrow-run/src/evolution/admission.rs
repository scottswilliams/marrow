//! Repair and destructive-decision admission for evolution apply.

use marrow_check::evolution::{EvolutionWitness, Verdict};

use super::apply::{ApplyError, Approval};

pub(super) fn gate_obligations(
    witness: &EvolutionWitness,
    maintenance: bool,
    approval: Option<&Approval>,
) -> Result<(), ApplyError> {
    let destructive: Vec<_> = witness
        .verdicts
        .iter()
        .filter_map(|obligation| match &obligation.verdict {
            Verdict::DestructiveDecisionRequired { populated } => {
                Some((obligation.catalog_id.clone(), *populated))
            }
            _ => None,
        })
        .collect();
    for obligation in &witness.verdicts {
        match &obligation.verdict {
            Verdict::RepairRequired { .. } => {
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
                if !approval_matches(approval, &destructive) {
                    return Err(ApplyError::ApprovalMismatch);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn approval_matches(
    approval: &Approval,
    destructive: &[(marrow_store::cell::CatalogId, usize)],
) -> bool {
    if approval.populated
        != destructive
            .iter()
            .map(|(_, populated)| populated)
            .sum::<usize>()
    {
        return false;
    }
    let mut approved = approval.catalog_ids.clone();
    approved.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    approved.dedup();
    let mut expected: Vec<_> = destructive.iter().map(|(id, _)| id.clone()).collect();
    expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    expected.dedup();
    approved == expected
}

/// The total populated cell count every destructive retire in the witness recorded.
/// The staged deletes must match this sum, so apply refuses a store that changed under
/// an approved decision.
pub(super) fn expected_retire_count(witness: &EvolutionWitness) -> usize {
    witness
        .verdicts
        .iter()
        .map(|obligation| match &obligation.verdict {
            Verdict::DestructiveDecisionRequired { populated } => *populated,
            _ => 0,
        })
        .sum()
}
