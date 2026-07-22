//! The Marrow stack virtual machine.
//!
//! The VM runs a sealed [`marrow_verify::VerifiedImage`] over its typed
//! instruction tape. It is the sole production executor on the beta line: it
//! accepts only a verified image, never raw bytes or a compiler artifact, so a
//! verifier/VM disagreement about instruction shape is unrepresentable. Runtime
//! faults are typed and source-mapped ([`RuntimeFault`]); execution runs under
//! private bounds. Durable operations route through the path kernel, wired in with
//! the durable slices.

mod attach;
mod fault;
pub mod render;
mod run;
mod value;

pub use attach::{
    DurableRun, Ephemeral, derive_store_schemas, mint_ephemeral, run_driver_test, run_durable_test,
    run_export,
};
pub use fault::{DurableExecutionFault, IncompleteDisposition, InvocationIncomplete, RuntimeFault};
pub use marrow_kernel::durable::DurableCommitState;
pub use run::{run, run_durable};
pub use value::Value;
// The key-scalar type a `Value::Map` entry and a `Value::Id` key tuple carry. It
// belongs to the kernel codec owner; the value model surfaces it because its public
// `Value` API (constructors and variants) already exposes it.
pub use marrow_kernel::codec::key::KeyScalar;
