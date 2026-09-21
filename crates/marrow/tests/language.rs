//! Language semantics through the production path: types, collections, enums, generics,
//! groups, temporal values, bounds, and operation sites.

mod common;

#[path = "language/collections.rs"]
mod collections;
#[path = "language/conformance_digests.rs"]
mod conformance_digests;
#[path = "language/enum_types.rs"]
mod enum_types;
#[path = "language/generics.rs"]
mod generics;
#[path = "language/groups.rs"]
mod groups;
#[path = "language/int_bounds.rs"]
mod int_bounds;
#[path = "language/local_sparse.rs"]
mod local_sparse;
#[path = "language/nominal_ints.rs"]
mod nominal_ints;
#[path = "language/operation_sites.rs"]
mod operation_sites;
#[path = "language/option_result.rs"]
mod option_result;
#[path = "language/optional_exists.rs"]
mod optional_exists;
#[path = "language/semantic_paths.rs"]
mod semantic_paths;
#[path = "language/struct_types.rs"]
mod struct_types;
#[path = "language/temporal.rs"]
mod temporal;
#[path = "language/traversal_bounds.rs"]
mod traversal_bounds;
#[path = "language/value_size_boundary.rs"]
mod value_size_boundary;
