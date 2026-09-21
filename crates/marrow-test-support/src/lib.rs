//! The test scaffolding more than one test binary builds over.
//!
//! Each fixture here is a contract, not a convenience: the scratch directory every test
//! mints its temporary files under, the admitted plan a draft is built under, the site
//! seam that is the only producer path to an operation site, the container forger that
//! names the digest slot by offset, the ledger and project builders a compiled fixture
//! is captured through, the engine doubles the kernel and VM drive commits over, and the
//! tracer corpus the verifier's pins compare. A copy per crate is a copy free to drift,
//! and drift here does not fail loudly: it yields fixtures that stop at an earlier gate,
//! and a test asserting a rejection still passes, at the wrong phase, for the wrong
//! reason.
//!
//! Every consumer names this crate as a `[dev-dependencies]` path edge, so nothing here
//! reaches a production build and no production visibility widens to serve it. A fixture
//! that needs a private constructor does not belong here; it belongs beside the tests of
//! the crate that owns the constructor. A test temporary directory is minted by
//! [`Scratch`] alone: no test under `crates/*/tests` or `crates/*/src` reaches
//! `std::env::temp_dir` itself, and the repository gate in `marrow-codes` scans for it.

pub mod counting_engine;
mod draft;
pub mod fault_engine;
pub mod fixture_graph;
mod forgery;
pub mod graph_corpus;
pub mod ids;
pub mod ledger;
pub mod ledger_ids;
mod mode;
mod output;
pub mod owned_heap;
pub mod program;
pub mod project;
mod scratch;
pub mod tracer_schema;

pub use counting_engine::{Counters, CountingEngine};
pub use draft::{admitted_plan, site};
pub use forgery::{forge, rehash};
pub use ledger_ids::id;
pub use mode::{mode_of, require_mode_bits_bind, set_mode};
pub use output::broken_output;
pub use scratch::{Scratch, file_uri};
