//! Type-check driver over a parsed file: return placement, operator, condition,
//! assignment, call, and saved-key argument checks.
//!
//! Split by concern: the resolved-file driver and type-annotation pass
//! (`driver`), return placement and divergence (`returns`), the statement/block
//! type driver (`statements`), collection and saved-path loop typing
//! (`collections`), range-for rules (`ranges`), the operator/condition/
//! assign/return/throw checks (`operators`), saved-access key typing
//! (`saved_keys`), call checking (`calls`), and the shared diagnostic
//! constructors (`diagnostics`).

mod calls;
mod collections;
mod diagnostics;
mod driver;
mod operators;
mod ranges;
mod returns;
mod saved_keys;
mod statements;

pub(crate) use calls::{CallCheck, check_call};
pub(crate) use collections::for_frame;
pub(crate) use diagnostics::key_type_diagnostic;
pub(crate) use driver::{
    FilePrelude, ModuleNamePolicy, ResolvedFileCheck, annotation_type_known, check_resolved_files,
    file_prelude,
};
pub(crate) use operators::{CoalesceCheck, check_binary, check_coalesce, check_unary};
pub(crate) use ranges::check_range_value;
pub(crate) use saved_keys::{SavedKeyArgCheck, check_saved_key_args};
pub(crate) use statements::{
    TransformBlockTypeCheck, check_block_types, check_transform_block_types,
};
