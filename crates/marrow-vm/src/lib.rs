//! The Marrow stack virtual machine.
//!
//! The VM runs a sealed [`marrow_verify::VerifiedImage`] over its typed
//! instruction tape. It is the sole production executor on the beta line: it
//! accepts only a verified image, never raw bytes or a compiler artifact, so a
//! verifier/VM disagreement about instruction shape is unrepresentable. Runtime
//! faults are typed and source-mapped ([`RuntimeFault`]); execution runs under
//! private bounds. Durable operations route through the path kernel, wired in with
//! the durable slices. A durable export runs only through the attachment the lifecycle
//! prepared and admitted ([`run_export`]), and a durable source test only through the
//! fresh test it minted ([`run_test`]); the preparation and mint types are re-exported
//! for the CLI, which reaches them through this crate alone.

#[cfg(test)]
#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
mod attach;
#[cfg(test)]
mod commit_outcome_tests;
#[cfg(test)]
mod commit_poison_tests;
mod fault;
pub mod render;
mod run;
mod value;

pub use attach::{DurableRun, run_export, run_test};
pub use fault::{DurableExecutionFault, IncompleteDisposition, InvocationIncomplete, RuntimeFault};
pub use marrow_kernel::durable::DurableCommitState;
pub use marrow_lifecycle::{
    EphemeralOutcome, FreshTest, MemoryAttachment, PreparedImage, fresh_test, mint_ephemeral,
    prepare,
};
pub use run::run;
pub use value::Value;
// The key-scalar type a `Value::Map` entry and a `Value::Id` key tuple carry. It
// belongs to the kernel codec owner; the value model surfaces it because its public
// `Value` API (constructors and variants) already exposes it.
pub use marrow_kernel::codec::key::KeyScalar;
