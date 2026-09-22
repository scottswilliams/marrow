//! The test scaffolding more than one binary builds over: the one scratch directory, the
//! image draft seam and forger, the ledger-id fixtures, the engine doubles and the
//! verifier's tracer corpus, plus captured project inputs and their ledger writer.
//! It does not depend on the compiler. Every production crate reaches it only through
//! `[dev-dependencies]`, so nothing here reaches a production build.

pub mod counting_engine;
mod draft;
pub mod fault_engine;
pub mod fixture_graph;
mod forgery;
pub mod graph_corpus;
pub mod ledger;
pub mod ledger_ids;
mod mode;
mod output;
pub mod owned_heap;
pub mod project;
mod scratch;
pub mod tracer_schema;

pub use counting_engine::{Counters, CountingEngine};
pub use draft::{admitted_plan, site};
pub use forgery::{forge, rehash};
pub use ledger_ids::id;
pub use mode::{mode_of, require_mode_bits_bind, set_mode};
pub use output::broken_output;
pub use scratch::{MANIFEST, Scratch, file_uri};
