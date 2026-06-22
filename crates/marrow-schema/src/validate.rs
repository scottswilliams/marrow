//! Single-declaration validation: duplicate-name tracking, the orderable-scalar
//! key allowlist, store identity-key and index checks, and the saved-member
//! rules (`unknown`, key types, named fields) the schema decides alone.

use std::collections::HashSet;

use marrow_syntax::{FieldDecl, IndexDecl, KeyParam, ResourceMember, SourceSpan, StoreDecl};

use crate::errors::{
    SCHEMA_DUPLICATE_MEMBER, SCHEMA_INDEX_MISSING_IDENTITY_KEYS, SCHEMA_NESTED_INDEX_ARG,
    SCHEMA_NON_ENUM_NAMED_FIELD, SCHEMA_NONSCALAR_KEY, SCHEMA_UNKNOWN_INDEX_ARG,
    SCHEMA_UNORDERABLE_KEY, SchemaDuplicateTarget, SchemaError, SchemaErrorKind, SchemaKeyTarget,
    SchemaSavedUnknownTarget, field_index_collision_error, index_requires_keyed_root_error,
    key_index_collision_error, key_member_collision_error, unknown_error,
};
use crate::{Node, NodeKind, ResourceSchema, ScalarType, Type};

/// Report saved-data member rules for a resource attached by a store
/// declaration. The check runs from the store declaration so a plain resource
/// AST stays store-independent.
///
/// [`compile_resource`]: crate::compile_resource
pub fn check_saved_member_rules(members: &[ResourceMember]) -> Vec<SchemaError> {
    let mut errors = Vec::new();
    for member in members {
        check_member_unknown(member, &mut errors);
        check_member_keys(member, &mut errors);
    }
    errors
}

/// Validate one store identity key: its type is a saved key, and its name does not
/// collide with a stored top-level member. Identity keys carry no span of their
/// own, so errors point at the store declaration's `span`.
pub(crate) fn check_identity_key(
    key: &KeyParam,
    resource: &ResourceSchema,
    span: SourceSpan,
    errors: &mut Vec<SchemaError>,
) {
    let ty = Type::resolve(&key.ty);
    if ty.embeds_unknown() {
        errors.push(unknown_error(
            SchemaSavedUnknownTarget::IdentityKey,
            &key.name,
            span,
        ));
    } else if let Some(error) = key_type_error(
        SavedKeyTarget::IdentityKey {
            name: key.name.clone(),
        },
        &ty,
        span,
    ) {
        errors.push(error);
    }
    if resource
        .members
        .iter()
        .any(|member| member.name == key.name)
    {
        errors.push(key_member_collision_error(&key.name, span));
    }
}

/// Validate one declared index: a unique name, a keyed saved root to attach to, no
/// collision with an identity key, and well-typed arguments.
pub(crate) fn check_store_index(
    index: &IndexDecl,
    decl: &StoreDecl,
    resource: &ResourceSchema,
    names: &mut Namespace,
    errors: &mut Vec<SchemaError>,
) {
    names.check(&index.name, index.span, errors);
    if resource
        .members
        .iter()
        .any(|member| member.name == index.name)
    {
        errors.push(field_index_collision_error(&index.name, index.span));
    }
    if decl.root.keys.is_empty() {
        errors.push(index_requires_keyed_root_error(&index.name, index.span));
        return;
    }
    if decl.root.keys.iter().any(|key| key.name == index.name) {
        errors.push(key_index_collision_error(&index.name, index.span));
    }
    check_store_index_args(index, &decl.root.keys, resource, errors);
}

/// Validate a keyed-layer's own key parameters, descending into groups. A keyed
/// layer's key must be a saved key, so it may not embed `unknown` and may not be
/// an unorderable type such as `decimal`. A keyed leaf and a keyed group both
/// carry their key parameters in `keys`; an unkeyed field or group has none.
/// Identity keys are checked by store compilation.
fn check_member_keys(member: &ResourceMember, errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => {
            check_key_params(&field.keys, field.span, errors);
        }
        ResourceMember::Group(group) => {
            check_key_params(&group.keys, group.span, errors);
            for nested in &group.members {
                check_member_keys(nested, errors);
            }
        }
    }
}

