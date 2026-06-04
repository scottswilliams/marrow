use marrow_check::evolution::{EvolutionWitness, Verdict};
use marrow_check::{CheckedProgram, CheckedSavedPlace};
use marrow_store::tree::TreeStore;

use super::super::apply::ApplyError;
use super::super::transform::{TransformVisit, visit_transform_writes};

pub(super) fn verify_transform_completion(
    program: &CheckedProgram,
    store: &TreeStore,
    places: &[CheckedSavedPlace],
    witness: &EvolutionWitness,
) -> Result<usize, ApplyError> {
    let runtime = program.runtime();
    let mut completed = 0usize;
    for obligation in &witness.verdicts {
        let Verdict::Transform { reads } = &obligation.verdict else {
            continue;
        };
        let target = obligation.catalog_id.clone();
        let mut count = 0usize;
        let mut verify = |address: crate::store::DataAddress, value: Vec<u8>| {
            let current =
                store.read_data_value(&address.store, &address.identity, &address.path)?;
            if current.as_deref() != Some(value.as_slice()) {
                return Err(ApplyError::Drift);
            }
            count += 1;
            Ok(())
        };
        visit_transform_writes(TransformVisit {
            target_id: &target,
            witness_reads: reads,
            program,
            runtime: &runtime,
            places,
            store,
            visit: &mut verify,
        })?;
        completed += count;
    }
    Ok(completed)
}
