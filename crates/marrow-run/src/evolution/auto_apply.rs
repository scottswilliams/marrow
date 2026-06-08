//! Run-time auto-apply of a zero-record-mutation evolution.
//!
//! When the activation fence reports schema drift at the current epoch, a `run` may
//! discharge the evolution itself instead of fencing — but only when discharging it
//! mutates no stored record. Adding a sparse field, a new resource/store/enum-member,
//! or any change against an empty affected store stages no data write, so the apply is
//! a metadata-only stamp the run can perform unattended. A backfill, a record-rewriting
//! transform, or a destructive drop over populated data still requires explicit
//! `evolve apply` (and, for a drop, confirmation), so it fences with an actionable
//! diagnostic.
//!
//! The decision is computed from the same evolution witness `evolve preview`/`evolve
//! apply` own, against committed data, and the apply it performs is the production apply
//! path. The witness pins the store commit id, and apply re-checks that pin inside the
//! write transaction, so a write that commits between the probe and the stamp moves the
//! commit id and fails the apply closed: the auto-apply decision can only become more
//! conservative under a race, never migrate a store that is no longer empty.

use marrow_check::CheckedProgram;
use marrow_check::evolution::{EvolutionWitness, Verdict};
use marrow_store::cell::CatalogId;
use marrow_store::tree::TreeStore;

use super::apply::{ApplyError, Approval, apply};

/// The mutation an evolution would inflict on stored records, classified from its
/// witness. Only `ZeroMutation` is safe to apply automatically on `run`; every other
/// outcome carries record work or a data-loss decision a developer must drive through
/// explicit `evolve apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunObligation {
    /// No stored record changes: an intrinsically additive change, or any change against
    /// an empty affected store. The retire set, if any, names only empty targets, so
    /// discharging it deletes nothing. Safe to auto-apply on `run`.
    ZeroMutation { empty_retires: Vec<CatalogId> },
    /// A newly-required member must be backfilled into existing records.
    Backfill { records: usize },
    /// A checked transform rewrites stored records.
    Transform { records: usize },
    /// A drop over populated data: discharging it loses `populated` stored cells. Stays
    /// explicit and confirmed; never auto-applies.
    DestructiveDrop { populated: usize },
    /// The snapshot cannot satisfy the obligation; activation fails closed regardless of
    /// how it is driven.
    Repair,
}

impl RunObligation {
    /// Classify the witness by the heaviest record obligation it carries, against
    /// committed data. The order is by escalating severity: a repair blocks everything,
    /// a populated drop is a data-loss decision, then record-rewriting work, and only an
    /// evolution with none of these mutates zero records. A drop whose target is empty
    /// carries a zero populated count and folds into the zero-mutation bucket — its
    /// retire id is recorded so the auto-apply can authorize the (empty) delete.
    pub fn classify(witness: &EvolutionWitness) -> Self {
        if witness
            .verdicts
            .iter()
            .any(|obligation| matches!(obligation.verdict, Verdict::RepairRequired { .. }))
        {
            return Self::Repair;
        }
        let populated_drop =
            witness
                .verdicts
                .iter()
                .find_map(|obligation| match obligation.verdict {
                    Verdict::DestructiveDecisionRequired { populated } if populated > 0 => {
                        Some(populated)
                    }
                    _ => None,
                });
        if let Some(populated) = populated_drop {
            return Self::DestructiveDrop { populated };
        }
        if witness.counts.records_to_backfill > 0 {
            return Self::Backfill {
                records: witness.counts.records_to_backfill,
            };
        }
        if witness.counts.records_to_transform > 0 {
            return Self::Transform {
                records: witness.counts.records_to_transform,
            };
        }
        Self::ZeroMutation {
            empty_retires: empty_retire_ids(witness),
        }
    }
}

/// The catalog ids of every empty-target retire in the witness — a drop whose target holds
/// no records. Such a retire mutates nothing, but the apply path still gates every retire
/// behind maintenance and a scoped approval, so the auto-apply must name these ids at count
/// zero to discharge them.
fn empty_retire_ids(witness: &EvolutionWitness) -> Vec<CatalogId> {
    witness
        .verdicts
        .iter()
        .filter_map(|obligation| match obligation.verdict {
            Verdict::DestructiveDecisionRequired { populated: 0 } => {
                Some(obligation.catalog_id.clone())
            }
            _ => None,
        })
        .collect()
}

/// What a `run` does when the fence reports schema drift: either the evolution was
/// auto-applied and the run proceeds, or it carries record work / a data-loss decision
/// and the run must fence with the obligation as the actionable cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoApplyOutcome {
    Applied,
    MustFence(RunObligation),
}

