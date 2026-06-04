//! Source-native evolution: the durable intents an `evolve` block declares and
//! the data-attached discharge that decides what a future apply must do.
//!
//! Intents (`intents`) turn an `evolve` block into the rename/retire/default/
//! transform declarations the catalog binding and the discharge consult. Discharge
//! (`discharge`) compares the accepted snapshot in the store against what current
//! source and the catalog proposal now require, classifying each obligation as one
//! [`Verdict`] role. The witness (`witness`) is the read-only artifact a future
//! apply consumes; preview (`preview`) assembles it and reports whether the program
//! is activatable. Discharge stays crate-internal: the witness and `preview` are the
//! only surface that crosses into apply. Nothing here mutates the store.

mod const_default;
mod discharge;
mod intents;
pub(crate) mod leaf_type;
mod preview;
mod transform_reads;
mod witness;

use marrow_syntax::Expression;

pub(crate) use intents::{
    DefaultIntent, EvolveIntents, RenameIntent, RetireIntent, TransformIntent, check_evolve_types,
    check_transform_effects, collect_evolve_intents, transform_body_in_source,
};

pub use discharge::RepairDiagnostic;
pub use preview::preview;
pub use transform_reads::{TransformReadMember, transform_read_members};
pub use witness::{
    CatalogFingerprint, DefaultValue, DischargeCounts, EvolutionWitness, ObligationVerdict,
    RepairReason, Verdict,
};

use crate::executable::{CheckedSavedMember, checked_activation_root_places};
use crate::{CheckedProgram, StoreLeafKind};

/// Evaluate the encoded default value for a bound member id using the same const-default
/// owner as discharge. Resume verification uses this to prove the committed default
/// bytes are still present before publishing a stored proposal.
pub fn default_value_for_bound_member(
    program: &CheckedProgram,
    catalog_id: &str,
    value: &Expression,
) -> Option<Result<DefaultValue, String>> {
    let leaf = checked_activation_root_places(program)
        .iter()
        .find_map(|place| member_leaf(&place.root_members, catalog_id))?;
    let StoreLeafKind::Scalar(scalar) = leaf else {
        return Some(Err(
            "evolve default targets a non-scalar member; use a transform for computed values"
                .to_string(),
        ));
    };
    Some(const_default::eval_const_default(value, scalar).map_err(|error| error.message()))
}

fn member_leaf(members: &[CheckedSavedMember], catalog_id: &str) -> Option<StoreLeafKind> {
    for member in members {
        if member.catalog_id.as_deref() == Some(catalog_id) {
            return member.leaf.clone();
        }
        if let Some(leaf) = member_leaf(&member.group_members, catalog_id) {
            return Some(leaf);
        }
    }
    None
}
