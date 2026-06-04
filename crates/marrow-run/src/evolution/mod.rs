//! Witness-validated evolution apply.
//!
//! Apply is the runtime side of source-native evolution: it consumes the read-only
//! [`marrow_check::evolution::EvolutionWitness`] a preview produced and commits the
//! durable work it describes. It re-runs the production discharge to confirm the store
//! still matches the witness, gates blocking and destructive obligations, stages the
//! backfills, index rebuilds, and approved retires into one write plan, and commits
//! that plan atomically with the catalog-epoch and engine-profile stamp. Drift, a
//! blocking obligation, or a store error leaves the store unchanged.

mod admission;
mod apply;
mod backfill;
mod completion;
mod rebuild;
mod transform;
mod validate;
mod window;

pub use apply::{ApplyError, ApplyOutcome, Approval, apply};
pub use completion::verify_activation_completion;
pub use rebuild::rebuild_store_indexes;
pub use window::{FenceError, current_engine_profile, fence};
pub(crate) use window::{StampFacts, metadata_stamp};
