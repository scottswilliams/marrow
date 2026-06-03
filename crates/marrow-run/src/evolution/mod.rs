//! Witness-validated evolution apply.
//!
//! Apply is the runtime side of source-native evolution: it consumes the read-only
//! [`marrow_check::evolution::EvolutionWitness`] a preview produced and commits the
//! durable work it describes. It re-runs the production discharge to confirm the store
//! still matches the witness, gates blocking and destructive obligations, stages the
//! backfills, index rebuilds, and approved retires into one write plan, and commits
//! that plan atomically with the catalog-epoch and engine-profile stamp. Drift, a
//! blocking obligation, or a store error leaves the store unchanged.

mod apply;
mod backfill;
mod scan;

pub use apply::{ApplyError, ApplyOutcome, Approval, apply};
