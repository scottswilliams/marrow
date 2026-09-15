//! The lifecycle suites' shared fixtures. Each concept has exactly one owner here: the
//! scratch directory, the capture/compile path, the multi-shape graph corpus, and the
//! read-only/broadened ceiling pair the admission suites present against each other.
//!
//! The in-crate suites reach the same files through `crate::test_support`, so an in-crate
//! and an integration case cannot disagree about what a fixture contains.

pub mod actor_fixtures;
pub mod ceiling;
pub mod compile;
mod scratch;
pub mod store;

pub use scratch::Scratch;
