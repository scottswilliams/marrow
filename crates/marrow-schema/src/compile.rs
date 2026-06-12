//! Lowering parsed declarations into schema shapes: the `compile_*` entries,
//! resource member → [`Node`] building with sequence desugaring, enum
//! flattening, and the shared `sequence` type-spelling parsing.

use std::collections::HashSet;

use marrow_syntax::{
    EnumDecl, EnumMember, FieldDecl, GroupDecl, KeyParam, ResourceDecl, ResourceMember, SourceSpan,
    StoreDecl,
};

use crate::enums::{EnumMemberSchema, EnumSchema};
use crate::errors::{
    SCHEMA_CATEGORY_LEAF, SCHEMA_DUPLICATE_MEMBER, SCHEMA_PARENT_NOT_CATEGORY,
    SchemaDuplicateTarget, SchemaError, SchemaErrorKind,
};
use crate::validate::{
    Namespace, check_duplicate_key_params, check_identity_key, check_store_index,
};
use crate::{IndexSchema, KeyDef, Node, NodeKind, ResourceSchema, ScalarType, StoreSchema, Type};

/// Compile a parsed resource declaration into a [`ResourceSchema`].
///
/// Always returns a best-effort schema together with any errors, so callers can
/// keep checking. Maps the resource tree shape and the single-resource rules the
/// schema alone can decide, including saved-field value types and keyed-layer
/// key types. Store identity keys and index arguments are checked by
/// [`compile_store`].
///
/// Deferred: full type validation and one-owner-per-root.
pub fn compile_resource(decl: &ResourceDecl) -> (ResourceSchema, Vec<SchemaError>) {
    compile_resource_shape(decl)
}

/// Compile a resource shape that is attached to at least one store declaration.
pub fn compile_stored_resource(decl: &ResourceDecl) -> (ResourceSchema, Vec<SchemaError>) {
    compile_resource_shape(decl)
}

fn compile_resource_shape(decl: &ResourceDecl) -> (ResourceSchema, Vec<SchemaError>) {
    let mut errors = Vec::new();

    let mut members = Vec::new();
    let mut names = Namespace::new(SchemaDuplicateTarget::ResourceMember);

    for member in &decl.members {
        names.check(member_name(member), member_span(member), &mut errors);
        members.push(member_node(member, &mut errors));
    }

    let schema = ResourceSchema {
        name: decl.name.clone(),
        docs: decl.docs.clone(),
        members,
    };

    (schema, errors)
}

/// Compile a parsed store declaration into a [`StoreSchema`] against the resource
/// shape it stores.
pub fn compile_store(
    decl: &StoreDecl,
    resource: &ResourceSchema,
) -> (StoreSchema, Vec<SchemaError>) {
    let mut errors = Vec::new();
    check_duplicate_key_params(&decl.root.keys, decl.span, &mut errors);
    for key in &decl.root.keys {
        check_identity_key(key, resource, decl.span, &mut errors);
    }

    let mut names = Namespace::new(SchemaDuplicateTarget::Index);
    for index in &decl.indexes {
        check_store_index(index, decl, resource, &mut names, &mut errors);
    }

    (
        StoreSchema {
            root: decl.root.root.clone(),
            resource: decl.resource.clone(),
            docs: decl.docs.clone(),
            identity_keys: decl.root.keys.iter().map(key_def).collect(),
            indexes: decl
                .indexes
                .iter()
                .map(|index| IndexSchema {
                    name: index.name.clone(),
                    docs: index.docs.clone(),
                    args: index.args.clone(),
                    unique: index.unique,
                })
                .collect(),
        },
        errors,
    )
}

/// Compile a parsed enum into an [`EnumSchema`], with any errors.
///
/// Members flatten in pre-order DFS, so each member has a source traversal index.
/// Member-name uniqueness is per sibling level (two `tiger`s under one parent
/// collide; `Cat::tiger` and `Dog::tiger` do not), reported with the shared
/// duplicate-member code so it reads like a resource's. The `category` flag and
/// having children are held in lockstep:
/// a `category` with no children is dead, and a non-`category` with children is a
/// grouping node a value could never select — both are rejected, so every parent is
/// a category and every non-category is a leaf.
pub fn compile_enum(decl: &EnumDecl) -> (EnumSchema, Vec<SchemaError>) {
    let mut errors = Vec::new();
    let mut members = Vec::new();
    flatten_enum_members(&decl.members, None, &mut members, &mut errors);
    let schema = EnumSchema {
        name: decl.name.clone(),
        docs: decl.docs.clone(),
        members,
    };
    (schema, errors)
}

