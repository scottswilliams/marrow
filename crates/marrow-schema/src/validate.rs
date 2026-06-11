//! Single-declaration validation: duplicate-name tracking, the orderable-scalar
//! key allowlist, store identity-key and index checks, and the saved-member
//! rules (`unknown`, key types, named fields) the schema decides alone.

use std::collections::HashSet;

use marrow_syntax::{FieldDecl, IndexDecl, KeyParam, ResourceMember, SourceSpan, StoreDecl};

use crate::compile::{MapLeaf, contains_map_type};
use crate::errors::{
    SCHEMA_DUPLICATE_MEMBER, SCHEMA_INDEX_MISSING_IDENTITY_KEYS, SCHEMA_NESTED_INDEX_ARG,
    SCHEMA_NON_ENUM_NAMED_FIELD, SCHEMA_NONSCALAR_KEY, SCHEMA_UNKNOWN_INDEX_ARG,
    SCHEMA_UNORDERABLE_KEY, SchemaDuplicateTarget, SchemaError, SchemaErrorKind, SchemaKeyTarget,
    SchemaSavedUnknownTarget, index_requires_keyed_root_error, key_index_collision_error,
    key_member_collision_error, unknown_error, unsupported_map_field_error,
    unsupported_map_key_error, unsupported_map_key_param_error,
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
    if let Some(error) = unsupported_map_key_param_error(key, span) {
        errors.push(error);
        return;
    }
    let ty = Type::resolve(&key.ty);
    if ty.embeds_unknown() {
        errors.push(unknown_error(
            SchemaSavedUnknownTarget::IdentityKey,
            &key.name,
            span,
        ));
    } else if let Some(error) = key_type_error(
        SchemaKeyTarget::IdentityKey {
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
            if let Some(map) = MapLeaf::resolve(field) {
                check_synthetic_map_key(&map, field.span, errors);
            }
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
        if let Some(error) = unsupported_map_key_param_error(key, span) {
            errors.push(error);
            continue;
        }
        let ty = Type::resolve(&key.ty);
        if ty.embeds_unknown() {
            errors.push(unknown_error(
                SchemaSavedUnknownTarget::Key,
                &key.name,
                span,
            ));
        } else if let Some(error) = key_type_error(
            SchemaKeyTarget::KeyParam {
                name: key.name.clone(),
            },
            &ty,
            span,
        ) {
            errors.push(error);
        }
    }
}

/// Validate the synthetic `key: K` of a `map[K, V]` member from its resolved
/// decomposition: a nested-map key is unsupported, and otherwise the key obeys the
/// saved-key rules just as a written key parameter does.
fn check_synthetic_map_key(map: &MapLeaf, span: SourceSpan, errors: &mut Vec<SchemaError>) {
    if map.key_is_map {
        errors.push(unsupported_map_key_error("key", span));
        return;
    }
    if map.key.embeds_unknown() {
        errors.push(unknown_error(SchemaSavedUnknownTarget::Key, "key", span));
    } else if let Some(error) = key_type_error(
        SchemaKeyTarget::KeyParam {
            name: "key".to_string(),
        },
        &map.key,
        span,
    ) {
        errors.push(error);
    }
}

/// Reject `unknown` on the value type of a field or keyed leaf, descending into
/// groups. A keyed layer's own key parameters are validated separately in
/// [`check_member_keys`].
fn check_member_unknown(member: &ResourceMember, errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => {
            // A keyed leaf carries its value type the same way a plain field
            // does; both reject `unknown`.
            let (target, ty) = if let Some(map) = MapLeaf::resolve(field) {
                (SchemaSavedUnknownTarget::KeyedLeaf, map.value)
            } else if field.keys.is_empty() {
                (SchemaSavedUnknownTarget::Field, Type::resolve(&field.ty))
            } else {
                (
                    SchemaSavedUnknownTarget::KeyedLeaf,
                    Type::resolve(&field.ty),
                )
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

pub(crate) fn check_unsupported_map_types(
    members: &[ResourceMember],
    saved_map_sugar: bool,
    errors: &mut Vec<SchemaError>,
) {
    for member in members {
        match member {
            ResourceMember::Field(field) => {
                if !saved_map_sugar {
                    check_unsupported_map_key_params(&field.keys, field.span, errors);
                }
                errors.extend(unsupported_map_field(field, saved_map_sugar));
            }
            ResourceMember::Group(group) => {
                if !saved_map_sugar {
                    check_unsupported_map_key_params(&group.keys, group.span, errors);
                }
                check_unsupported_map_types(&group.members, saved_map_sugar, errors);
            }
        }
    }
}

/// The field-level `map[...]` rejection a field earns, or `None` when its type is
/// not a `map[...]` or is an accepted saved keyed-leaf member.
///
/// A top-level `map[K, V]` field under `saved_map_sugar` is accepted exactly when
/// [`MapLeaf::is_supported_member`] holds. A required member or a nested value is
/// rejected here; a nested *key* is left to [`check_synthetic_map_key`], which
/// owns the key position, so it is not reported twice. Any other `map[...]`
/// spelling — a local resource, a keyed leaf whose value is a map, or a map under
/// a `sequence[...]` — is unsupported.
fn unsupported_map_field(field: &FieldDecl, saved_map_sugar: bool) -> Option<SchemaError> {
    if saved_map_sugar && let Some(map) = MapLeaf::resolve(field) {
        if map.is_supported_member(field) {
            return None;
        }
        return (field.required || map.value_is_map).then(|| unsupported_map_field_error(field));
    }
    contains_map_type(&field.ty.text).then(|| unsupported_map_field_error(field))
}

fn check_unsupported_map_key_params(
    keys: &[KeyParam],
    span: SourceSpan,
    errors: &mut Vec<SchemaError>,
) {
    for key in keys {
        if let Some(error) = unsupported_map_key_param_error(key, span) {
            errors.push(error);
        }
    }
}

/// Apply the saved named-field rule directly to resource members. Split store
/// declarations use this after resolving the resource they attach.
pub fn check_saved_named_member_fields(
    members: &[ResourceMember],
    enums: &[String],
) -> Vec<SchemaError> {
    check_saved_named_member_fields_with(members, |name| {
        name.contains("::") || enums.iter().any(|enum_name| enum_name == name)
    })
}

/// Apply the saved named-field rule with a project-aware enum resolver. Schema
/// compilation only knows same-file enum names; the checker supplies a resolver
/// for qualified names after imports and module visibility are known.
pub fn check_saved_named_member_fields_with(
    members: &[ResourceMember],
    mut is_declared_enum_name: impl FnMut(&str) -> bool,
) -> Vec<SchemaError> {
    let mut errors = Vec::new();
    for member in members {
        check_named_field(member, &mut is_declared_enum_name, &mut errors);
    }
    errors
}

fn check_named_field(
    member: &ResourceMember,
    is_declared_enum_name: &mut impl FnMut(&str) -> bool,
    errors: &mut Vec<SchemaError>,
) {
    match member {
        ResourceMember::Field(field) => {
            // Only an accepted keyed-leaf member exposes a saved value type to
            // check; any other `map[...]` spelling is rejected elsewhere.
            let ty = match MapLeaf::resolve(field) {
                Some(map) if map.is_supported_member(field) => map.value,
                Some(_) => return,
                None if contains_map_type(&field.ty.text) => return,
                None => Type::resolve(&field.ty),
            };
            check_named_field_type(&ty, field, is_declared_enum_name, errors);
        }
        ResourceMember::Group(group) => {
            for nested in &group.members {
                check_named_field(nested, is_declared_enum_name, errors);
            }
        }
    }
}

fn check_named_field_type(
    ty: &Type,
    field: &FieldDecl,
    is_declared_enum_name: &mut impl FnMut(&str) -> bool,
    errors: &mut Vec<SchemaError>,
) {
    match ty {
        Type::Named(name) if !is_declared_enum_name(name) => errors.push(SchemaError {
            kind: SchemaErrorKind::NonEnumNamedField {
                field: field.name.clone(),
                ty: name.clone(),
            },
            code: SCHEMA_NON_ENUM_NAMED_FIELD,
            message: format!(
                "saved field `{}` has type `{name}`, which is not a declared enum; \
                 a saved field stores a scalar or checked enum value",
                field.name
            ),
            span: field.span,
        }),
        Type::Sequence(element) => {
            check_named_field_type(element, field, is_declared_enum_name, errors);
        }
        _ => {}
    }
}

fn check_store_index_args(
    index: &IndexDecl,
    keys: &[KeyParam],
    resource: &ResourceSchema,
    errors: &mut Vec<SchemaError>,
) {
    for arg in &index.args {
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
                    span: index.span,
                });
            }
            None => errors.push(SchemaError {
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
                span: index.span,
            }),
            Some(ty) => {
                if let Some(error) = index_arg_type_key_error(&index.name, arg, &ty, index.span) {
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
fn key_type_error(target: SchemaKeyTarget, ty: &Type, span: SourceSpan) -> Option<SchemaError> {
    let what = target
        .saved_key_name()
        .expect("only saved key targets are checked here");
    let name = target.name().to_string();
    match classify_key_type(ty) {
        KeyTypeVerdict::Ok => None,
        KeyTypeVerdict::Decimal => Some(SchemaError {
            kind: SchemaErrorKind::UnorderableKey {
                target,
                ty: ty.clone(),
            },
            code: SCHEMA_UNORDERABLE_KEY,
            message: format!(
                "saved {what} `{name}` cannot use `decimal`; saved keys use ordered \
                 key types and `decimal` has no key encoding"
            ),
            span,
        }),
        KeyTypeVerdict::NonScalar => Some(SchemaError {
            kind: SchemaErrorKind::NonScalarKey {
                target,
                ty: ty.clone(),
            },
            code: SCHEMA_NONSCALAR_KEY,
            message: format!(
                "saved {what} `{name}` must be an orderable scalar type, but found `{ty}`"
            ),
            span,
        }),
    }
}

/// The error an index argument of type `resolved` earns, sharing the orderable-key
/// verdict ([`classify_key_type`]) with identity keys and key parameters. The one
/// divergence: an index argument may name a field whose declared type is a `Named`
/// (an enum the checker later resolves to its scalar), so a `Named` is accepted
/// here where a written key would reject it. Index arguments keep their own message
/// wording but the same kinds and codes.
fn index_arg_type_key_error(
    index: &str,
    arg: &str,
    resolved: &Type,
    span: SourceSpan,
) -> Option<SchemaError> {
    if let Type::Named(_) = resolved {
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
        KeyTypeVerdict::NonScalar => Some(SchemaError {
            kind: SchemaErrorKind::NonScalarKey {
                target,
                ty: resolved.clone(),
            },
            code: SCHEMA_NONSCALAR_KEY,
            message: format!(
                "index `{index}` argument `{arg}` must be an orderable scalar type, \
                 but found `{resolved}`"
            ),
            span,
        }),
    }
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
