//! Witness-validated evolution apply.
//!
//! Apply is the runtime side of source-native evolution: it consumes the read-only
//! [`marrow_check::evolution::EvolutionWitness`] a preview produced and commits the
//! durable work it describes. It re-runs the production discharge to confirm the store
//! still matches the witness, gates blocking and destructive obligations, opens one
//! write transaction, and writes backfills, transforms, index rebuilds, index drops,
//! and approved retires directly through transaction-visible store operations.
//! `WritePlan` is used only for the final catalog and metadata stamp. Drift, a
//! blocking obligation, or a store error leaves the store unchanged.

mod admission;
mod apply;
mod auto_apply;
mod backfill;
mod baseline;
mod lifecycle;
mod locate;
mod rebuild;
mod transform;
mod validate;
mod window;

pub use apply::{ApplyError, ApplyOutcome, Approval, apply};
pub use auto_apply::{AutoApplyOutcome, RunObligation, try_auto_apply};
pub use baseline::{BaselineError, commit_catalog_baseline};
pub use rebuild::rebuild_store_indexes;
pub use window::{FenceError, current_engine_profile, fence};
pub(crate) use window::{StampFacts, metadata_stamp};