/// Report each key parameter whose type cannot be a saved key. Key params have
/// no span of their own, so errors point at the keyed layer's `span`.
fn check_key_params(keys: &[KeyParam], span: SourceSpan, errors: &mut Vec<SchemaError>) {
    for key in keys {
        let ty = Type::resolve(&key.ty);
        if ty.embeds_unknown() {
            errors.push(unknown_error(
                SchemaSavedUnknownTarget::Key,
                &key.name,
                span,
            ));
        } else if let Some(error) = key_type_error(
            SavedKeyTarget::KeyParam {
                name: key.name.clone(),
            },
            &ty,
            span,
        ) {
            errors.push(error);
        }
    }
}

/// Reject `unknown` on the value type of a field or keyed leaf, descending into
/// groups. A keyed layer's own key parameters are validated separately in
/// [`check_member_keys`].
fn check_member_unknown(member: &ResourceMember, errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => {
            let ty = Type::resolve(&field.ty);
            let target = if field.keys.is_empty() {
                SchemaSavedUnknownTarget::Field
            } else {
                SchemaSavedUnknownTarget::KeyedLeaf
            };
            if ty.embeds_unknown() {
                errors.push(unknown_error(target, &field.name, field.span));
            }
        }
        ResourceMember::Group(group) => {
            for nested in &group.members {
                check_member_unknown(nested, errors);
            }
        }
    }
}

/// Apply the saved plain-field named-type rule directly to resource members.
/// Split store declarations use this after resolving the resource they attach.
pub fn check_saved_named_member_fields(
    members: &[ResourceMember],
    enums: &[String],
) -> Vec<SchemaError> {
    check_saved_named_member_fields_with(members, |name| {
        name.contains("::") || enums.iter().any(|enum_name| enum_name == name)
    })
}

/// Apply the saved plain-field named-type rule with a project-aware enum
/// resolver. Schema compilation only knows same-file enum names; the checker
/// supplies a resolver for qualified names after imports and module visibility
/// are known.
pub fn check_saved_named_member_fields_with(
    members: &[ResourceMember],
    mut is_declared_enum_name: impl FnMut(&str) -> bool,
) -> Vec<SchemaError> {
    let mut errors = Vec::new();
    walk_saved_named_member_fields(members, |field, name| {
        if !is_declared_enum_name(name) {
            errors.push(non_enum_named_field_error(field, name));
        }
    });
    errors
}

pub fn walk_saved_named_member_fields(
    members: &[ResourceMember],
    mut visit: impl FnMut(&FieldDecl, &str),
) {
    for member in members {
        walk_saved_named_member(member, &mut visit);
    }
}

fn walk_saved_named_member(member: &ResourceMember, visit: &mut impl FnMut(&FieldDecl, &str)) {
    match member {
        ResourceMember::Field(field) => {
            let ty = Type::resolve(&field.ty);
            if field.keys.is_empty() {
                walk_named_field_type(&ty, field, visit);
            }
        }
        ResourceMember::Group(group) => {
            for nested in &group.members {
                walk_saved_named_member(nested, visit);
            }
        }
    }
}

fn walk_named_field_type(ty: &Type, field: &FieldDecl, visit: &mut impl FnMut(&FieldDecl, &str)) {
    match ty {
        Type::Named(name) => visit(field, name),
        Type::Sequence(element) => {
            walk_named_field_type(element, field, visit);
        }
        _ => {}
    }
}

pub fn non_enum_named_field_error(field: &FieldDecl, name: &str) -> SchemaError {
    SchemaError {
        kind: SchemaErrorKind::NonEnumNamedField {
            field: field.name.clone(),
            ty: name.to_string(),
        },
        code: SCHEMA_NON_ENUM_NAMED_FIELD,
        message: format!(
            "saved field `{}` has type `{name}`, which is not a declared enum; \
             a saved field stores a scalar or checked enum value",
            field.name
        ),
        span: field.name_span,
    }
}

