//! The schema diagnostic vocabulary: [`SchemaError`], its typed
//! [`SchemaErrorKind`] payloads and target enums, the stable `schema.*` codes,
//! and the message constructors. Prose is render-only; callers assert the kind
//! and code.

use std::fmt;

use marrow_syntax::SourceSpan;

use crate::Type;

/// An error found while compiling a resource into a schema.
///
/// `code` is a stable `schema.*` identifier; `message` is human-readable; and
/// `span` points at the offending declaration. `kind` carries the semantic fact
/// tests and downstream callers should assert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    pub kind: SchemaErrorKind,
    pub code: &'static str,
    pub message: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaErrorKind {
    DuplicateMember {
        target: SchemaDuplicateTarget,
        name: String,
    },
    CategoryLeaf {
        member: String,
    },
    ParentNotCategory {
        member: String,
    },
    UnknownInSaved {
        target: SchemaSavedUnknownTarget,
        name: String,
    },
    KeyMemberCollision {
        collision: SchemaNameCollision,
    },
    UnknownIndexArg {
        index: String,
        arg: String,
    },
    UnorderableKey {
        target: SchemaKeyTarget,
        ty: Type,
    },
    IndexMissingIdentityKeys {
        index: String,
    },
    IndexRequiresKeyedRoot {
        index: String,
    },
    NestedIndexArg {
        index: String,
        arg: String,
    },
    NonEnumNamedField {
        field: String,
        ty: String,
    },
    NonScalarKey {
        target: SchemaKeyTarget,
        ty: Type,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaStoreInvalidation {
    Store,
    Index { name: String },
}

impl SchemaErrorKind {
    pub fn store_invalidation(&self) -> Option<SchemaStoreInvalidation> {
        match self {
            Self::DuplicateMember {
                target: SchemaDuplicateTarget::ResourceMember | SchemaDuplicateTarget::EnumMember,
                ..
            } => None,
            Self::DuplicateMember {
                target: SchemaDuplicateTarget::KeyParam,
                ..
            } => Some(SchemaStoreInvalidation::Store),
            Self::DuplicateMember {
                target: SchemaDuplicateTarget::Index,
                name,
            } => Some(SchemaStoreInvalidation::Index { name: name.clone() }),
            Self::CategoryLeaf { .. }
            | Self::ParentNotCategory { .. }
            | Self::NonEnumNamedField { .. } => None,
            Self::UnknownInSaved { target, .. } => match target {
                SchemaSavedUnknownTarget::Field
                | SchemaSavedUnknownTarget::Key
                | SchemaSavedUnknownTarget::KeyedLeaf => None,
                SchemaSavedUnknownTarget::IdentityKey => Some(SchemaStoreInvalidation::Store),
            },
            Self::KeyMemberCollision { collision } => match collision {
                SchemaNameCollision::IdentityKeyWithMember { .. } => {
                    Some(SchemaStoreInvalidation::Store)
                }
                SchemaNameCollision::IdentityKeyWithIndex { index, .. } => {
                    Some(SchemaStoreInvalidation::Index {
                        name: index.clone(),
                    })
                }
            },
            Self::UnknownIndexArg { index, .. }
            | Self::IndexMissingIdentityKeys { index }
            | Self::IndexRequiresKeyedRoot { index }
            | Self::NestedIndexArg { index, .. } => Some(SchemaStoreInvalidation::Index {
                name: index.clone(),
            }),
            Self::UnorderableKey { target, .. } | Self::NonScalarKey { target, .. } => match target
            {
                SchemaKeyTarget::IdentityKey { .. } => Some(SchemaStoreInvalidation::Store),
                SchemaKeyTarget::KeyParam { .. } => None,
                SchemaKeyTarget::IndexArg { index, .. } => Some(SchemaStoreInvalidation::Index {
                    name: index.clone(),
                }),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaDuplicateTarget {
    ResourceMember,
    EnumMember,
    KeyParam,
    Index,
}

impl SchemaDuplicateTarget {
    pub(crate) fn message_name(self) -> &'static str {
        match self {
            Self::ResourceMember => "resource member",
            Self::EnumMember => "enum member",
            Self::KeyParam => "key",
            Self::Index => "index",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaSavedUnknownTarget {
    Field,
    IdentityKey,
    Key,
    KeyedLeaf,
}

impl SchemaSavedUnknownTarget {
    fn message_name(self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::IdentityKey => "identity key",
            Self::Key => "key",
            Self::KeyedLeaf => "keyed leaf",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaNameCollision {
    IdentityKeyWithMember { key: String },
    IdentityKeyWithIndex { key: String, index: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaKeyTarget {
    IdentityKey { name: String },
    KeyParam { name: String },
    IndexArg { index: String, arg: String },
}

/// A resource member name collides with another member at the same level.
pub const SCHEMA_DUPLICATE_MEMBER: &str = "schema.duplicate_member";

/// A `category` enum member has no nested members. A category groups its
/// descendants, so one with nothing under it can never be selected as a value nor
/// matched, leaving it dead.
pub const SCHEMA_CATEGORY_LEAF: &str = "schema.category_leaf";

/// A non-`category` enum member has nested members. A member with children is a
/// grouping node: a value selects one of its descendants, never the node itself,
/// and a `match` covers its leaves, never the node. Marking such a parent
/// `category` is what keeps the two value-validity notions aligned — value position
/// rejects exactly the categories, while `match` covers exactly the childless
/// non-categories — so a parent left unmarked would be a legal value no arm could
/// cover. The invariant category <=> has-children makes that fail-open impossible.
pub const SCHEMA_PARENT_NOT_CATEGORY: &str = "schema.parent_not_category";

/// A managed saved field or key is typed `unknown`. `unknown` is a dynamic
/// boundary value; saved schemas use concrete field and key types. Local-only
/// resources may use `unknown`.
pub const SCHEMA_UNKNOWN_IN_SAVED: &str = "schema.unknown_in_saved";

/// A top-level field or layer shares a name with an identity key. Identity keys
/// live in the saved path, so a stored member of the same name is ambiguous.
pub const SCHEMA_KEY_MEMBER_COLLISION: &str = "schema.key_member_collision";

/// An index argument does not resolve to an identity key or a top-level field.
/// Index arguments do not walk keyed child layers or unkeyed group descendants.
pub const SCHEMA_UNKNOWN_INDEX_ARG: &str = "schema.unknown_index_arg";

/// A saved key (an identity key, a keyed-layer key parameter, or an index
/// argument) has a type with no order-preserving key encoding — currently
/// `decimal`. Saved keys use ordered key types; the store cannot encode a
/// decimal as a key, so the write planner could never maintain such an entry.
/// Reject it at compile time rather than commit data with an unmaintained index
/// or key.
pub const SCHEMA_UNORDERABLE_KEY: &str = "schema.unorderable_key";

/// A non-unique index does not end with all identity keys in declaration order.
/// A non-unique entry is a presence marker, so two records sharing the indexed
/// values would collapse onto one entry unless the identity keys make each entry
/// distinct. A unique index is exempt: each populated entry already points to one
/// identity.
pub const SCHEMA_INDEX_MISSING_IDENTITY_KEYS: &str = "schema.index_missing_identity_keys";

/// An index is declared on a store with no keyed saved root. Declared indexes
/// need a store identity for entries to point to.
pub const SCHEMA_INDEX_REQUIRES_KEYED_ROOT: &str = "schema.index_requires_keyed_root";

/// An index argument names a field nested through an unkeyed group. The write
/// planner matches index arguments by flat top-level name, so it would silently
/// never maintain such an entry. Until nested index resolution lands, reject it.
pub const SCHEMA_NESTED_INDEX_ARG: &str = "schema.nested_index_arg";

/// A managed saved field's type is a bare name that is not a declared enum. A
/// saved field stores a scalar or a checked enum value; an undefined name or a
/// resource type has no saved leaf form, so it cannot be a saved field.
pub const SCHEMA_NON_ENUM_NAMED_FIELD: &str = "schema.non_enum_named_field";

/// A saved key (an identity key, a keyed-layer key parameter, or an index
/// argument) is typed as a non-scalar. A key must be an orderable scalar, because
/// the store projects a key from its scalar value. Every bare or qualified name
/// in identity-key and keyed-layer positions — a local enum, a cross-module enum,
/// a resource, or a typo — every sequence, and every store identity is rejected
/// structurally, since the rule asks only whether the type is an orderable scalar.
pub const SCHEMA_NONSCALAR_KEY: &str = "schema.nonscalar_key";

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}",
            self.span.line, self.span.column, self.code, self.message
        )
    }
}

impl std::error::Error for SchemaError {}

pub(crate) fn key_member_collision_error(key: &str, span: SourceSpan) -> SchemaError {
    SchemaError {
        kind: SchemaErrorKind::KeyMemberCollision {
            collision: SchemaNameCollision::IdentityKeyWithMember {
                key: key.to_string(),
            },
        },
        code: SCHEMA_KEY_MEMBER_COLLISION,
        message: format!(
            "identity key `{key}` collides with a top-level member of the same \
             name; identity keys live in the saved path, not stored members"
        ),
        span,
    }
}

pub(crate) fn index_requires_keyed_root_error(index: &str, span: SourceSpan) -> SchemaError {
    SchemaError {
        kind: SchemaErrorKind::IndexRequiresKeyedRoot {
            index: index.to_string(),
        },
        code: SCHEMA_INDEX_REQUIRES_KEYED_ROOT,
        message: format!(
            "index `{index}` requires a keyed saved root; a singleton store has no \
             identity for an index entry to point to"
        ),
        span,
    }
}

pub(crate) fn key_index_collision_error(index: &str, span: SourceSpan) -> SchemaError {
    SchemaError {
        kind: SchemaErrorKind::KeyMemberCollision {
            collision: SchemaNameCollision::IdentityKeyWithIndex {
                key: index.to_string(),
                index: index.to_string(),
            },
        },
        code: SCHEMA_KEY_MEMBER_COLLISION,
        message: format!(
            "identity key `{index}` collides with index `{index}`; identity keys \
             and indexes share the store namespace"
        ),
        span,
    }
}

pub(crate) fn unknown_error(
    target: SchemaSavedUnknownTarget,
    name: &str,
    span: SourceSpan,
) -> SchemaError {
    SchemaError {
        kind: SchemaErrorKind::UnknownInSaved {
            target,
            name: name.to_string(),
        },
        code: SCHEMA_UNKNOWN_IN_SAVED,
        message: format!(
            "saved {} `{name}` cannot use `unknown`; managed saved \
             schemas use concrete types",
            target.message_name()
        ),
        span,
    }
}
