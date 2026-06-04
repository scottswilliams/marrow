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

/// Whether the approval names exactly the witness destructive set, each id at its exact
/// per-id populated count. Comparing the sorted `(id, count)` pairs rejects a wrong
/// per-id count even when the totals across ids happen to match, so a developer cannot
/// approve dropping two cells from one member by naming one cell each on two members.
/// The witness has one entry per destructive id, so the approval is deduped first: a
/// flag repeated verbatim collapses and still matches, while two entries for one id with
/// different counts survive the dedup and correctly mismatch the single witness entry.
fn approval_matches(
    approval: &Approval,
    destructive: &[(marrow_store::cell::CatalogId, usize)],
) -> bool {
    let mut approved = approval.retires.clone();
    approved.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    approved.dedup();
    let mut expected = destructive.to_vec();
    expected.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
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

#[cfg(test)]
mod tests {
    use super::{Approval, approval_matches};
    use marrow_store::cell::CatalogId;

    fn id(hex: &str) -> CatalogId {
        CatalogId::new(format!("cat_{hex:0>32}")).expect("valid catalog id")
    }

    #[test]
    fn approval_matches_exact_set() {
        let approval = Approval {
            retires: vec![(id("0000000000000001"), 3)],
        };
        let destructive = [(id("0000000000000001"), 3)];
        assert!(approval_matches(&approval, &destructive));
    }

    #[test]
    fn approval_with_duplicated_flag_still_matches() {
        let approval = Approval {
            retires: vec![(id("0000000000000001"), 3), (id("0000000000000001"), 3)],
        };
        let destructive = [(id("0000000000000001"), 3)];
        assert!(approval_matches(&approval, &destructive));
    }

    #[test]
    fn approval_with_conflicting_counts_for_one_id_mismatches() {
        let approval = Approval {
            retires: vec![(id("0000000000000001"), 3), (id("0000000000000001"), 4)],
        };
        let destructive = [(id("0000000000000001"), 3)];
        assert!(!approval_matches(&approval, &destructive));
    }

    #[test]
    fn approval_with_wrong_count_mismatches() {
        let approval = Approval {
            retires: vec![(id("0000000000000001"), 2)],
        };
        let destructive = [(id("0000000000000001"), 3)];
        assert!(!approval_matches(&approval, &destructive));
    }
}