fn check_store_index_args(
    index: &IndexDecl,
    keys: &[KeyParam],
    resource: &ResourceSchema,
    errors: &mut Vec<SchemaError>,
) {
    for (position, arg) in index.args.iter().enumerate() {
        // Point a per-argument rejection at the argument itself, falling back to
        // the declaration when the parser recorded no span for it.
        let arg_span = index.arg_spans.get(position).copied().unwrap_or(index.span);
        match store_index_arg_type(arg, keys, resource) {
            None if store_index_arg_is_nested_field(arg, resource) => {
                errors.push(SchemaError {
                    kind: SchemaErrorKind::NestedIndexArg {
                        index: index.name.clone(),
                        arg: arg.clone(),
                    },
                    code: SCHEMA_NESTED_INDEX_ARG,
                    message: format!(
                        "index `{}` argument `{arg}` names a field nested through an \
                         unkeyed group, which the write planner does not maintain",
                        index.name
                    ),
                    span: arg_span,
                });
            }
            None => {
                let target = SchemaKeyTarget::IndexArg {
                    index: index.name.clone(),
                    arg: arg.clone(),
                };
                errors.push(match top_level_keyed_layer(arg, resource) {
                    Some(layer) => index_arg_nonscalar_key_error(target, &layer, arg_span),
                    None => SchemaError {
                        kind: SchemaErrorKind::UnknownIndexArg {
                            index: index.name.clone(),
                            arg: arg.clone(),
                        },
                        code: SCHEMA_UNKNOWN_INDEX_ARG,
                        message: format!(
                            "index `{}` argument `{arg}` does not name an identity \
                             key or top-level field",
                            index.name
                        ),
                        span: arg_span,
                    },
                });
            }
            Some(ty) => {
                if let Some(error) = index_arg_type_key_error(&index.name, arg, &ty, arg_span) {
                    errors.push(error);
                }
            }
        }
    }
    if !index.unique && !ends_with_identity_keys(&index.args, keys) {
        errors.push(SchemaError {
            kind: SchemaErrorKind::IndexMissingIdentityKeys {
                index: index.name.clone(),
            },
            code: SCHEMA_INDEX_MISSING_IDENTITY_KEYS,
            message: format!(
                "non-unique index `{}` must end with all identity key(s) in \
                 declaration order so each entry is distinct",
                index.name
            ),
            span: index.span,
        });
    }
}

fn store_index_arg_type(arg: &str, keys: &[KeyParam], resource: &ResourceSchema) -> Option<Type> {
    if arg.contains('.') {
        return None;
    }
    if let Some(key) = keys.iter().find(|key| key.name == arg) {
        return Some(Type::resolve(&key.ty));
    }
    resource.field_type(&[arg]).cloned()
}

fn store_index_arg_is_nested_field(arg: &str, resource: &ResourceSchema) -> bool {
    if arg.contains('.') {
        let segments: Vec<&str> = arg.split('.').collect();
        return resource.field_type(&segments).is_some();
    }
    resource
        .members
        .iter()
        .any(|node| node_has_nested_field_named(node, arg))
}

fn node_has_nested_field_named(node: &Node, name: &str) -> bool {
    if !node.key_params.is_empty() {
        return false;
    }
    matches!(node.kind, NodeKind::Group)
        && node.members.iter().any(|member| {
            (member.name == name && member.is_plain_field())
                || node_has_nested_field_named(member, name)
        })
}

/// Does this index argument list end with all identity key names in declaration
/// order? A non-unique entry is a presence marker, so without the trailing
/// identity keys two records sharing the indexed values collapse onto one entry.
fn ends_with_identity_keys(args: &[String], keys: &[KeyParam]) -> bool {
    args.len() >= keys.len()
        && args[args.len() - keys.len()..]
            .iter()
            .zip(keys)
            .all(|(arg, key)| arg == &key.name)
}

