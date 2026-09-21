//! The shared `.mw` fixture harness for the `marrow` crate's integration suites: the
//! in-process [`project`] path and the spawned-binary [`cli`] path over the same
//! [`Project`]. A binary that drives only the library path includes `common/project.rs`
//! alone.

pub mod cli;
pub mod project;

pub use cli::*;
pub use project::*;
