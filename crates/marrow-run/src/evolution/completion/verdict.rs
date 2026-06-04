use marrow_check::evolution::{EvolutionWitness, Verdict};

use super::super::apply::ApplyError;

pub(super) fn verify_no_repair_verdicts(witness: &EvolutionWitness) -> Result<(), ApplyError> {
    if witness
        .verdicts
        .iter()
        .any(|outcome| matches!(outcome.verdict, Verdict::RepairRequired { .. }))
    {
        return Err(ApplyError::NotActivatable);
    }
    Ok(())
}
