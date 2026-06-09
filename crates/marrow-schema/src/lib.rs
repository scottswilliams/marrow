//! Compiles parsed Marrow resource and store declarations into schema shapes.
//!
//! [`ResourceSchema`] describes the typed resource tree: fields, keyed layers,
//! groups, and saved-field value rules. [`StoreSchema`] owns the durable root,
//! identity keys, and indexes that attach a resource shape to saved data. Semantic
//! validation beyond structure is deferred; see the notes on [`compile_resource`]
//! and [`compile_store`].

mod compile;
mod enums;
mod errors;
mod types;
mod validate;

pub mod error;
pub mod stdlib;

// The canonical scalar type lives in marrow-store; re-export it so resolution
// and downstream crates share one import path for the storable scalars.
pub use marrow_store::value::ScalarType;

pub use compile::{
    compile_enum, compile_resource, compile_store, compile_stored_resource,
    contains_map_type_syntax,
};
pub use enums::{EnumMemberSchema, EnumSchema, MemberPathResolution};
pub use errors::{
    SCHEMA_CATEGORY_LEAF, SCHEMA_DUPLICATE_MEMBER, SCHEMA_INDEX_MISSING_IDENTITY_KEYS,
    SCHEMA_INDEX_REQUIRES_KEYED_ROOT, SCHEMA_KEY_MEMBER_COLLISION, SCHEMA_NESTED_INDEX_ARG,
    SCHEMA_NON_ENUM_NAMED_FIELD, SCHEMA_NONSCALAR_KEY, SCHEMA_PARENT_NOT_CATEGORY,
    SCHEMA_UNKNOWN_IN_SAVED, SCHEMA_UNKNOWN_INDEX_ARG, SCHEMA_UNORDERABLE_KEY,
    SCHEMA_UNSUPPORTED_TYPE, SchemaDuplicateTarget, SchemaError, SchemaErrorKind, SchemaKeyTarget,
    SchemaNameCollision, SchemaSavedUnknownTarget, SchemaUnsupportedTypeTarget,
};
pub use types::{
    IndexSchema, KeyDef, Node, NodeKind, ResourceSchema, SavedRootSchema, StoreSchema, Type,
};
pub use validate::{
    check_saved_member_rules, check_saved_named_member_fields, check_saved_named_member_fields_with,
};
