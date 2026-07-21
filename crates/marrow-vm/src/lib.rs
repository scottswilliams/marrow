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
pub use fault::RuntimeFault;
pub use run::{run, run_durable};
pub use value::Value;
