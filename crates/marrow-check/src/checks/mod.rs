//! Type-check driver over a parsed file: return placement, operator, condition,
//! assignment, call, and saved-key argument checks.
//!
//! Split by concern: the resolved-file driver and type-annotation pass
//! (`driver`), return placement and divergence (`returns`), the statement/block
//! type driver (`statements`), collection and saved-path loop typing
//! (`collections`), range-for rules (`ranges`), the operator/condition/
//! assign/return/throw checks (`operators`), saved-access key typing
//! (`saved_keys`), straight-line required-field assignment (`required_fields`),
//! call checking (`calls`), the statically-known integer fold for sequence
//! positions (`const_int`), and the shared diagnostic constructors
//! (`diagnostics`).

mod calls;
mod collections;
mod const_int;
mod diagnostics;
mod driver;
mod operators;
mod ranges;
mod required_fields;
mod returns;
mod saved_keys;
mod statements;

pub(crate) use calls::{CallCheck, check_call, materializes_saved_collection_by_value};
pub(crate) use collections::{catch_frame, check_entries_value_position, for_frame};
pub(crate) use diagnostics::key_type_diagnostic;
pub(crate) use driver::{
    FilePrelude, ModuleNamePolicy, ResolvedFileCheck, check_resolved_files, file_prelude,
};
pub(crate) use operators::{CoalesceCheck, check_binary, check_coalesce, check_unary};
pub(crate) use ranges::check_range_value;
pub(crate) use saved_keys::{
    SavedKeyArgCheck, check_saved_key_args, saved_root_args_address_record,
};
pub(crate) use statements::{
    TransformBlockTypeCheck, check_block_types, check_transform_block_types,
};