/// Append one sibling level to `members` in pre-order — each member before its
/// own children — recording its parent traversal index and recursing into its
/// nested members. A duplicate name at the same level is reported and dropped, so
/// the flattened tree reflects only the distinct members.
fn flatten_enum_members(
    siblings: &[EnumMember],
    parent: Option<usize>,
    members: &mut Vec<EnumMemberSchema>,
    errors: &mut Vec<SchemaError>,
) {
    let mut seen: HashSet<&str> = HashSet::new();
    for member in siblings {
        if !seen.insert(&member.name) {
            errors.push(SchemaError {
                kind: SchemaErrorKind::DuplicateMember {
                    target: SchemaDuplicateTarget::EnumMember,
                    name: member.name.clone(),
                },
                code: SCHEMA_DUPLICATE_MEMBER,
                message: format!("duplicate enum member `{}`", member.name),
                span: member.span,
            });
            continue;
        }
        if member.category && member.members.is_empty() {
            errors.push(SchemaError {
                kind: SchemaErrorKind::CategoryLeaf {
                    member: member.name.clone(),
                },
                code: SCHEMA_CATEGORY_LEAF,
                message: format!(
                    "category `{}` has no members; a category must group nested members",
                    member.name
                ),
                span: member.span,
            });
        } else if !member.category && !member.members.is_empty() {
            errors.push(SchemaError {
                kind: SchemaErrorKind::ParentNotCategory {
                    member: member.name.clone(),
                },
                code: SCHEMA_PARENT_NOT_CATEGORY,
                message: format!(
                    "`{}` has nested members but is not a category; mark a grouping member \
                     `category`, since a value selects a concrete member under it, not the \
                     group itself",
                    member.name
                ),
                span: member.span,
            });
        }
        let ordinal = members.len();
        members.push(EnumMemberSchema {
            name: member.name.clone(),
            docs: member.docs.clone(),
            parent,
            category: member.category,
        });
        flatten_enum_members(&member.members, Some(ordinal), members, errors);
    }
}

/// The element type spelling of a `sequence[T]`, or `None` for a non-sequence
/// type. The one place the `sequence[...]` spelling is parsed; [`Type::resolve`]
/// drives off it. `sequence[T]` is sugar for the 1-based `pos: int` keyed tree.
pub(crate) fn sequence_element(text: &str) -> Option<&str> {
    text.trim()
        .strip_prefix("sequence[")
        .and_then(|rest| rest.strip_suffix(']'))
        .map(str::trim)
}

/// Compile the members nested inside a group into nodes.
fn group_members(group: &GroupDecl, errors: &mut Vec<SchemaError>) -> Vec<Node> {
    let mut members = Vec::new();
    let mut names = Namespace::new(SchemaDuplicateTarget::ResourceMember);

    for member in &group.members {
        names.check(member_name(member), member_span(member), errors);
        members.push(member_node(member, errors));
    }

    members
}

/// The name of a resource member.
fn member_name(member: &ResourceMember) -> &str {
    match member {
        ResourceMember::Field(field) => &field.name,
        ResourceMember::Group(group) => &group.name,
    }
}

/// The span of a resource member.
fn member_span(member: &ResourceMember) -> SourceSpan {
    match member {
        ResourceMember::Field(field) => field.span,
        ResourceMember::Group(group) => group.span,
    }
}

/// Compile one resource member into a [`Node`]:
/// an unkeyed plain field is a top-level `Slot`; `sequence[T]` and keyed fields
/// are keyed-leaf `Slot`s; a group is a `Group` with recursed members.
fn member_node(member: &ResourceMember, errors: &mut Vec<SchemaError>) -> Node {
    match member {
        ResourceMember::Field(field) if field.keys.is_empty() => {
            // Collection member sugar becomes a keyed `Slot` rather than a
            // plain top-level field.
            match Type::resolve(&field.ty) {
                Type::Sequence(element) => sequence_leaf(field, *element),
                ty => slot_node(field, ty, vec![], field.required),
            }
        }
        // A keyed field is a keyed-leaf layer; its declared type is the leaf type
        // and a keyed leaf never exposes `required`.
        ResourceMember::Field(field) => {
            check_duplicate_key_params(&field.keys, field.span, errors);
            slot_node(
                field,
                Type::resolve(&field.ty),
                field.keys.iter().map(key_def).collect(),
                false,
            )
        }
        ResourceMember::Group(group) => {
            check_duplicate_key_params(&group.keys, group.span, errors);
            Node {
                name: group.name.clone(),
                docs: group.docs.clone(),
                key_params: group.keys.iter().map(key_def).collect(),
                members: group_members(group, errors),
                kind: NodeKind::Group,
            }
        }
    }
}

/// A `Slot` node for `field`, carrying its value type, key parameters (empty for
/// a plain field, the keyed-leaf keys otherwise), and required flag.
fn slot_node(field: &FieldDecl, ty: Type, key_params: Vec<KeyDef>, required: bool) -> Node {
    Node {
        name: field.name.clone(),
        docs: field.docs.clone(),
        key_params,
        members: Vec::new(),
        kind: NodeKind::Slot { ty, required },
    }
}

/// Desugar `name: sequence[T]` into the keyed leaf `name(pos: int): T`. The
/// implicit `pos: int` key matches the canonical sequence spelling, so the
/// resulting node is identical to the one `name(pos: int): T` produces and
/// append/read/traverse work unchanged.
fn sequence_leaf(field: &FieldDecl, element: Type) -> Node {
    slot_node(
        field,
        element,
        vec![KeyDef {
            name: "pos".to_string(),
            ty: Type::Scalar(ScalarType::Int),
        }],
        false,
    )
}

fn key_def(key: &KeyParam) -> KeyDef {
    KeyDef {
        name: key.name.clone(),
        ty: Type::resolve(&key.ty),
    }
}
