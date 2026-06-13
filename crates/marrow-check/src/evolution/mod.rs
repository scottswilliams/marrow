//! Source-native evolution: the durable intents an `evolve` block declares and
//! the data-attached discharge that decides what a future apply must do.
//!
//! Intents (`intents`) turn an `evolve` block into the rename/retire/default/
//! transform declarations the catalog binding and the discharge consult. Discharge
//! (`discharge`) compares the caller-supplied accepted snapshot against what current
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

pub(crate) use intents::{
    DefaultIntent, EvolveIntents, RenameIntent, RetireIntent, TransformIntent, check_evolve_types,
    check_transform_effects, collect_evolve_intents, transform_body_in_source,
};
pub(crate) use transform_reads::transform_old_member;

pub use discharge::RepairDiagnostic;
pub use preview::{
    BackupWitnessFactSet, EvolutionPreviewError, LiveStorePreviewStatus, WitnessFactSet,
    evolution_preview, preview,
};
pub use transform_reads::{TransformReadMember, transform_read_members};
pub use witness::{
    CatalogFingerprint, DefaultValue, DischargeCounts, EvolutionWitness, ObligationVerdict,
    RejectedDefault, RepairReason, Verdict,
};