/// Attempt to auto-apply `witness` against `store` when it mutates zero stored records.
///
/// A zero-mutation evolution is discharged through the production apply path, so it
/// stamps the new digest and advances the epoch exactly as `evolve apply` does for a
/// no-data-work change. An empty-target drop is authorized with a zero-count approval
/// under the maintenance capability, because the apply path gates every retire behind a
/// scoped approval even when it deletes nothing. Any other obligation returns
/// `MustFence` without touching the store.
pub fn try_auto_apply(
    witness: &EvolutionWitness,
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<AutoApplyOutcome, ApplyError> {
    let obligation = RunObligation::classify(witness);
    let RunObligation::ZeroMutation { empty_retires } = &obligation else {
        return Ok(AutoApplyOutcome::MustFence(obligation));
    };
    // Every empty-target drop is named at count zero so the approval matches the witness
    // destructive set exactly; the maintenance capability is granted only to authorize
    // these zero-count deletes, and gates nothing else here.
    let approval = (!empty_retires.is_empty()).then(|| Approval {
        retires: empty_retires.iter().map(|id| (id.clone(), 0)).collect(),
    });
    apply(witness, program, store, true, approval.as_ref())?;
    Ok(AutoApplyOutcome::Applied)
}

#[cfg(test)]
mod tests {
    use super::{RunObligation, empty_retire_ids};
    use marrow_check::evolution::{
        CatalogFingerprint, DischargeCounts, EvolutionWitness, ObligationVerdict, RepairReason,
        Verdict,
    };
    use marrow_store::cell::CatalogId;

    fn id(hex: &str) -> CatalogId {
        CatalogId::new(format!("cat_{hex:0>32}")).expect("valid catalog id")
    }

    fn witness(verdicts: Vec<ObligationVerdict>, counts: DischargeCounts) -> EvolutionWitness {
        EvolutionWitness {
            source_digest: "sha256:0".to_string(),
            evolution_digest: "sha256:0".to_string(),
            accepted_catalog: CatalogFingerprint {
                epoch: 1,
                digest: "sha256:0".to_string(),
            },
            proposal_catalog: None,
            store_source_digest: None,
            engine_profile_digest: None,
            layout_epoch: None,
            store_commit_id: None,
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
            verdicts,
            counts,
        }
    }

    fn verdict(catalog_id: CatalogId, verdict: Verdict) -> ObligationVerdict {
        ObligationVerdict {
            catalog_id,
            verdict,
        }
    }

    #[test]
    fn an_additive_change_is_zero_mutation() {
        let classified = RunObligation::classify(&witness(
            vec![verdict(id("1"), Verdict::NoOp)],
            DischargeCounts::default(),
        ));
        assert_eq!(
            classified,
            RunObligation::ZeroMutation {
                empty_retires: Vec::new()
            }
        );
    }

    #[test]
    fn a_drop_against_an_empty_target_is_zero_mutation_naming_the_retire() {
        let classified = RunObligation::classify(&witness(
            vec![verdict(
                id("7"),
                Verdict::DestructiveDecisionRequired { populated: 0 },
            )],
            DischargeCounts::default(),
        ));
        assert_eq!(
            classified,
            RunObligation::ZeroMutation {
                empty_retires: vec![id("7")]
            }
        );
    }

    #[test]
    fn a_drop_against_populated_data_is_destructive_and_never_zero_mutation() {
        let classified = RunObligation::classify(&witness(
            vec![verdict(
                id("7"),
                Verdict::DestructiveDecisionRequired { populated: 3 },
            )],
            DischargeCounts::default(),
        ));
        assert_eq!(classified, RunObligation::DestructiveDrop { populated: 3 });
    }

    #[test]
    fn a_required_add_over_populated_data_is_a_backfill() {
        let classified = RunObligation::classify(&witness(
            Vec::new(),
            DischargeCounts {
                records_to_backfill: 4,
                ..DischargeCounts::default()
            },
        ));
        assert_eq!(classified, RunObligation::Backfill { records: 4 });
    }

    #[test]
    fn a_record_rewriting_transform_is_a_transform() {
        let classified = RunObligation::classify(&witness(
            Vec::new(),
            DischargeCounts {
                records_to_transform: 2,
                ..DischargeCounts::default()
            },
        ));
        assert_eq!(classified, RunObligation::Transform { records: 2 });
    }

    #[test]
    fn a_repair_obligation_dominates_every_other_outcome() {
        // A repair blocks activation regardless of any record counts that ride alongside
        // it, so it is classified ahead of backfill or transform.
        let classified = RunObligation::classify(&witness(
            vec![verdict(
                id("9"),
                Verdict::RepairRequired {
                    reason: RepairReason::MissingRequiredMember,
                },
            )],
            DischargeCounts {
                records_to_backfill: 5,
                ..DischargeCounts::default()
            },
        ));
        assert_eq!(classified, RunObligation::Repair);
    }

    #[test]
    fn a_populated_drop_dominates_an_alongside_backfill() {
        // A multi-store evolution where one store needs a backfill and another drops
        // populated data is a data-loss decision as a whole: it must not auto-apply, so
        // the destructive drop is classified ahead of the backfill.
        let classified = RunObligation::classify(&witness(
            vec![verdict(
                id("7"),
                Verdict::DestructiveDecisionRequired { populated: 1 },
            )],
            DischargeCounts {
                records_to_backfill: 5,
                ..DischargeCounts::default()
            },
        ));
        assert_eq!(classified, RunObligation::DestructiveDrop { populated: 1 });
    }

    #[test]
    fn empty_retire_ids_lists_only_empty_target_retires() {
        let ids = empty_retire_ids(&witness(
            vec![
                verdict(id("1"), Verdict::NoOp),
                verdict(
                    id("7"),
                    Verdict::DestructiveDecisionRequired { populated: 0 },
                ),
                verdict(
                    id("9"),
                    Verdict::DestructiveDecisionRequired { populated: 3 },
                ),
            ],
            DischargeCounts::default(),
        ));
        assert_eq!(ids, vec![id("7")]);
    }
}
