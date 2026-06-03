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
mod preview;
mod witness;

pub(crate) use intents::{
    DefaultIntent, EvolveIntents, RenameIntent, RetireIntent, TransformIntent, check_evolve_types,
    collect_evolve_intents,
};

pub use preview::preview;
pub use witness::{
    CatalogFingerprint, DefaultValue, DischargeCounts, EvolutionWitness, ObligationVerdict,
    RepairReason, Verdict,
};