/// Why a type may not be a saved key, or `Ok` when it is a valid one. Saved keys
/// project from an orderable scalar value, so the rule is an allowlist: every
/// scalar except `decimal` is a key; everything else is rejected. The verdict
/// needs no knowledge of what a name refers to, so a local enum, a cross-module
/// enum, a resource, a typo, and a store identity are all the same
/// `NonScalar` case, caught structurally without an enum or resource list.
enum KeyTypeVerdict {
    Ok,
    /// `decimal` — a scalar, but the one with no order-preserving key encoding.
    Decimal,
    /// An identity, a name, or a sequence, none of which projects to an
    /// orderable scalar key.
    NonScalar,
}

enum SavedKeyTarget {
    IdentityKey { name: String },
    KeyParam { name: String },
    LocalKey { name: String },
}

impl SavedKeyTarget {
    /// The leading phrase a key-type message uses for this target, including the
    /// `saved` qualifier where it applies. A local key is not saved, so it carries
    /// its own qualifier rather than the saved prefix.
    fn label(&self) -> &'static str {
        match self {
            Self::IdentityKey { .. } => "saved identity key",
            Self::KeyParam { .. } => "saved key",
            Self::LocalKey { .. } => "local keyed-collection key",
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::IdentityKey { name } | Self::KeyParam { name } | Self::LocalKey { name } => name,
        }
    }

    fn into_schema_key_target(self) -> SchemaKeyTarget {
        match self {
            Self::IdentityKey { name } => SchemaKeyTarget::IdentityKey { name },
            Self::KeyParam { name } => SchemaKeyTarget::KeyParam { name },
            Self::LocalKey { name } => SchemaKeyTarget::LocalKey { name },
        }
    }
}

/// Classify a key type. `decimal` is the one scalar the store cannot encode as a
/// key; every other scalar is orderable. A store identity, name, or sequence
/// is a non-scalar key.
fn classify_key_type(ty: &Type) -> KeyTypeVerdict {
    match ty {
        Type::Scalar(ScalarType::Decimal) => KeyTypeVerdict::Decimal,
        Type::Scalar(_) => KeyTypeVerdict::Ok,
        Type::Identity(_) | Type::Named(_) | Type::Sequence(_) | Type::Unknown => {
            KeyTypeVerdict::NonScalar
        }
    }
}

/// The error a key of type `ty` earns in an identity-key or keyed-layer position,
/// or `None` if it is a valid key. `decimal` keeps its own "no key encoding"
/// message and code; any other non-scalar is the orderable-scalar rule. `unknown`
/// is reported separately by the caller, so it does not reach here.
fn key_type_error(target: SavedKeyTarget, ty: &Type, span: SourceSpan) -> Option<SchemaError> {
    let what = target.label();
    let name = target.name().to_string();
    let target = target.into_schema_key_target();
    match classify_key_type(ty) {
        KeyTypeVerdict::Ok => None,
        KeyTypeVerdict::Decimal => Some(SchemaError {
            kind: SchemaErrorKind::UnorderableKey {
                target,
                ty: ty.clone(),
            },
            code: SCHEMA_UNORDERABLE_KEY,
            message: format!(
                "{what} `{name}` cannot use `decimal`; keys use ordered key types \
                 and `decimal` has no key encoding"
            ),
            span,
        }),
        KeyTypeVerdict::NonScalar => Some(SchemaError {
            kind: SchemaErrorKind::NonScalarKey {
                target,
                ty: ty.clone(),
            },
            code: SCHEMA_NONSCALAR_KEY,
            message: format!("{what} `{name}` must be an orderable scalar type, but found `{ty}`"),
            span,
        }),
    }
}

/// The error a local keyed-collection key of type `ty` earns, or `None` if it is a
/// valid key. A local keyed `var` and a keyed function parameter hold no saved data,
/// but a key still projects from an orderable scalar, so they obey the same key-type
/// allowlist as a saved keyed layer through the one [`classify_key_type`] owner.
pub fn local_key_type_error(name: &str, ty: &Type, span: SourceSpan) -> Option<SchemaError> {
    key_type_error(
        SavedKeyTarget::LocalKey {
            name: name.to_string(),
        },
        ty,
        span,
    )
}

