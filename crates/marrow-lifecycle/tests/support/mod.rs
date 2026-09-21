//! The lifecycle suites' shared fixtures over the crate's own API: the store-publication
//! path and the read-only/broadened ceiling pair the admission suites present against
//! each other. The scratch directory, the compile path and the graph corpus are the
//! workspace fixtures in `marrow_test_support`.

pub mod ceiling;
pub mod store;
