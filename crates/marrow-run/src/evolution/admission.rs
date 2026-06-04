//! Repair and destructive-decision admission for evolution apply.

use marrow_check::evolution::{EvolutionWitness, Verdict};
use marrow_store::cell::CatalogId;

use super::apply::{ApplyError, Approval};

pub(super) fn gate_obligations(
    witness: &EvolutionWitness,
    maintenance: bool,
    approval: Option<&Approval>,
) -> Result<(), ApplyError> {
    let destructive = expected_retire_counts(witness);
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
fn approval_matches(approval: &Approval, destructive: &[(CatalogId, usize)]) -> bool {
    normalized_retire_approval(approval) == sorted_retire_counts(destructive)
}

pub(super) fn normalized_retire_approval(approval: &Approval) -> Vec<(CatalogId, usize)> {
    sorted_retire_counts(&approval.retires)
}

pub(super) fn expected_retire_counts(witness: &EvolutionWitness) -> Vec<(CatalogId, usize)> {
    let counts: Vec<_> = witness
        .verdicts
        .iter()
        .filter_map(|obligation| match &obligation.verdict {
            Verdict::DestructiveDecisionRequired { populated } => {
                Some((obligation.catalog_id.clone(), *populated))
            }
            _ => None,
        })
        .collect();
    sorted_retire_counts(&counts)
}

fn sorted_retire_counts(counts: &[(CatalogId, usize)]) -> Vec<(CatalogId, usize)> {
    let mut counts = counts.to_vec();
    counts.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    counts.dedup();
    counts
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