/// The error an index argument of type `resolved` earns, sharing the orderable-key
/// verdict ([`classify_key_type`]) with identity keys and key parameters. Index
/// arguments have two declared-field exceptions: a `Named` may resolve to an enum
/// later, and an identity field indexes by a store-prefixed identity payload.
fn index_arg_type_key_error(
    index: &str,
    arg: &str,
    resolved: &Type,
    span: SourceSpan,
) -> Option<SchemaError> {
    if matches!(resolved, Type::Identity(_) | Type::Named(_)) {
        return None;
    }
    let target = SchemaKeyTarget::IndexArg {
        index: index.to_string(),
        arg: arg.to_string(),
    };
    match classify_key_type(resolved) {
        KeyTypeVerdict::Ok => None,
        KeyTypeVerdict::Decimal => Some(SchemaError {
            kind: SchemaErrorKind::UnorderableKey {
                target,
                ty: resolved.clone(),
            },
            code: SCHEMA_UNORDERABLE_KEY,
            message: format!(
                "index `{index}` argument `{arg}` is a `decimal`, which has no key \
                 encoding; index arguments use ordered key types"
            ),
            span,
        }),
        KeyTypeVerdict::NonScalar => Some(index_arg_nonscalar_key_error(target, resolved, span)),
    }
}

/// The `nonscalar_key` error an index argument earns. A keyed-layer member and a
/// scalar field typed as a sequence, identity, or name all reduce to one fact:
/// the argument names a value with no orderable-scalar projection.
fn index_arg_nonscalar_key_error(
    target: SchemaKeyTarget,
    ty: &Type,
    span: SourceSpan,
) -> SchemaError {
    let (index, arg) = match &target {
        SchemaKeyTarget::IndexArg { index, arg } => (index.as_str(), arg.as_str()),
        _ => unreachable!("index-argument key target"),
    };
    SchemaError {
        message: format!(
            "index `{index}` argument `{arg}` must be an orderable scalar type, \
             but found `{ty}`"
        ),
        kind: SchemaErrorKind::NonScalarKey {
            target,
            ty: ty.clone(),
        },
        code: SCHEMA_NONSCALAR_KEY,
        span,
    }
}

/// The top-level keyed-layer member named `arg`, as the sequence-shaped type it
/// projects to (its leaf value type wrapped in a `sequence`). A keyed leaf and a
/// `sequence[T]` field — which desugars to one — are both layers, not a single
/// orderable value, so neither addresses an index entry.
fn top_level_keyed_layer(arg: &str, resource: &ResourceSchema) -> Option<Type> {
    let layer = resource
        .members
        .iter()
        .find(|node| node.name == arg && !node.key_params.is_empty())?;
    let leaf = layer.leaf_value_type()?.clone();
    Some(Type::Sequence(Box::new(leaf)))
}

/// Report a keyed layer's key parameters that repeat a name. Key params share a
/// per-layer namespace; two keys of the same name are unaddressable. Key params
/// have no span of their own, so errors point at the layer's `span`.
pub(crate) fn check_duplicate_key_params(
    keys: &[KeyParam],
    span: SourceSpan,
    errors: &mut Vec<SchemaError>,
) {
    let mut names = Namespace::new(SchemaDuplicateTarget::KeyParam);
    for key in keys {
        names.check(&key.name, span, errors);
    }
}

/// Tracks member names seen at one nesting level so duplicates can be reported.
/// Fields, layers, and indexes share one flat namespace per level.
pub(crate) struct Namespace {
    seen: HashSet<String>,
    target: SchemaDuplicateTarget,
}

impl Namespace {
    pub(crate) fn new(target: SchemaDuplicateTarget) -> Self {
        Self {
            seen: HashSet::new(),
            target,
        }
    }

    pub(crate) fn check(&mut self, name: &str, span: SourceSpan, errors: &mut Vec<SchemaError>) {
        if !self.seen.insert(name.to_string()) {
            errors.push(SchemaError {
                kind: SchemaErrorKind::DuplicateMember {
                    target: self.target,
                    name: name.to_string(),
                },
                code: SCHEMA_DUPLICATE_MEMBER,
                message: format!("duplicate {} `{name}`", self.target.message_name()),
                span,
            });
        }
    }
}
