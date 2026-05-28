//! Compiles a parsed Marrow resource declaration into a typed-tree
//! [`ResourceSchema`].
//!
//! This is the first slice of the schema model (roadmap step 5). It maps the
//! resource outline produced by `marrow-syntax` onto the saved/local tree
//! shape: a saved root with identity keys, top-level fields, keyed layers
//! (sequences, keyed trees, groups, and history), and declared indexes.
//!
//! Semantic validation beyond structure is deferred; see the `TODO(step 5+)`
//! notes on [`compile_resource`].

use std::fmt;

use marrow_syntax::{
    FieldDecl, GroupDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember, SavedRoot, SourceSpan,
    TypeRef,
};

/// The compiled tree shape of a resource declaration.
///
/// Members are kept in source order in `Vec`s rather than maps: a resource has
/// few members, lookups are linear, and order matches the declaration and
/// inspect output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub saved_root: Option<SavedRootSchema>,
    pub fields: Vec<FieldSchema>,
    pub layers: Vec<LayerSchema>,
    pub indexes: Vec<IndexSchema>,
}

/// The saved root a resource is attached to, with the identity keys that live
/// in the saved path. Identity keys are not stored fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRootSchema {
    pub root: String,
    pub identity_keys: Vec<KeyDef>,
}

/// A named, typed key parameter of a saved root or keyed layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDef {
    pub name: String,
    pub ty: TypeRef,
}

/// A scalar field stored directly on a resource or inside an unkeyed group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub required: bool,
    pub ty: TypeRef,
    pub stable_id: Option<String>,
}

/// A keyed layer: a sequence or keyed-tree leaf (`tags(pos: int): string`) or a
/// group with nested members (`notes(noteId: string)` / `versions(version)`).
///
/// A keyed leaf sets `leaf_type` and leaves `members` empty. A group leaves
/// `leaf_type` empty and fills `members`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub key_params: Vec<KeyDef>,
    pub leaf_type: Option<TypeRef>,
    pub members: Vec<LayerMember>,
    pub stable_id: Option<String>,
}

/// A member nested inside a group layer: either a scalar field or a deeper
/// keyed layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerMember {
    Field(FieldSchema),
    Layer(LayerSchema),
}

/// A declared lookup index over identity keys and fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub args: Vec<String>,
    pub unique: bool,
    pub stable_id: Option<String>,
}

/// An error found while compiling a resource into a schema.
///
/// `code` is a stable `schema.*` identifier; `message` is human-readable; and
/// `span` points at the offending declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    pub code: &'static str,
    pub message: String,
    pub span: SourceSpan,
}

/// A resource member name collides with another member at the same level.
pub const SCHEMA_DUPLICATE_MEMBER: &str = "schema.duplicate_member";

/// An index appears inside a group. Indexes are direct members of keyed saved
/// resources; nested-layer lookups are modeled as a separate resource.
pub const SCHEMA_INDEX_IN_GROUP: &str = "schema.index_in_group";

/// A managed saved field or key is typed `unknown`. `unknown` is a dynamic
/// boundary value; saved schemas use concrete field and key types. Local-only
/// resources may use `unknown`.
pub const SCHEMA_UNKNOWN_IN_SAVED: &str = "schema.unknown_in_saved";

/// A top-level field or layer shares a name with an identity key. Identity keys
/// live in the saved path, so a stored member of the same name is ambiguous.
pub const SCHEMA_KEY_MEMBER_COLLISION: &str = "schema.key_member_collision";

/// An index argument does not resolve to an identity key, a top-level field, or
/// a nested field reached through unkeyed groups. Index arguments do not walk
/// keyed child layers (resources-and-storage.md:197-199).
pub const SCHEMA_UNKNOWN_INDEX_ARG: &str = "schema.unknown_index_arg";

/// Two resource elements declare the same stable ID. Stable IDs must be unique
/// (resources-and-storage.md:159-161).
pub const SCHEMA_DUPLICATE_STABLE_ID: &str = "schema.duplicate_stable_id";

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

/// Compile a parsed resource declaration into a [`ResourceSchema`].
///
/// Always returns a best-effort schema together with any errors, so callers can
/// keep checking. This slice maps structure and the single-resource rules the
/// schema alone can decide: `unknown` is rejected in managed saved fields and
/// keys, an identity key may not share a name with a top-level member, index
/// arguments must resolve within the resource, and stable IDs are unique within
/// the resource.
///
// TODO(step 5+): full type validation, one-owner-per-root, and project-wide
// (cross-resource) stable-ID uniqueness.
pub fn compile_resource(decl: &ResourceDecl) -> (ResourceSchema, Vec<SchemaError>) {
    let mut errors = Vec::new();

    let saved_root = decl.store.as_ref().map(|store| SavedRootSchema {
        root: store.root.clone(),
        identity_keys: store.keys.iter().map(key_def).collect(),
    });

    let mut fields = Vec::new();
    let mut layers = Vec::new();
    let mut indexes = Vec::new();
    let mut names = Namespace::default();

    for member in &decl.members {
        match member {
            ResourceMember::Field(field) if field.keys.is_empty() => {
                names.check(&field.name, field.span, &mut errors);
                fields.push(field_schema(field));
            }
            ResourceMember::Field(field) => {
                names.check(&field.name, field.span, &mut errors);
                layers.push(keyed_leaf(field));
            }
            ResourceMember::Group(group) => {
                names.check(&group.name, group.span, &mut errors);
                layers.push(group_layer(group, &mut errors));
            }
            ResourceMember::Index(index) => {
                names.check(&index.name, index.span, &mut errors);
                indexes.push(index_schema(index));
            }
        }
    }

    let schema = ResourceSchema {
        name: decl.name.clone(),
        docs: decl.docs.clone(),
        saved_root,
        fields,
        layers,
        indexes,
    };

    // Saved-data rules apply only to managed saved resources. They are reported
    // over the declaration, which carries the spans the built schema does not.
    if let Some(store) = &decl.store {
        check_saved_data(store, &decl.members, decl.span, &mut errors);
    }

    check_index_args(decl, &mut errors);
    check_stable_ids(&decl.members, &mut errors);

    (schema, errors)
}

/// Report the saved-data rules for a managed saved resource: reject `unknown`
/// in identity keys, fields, and keyed leaves (recursively), and reject an
/// identity key that shares a name with a top-level member.
///
/// Errors are collected in source order: identity keys, then members. Identity
/// keys have no span of their own, so their errors point at the declaration.
fn check_saved_data(
    store: &SavedRoot,
    members: &[ResourceMember],
    decl_span: SourceSpan,
    errors: &mut Vec<SchemaError>,
) {
    for key in &store.keys {
        if embeds_unknown(&key.ty) {
            errors.push(unknown_error("identity key", &key.name, decl_span));
        }
        if let Some(span) = top_level_member_span(members, &key.name) {
            errors.push(SchemaError {
                code: SCHEMA_KEY_MEMBER_COLLISION,
                message: format!(
                    "identity key `{}` collides with a top-level member of the \
                     same name; identity keys live in the saved path, not stored \
                     members",
                    key.name
                ),
                span,
            });
        }
    }

    for member in members {
        check_member_unknown(member, errors);
    }
}

/// Reject `unknown` on the value type of a field or keyed leaf, descending into
/// groups. Key types use ordered scalars or identity types and never `unknown`
/// (see `docs/language/types.md`), so layer key parameters are not checked here.
fn check_member_unknown(member: &ResourceMember, errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => {
            // A keyed leaf carries its value type the same way a plain field
            // does; both reject `unknown`.
            let what = if field.keys.is_empty() {
                "field"
            } else {
                "keyed leaf"
            };
            if embeds_unknown(&field.ty) {
                errors.push(unknown_error(what, &field.name, field.span));
            }
        }
        ResourceMember::Group(group) => {
            for nested in &group.members {
                check_member_unknown(nested, errors);
            }
        }
        ResourceMember::Index(_) => {}
    }
}

/// The span of a top-level member named `name`, if one exists. Identity keys,
/// fields, layers, and index names share the resource namespace
/// (resources-and-storage.md:240-242), so an identity key may not reuse any of
/// them.
fn top_level_member_span(members: &[ResourceMember], name: &str) -> Option<SourceSpan> {
    members.iter().find_map(|member| match member {
        ResourceMember::Field(field) if field.name == name => Some(field.span),
        ResourceMember::Group(group) if group.name == name => Some(group.span),
        ResourceMember::Index(index) if index.name == name => Some(index.span),
        _ => None,
    })
}

/// Resolve each top-level index argument against the resource. Arguments may
/// name an identity key, a top-level unkeyed field, or a nested scalar field
/// reached through unkeyed groups; they do not walk keyed child layers
/// (resources-and-storage.md:197-199). Each unresolved argument is reported at
/// its index's span, in index then argument order.
fn check_index_args(decl: &ResourceDecl, errors: &mut Vec<SchemaError>) {
    let keys = decl
        .store
        .as_ref()
        .map(|store| &store.keys[..])
        .unwrap_or(&[]);
    for member in &decl.members {
        let ResourceMember::Index(index) = member else {
            continue;
        };
        for arg in &index.args {
            if !index_arg_resolves(arg, keys, &decl.members) {
                errors.push(SchemaError {
                    code: SCHEMA_UNKNOWN_INDEX_ARG,
                    message: format!(
                        "index `{}` argument `{arg}` does not name an identity \
                         key, a field, or a nested field through unkeyed groups",
                        index.name
                    ),
                    span: index.span,
                });
            }
        }
    }
}

/// Does `arg` resolve to an indexable value in this resource? A single segment
/// may name an identity key or a top-level unkeyed scalar field. A dotted path
/// walks unkeyed groups (each non-final segment an unkeyed group, the final
/// segment a scalar unkeyed field); identity keys are single-segment only.
fn index_arg_resolves(arg: &str, keys: &[KeyParam], members: &[ResourceMember]) -> bool {
    let segments: Vec<&str> = arg.split('.').collect();
    if segments.len() == 1 && keys.iter().any(|key| key.name == segments[0]) {
        return true;
    }
    resolves_through_members(&segments, members)
}

/// Resolve a non-empty field path against `members`. The final segment must be
/// an unkeyed scalar field; every earlier segment must be an unkeyed group
/// whose members resolve the rest. Keyed fields and groups are keyed layers
/// that index arguments do not walk.
fn resolves_through_members(segments: &[&str], members: &[ResourceMember]) -> bool {
    let (name, rest) = segments.split_first().expect("non-empty field path");
    members.iter().any(|member| match member {
        ResourceMember::Field(field) if rest.is_empty() => {
            field.name == *name && field.keys.is_empty()
        }
        ResourceMember::Group(group) if !rest.is_empty() && group.keys.is_empty() => {
            group.name == *name && resolves_through_members(rest, &group.members)
        }
        _ => false,
    })
}

/// Report stable IDs that repeat within this resource. Stable IDs must be
/// unique (resources-and-storage.md:159-161); the later element is the error.
/// Elements are visited in source order, descending into each group before the
/// next sibling, so the first occurrence wins deterministically.
///
/// This covers the within-resource subset only; cross-resource uniqueness is
/// deferred to a later project-wide pass.
fn check_stable_ids(members: &[ResourceMember], errors: &mut Vec<SchemaError>) {
    let mut seen: Vec<&str> = Vec::new();
    collect_stable_ids(members, &mut seen, errors);
}

fn collect_stable_ids<'a>(
    members: &'a [ResourceMember],
    seen: &mut Vec<&'a str>,
    errors: &mut Vec<SchemaError>,
) {
    for member in members {
        let (stable_id, span) = match member {
            ResourceMember::Field(field) => (&field.stable_id, field.span),
            ResourceMember::Group(group) => (&group.stable_id, group.span),
            ResourceMember::Index(index) => (&index.stable_id, index.span),
        };
        if let Some(id) = stable_id {
            if seen.contains(&id.as_str()) {
                errors.push(SchemaError {
                    code: SCHEMA_DUPLICATE_STABLE_ID,
                    message: format!("duplicate stable id `{id}`"),
                    span,
                });
            } else {
                seen.push(id);
            }
        }
        if let ResourceMember::Group(group) = member {
            collect_stable_ids(&group.members, seen, errors);
        }
    }
}

/// Does this saved type embed `unknown`? A type embeds `unknown` when it is
/// `unknown` itself or a `sequence[...]` whose element type embeds it. Managed
/// saved schemas reject `unknown` anywhere inside (docs/language/types.md).
fn embeds_unknown(ty: &TypeRef) -> bool {
    let mut text = ty.text.trim();
    while let Some(inner) = text
        .strip_prefix("sequence[")
        .and_then(|rest| rest.strip_suffix(']'))
    {
        text = inner.trim();
    }
    text == "unknown"
}

fn unknown_error(what: &str, name: &str, span: SourceSpan) -> SchemaError {
    SchemaError {
        code: SCHEMA_UNKNOWN_IN_SAVED,
        message: format!(
            "saved {what} `{name}` cannot use `unknown`; managed saved \
             schemas use concrete types"
        ),
        span,
    }
}

/// Compile the members nested inside a group. Fields and groups recurse; an
/// index here is an error, since indexes are direct members of the resource.
fn layer_members(group: &GroupDecl, errors: &mut Vec<SchemaError>) -> Vec<LayerMember> {
    let mut members = Vec::new();
    let mut names = Namespace::default();

    for member in &group.members {
        match member {
            ResourceMember::Field(field) if field.keys.is_empty() => {
                names.check(&field.name, field.span, errors);
                members.push(LayerMember::Field(field_schema(field)));
            }
            ResourceMember::Field(field) => {
                names.check(&field.name, field.span, errors);
                members.push(LayerMember::Layer(keyed_leaf(field)));
            }
            ResourceMember::Group(nested) => {
                names.check(&nested.name, nested.span, errors);
                members.push(LayerMember::Layer(group_layer(nested, errors)));
            }
            ResourceMember::Index(index) => {
                errors.push(SchemaError {
                    code: SCHEMA_INDEX_IN_GROUP,
                    message: format!(
                        "index `{}` cannot be declared inside group `{}`; \
                         declare indexes as direct resource members",
                        index.name, group.name
                    ),
                    span: index.span,
                });
            }
        }
    }

    members
}

fn field_schema(field: &FieldDecl) -> FieldSchema {
    FieldSchema {
        name: field.name.clone(),
        docs: field.docs.clone(),
        required: field.required,
        ty: field.ty.clone(),
        stable_id: field.stable_id.clone(),
    }
}

/// A field with key parameters is a keyed leaf layer (a sequence or keyed
/// tree), where the field type is the layer's leaf value type.
fn keyed_leaf(field: &FieldDecl) -> LayerSchema {
    LayerSchema {
        name: field.name.clone(),
        docs: field.docs.clone(),
        key_params: field.keys.iter().map(key_def).collect(),
        leaf_type: Some(field.ty.clone()),
        members: Vec::new(),
        stable_id: field.stable_id.clone(),
    }
}

fn group_layer(group: &GroupDecl, errors: &mut Vec<SchemaError>) -> LayerSchema {
    LayerSchema {
        name: group.name.clone(),
        docs: group.docs.clone(),
        key_params: group.keys.iter().map(key_def).collect(),
        leaf_type: None,
        members: layer_members(group, errors),
        stable_id: group.stable_id.clone(),
    }
}

fn index_schema(index: &IndexDecl) -> IndexSchema {
    IndexSchema {
        name: index.name.clone(),
        docs: index.docs.clone(),
        args: index.args.clone(),
        unique: index.unique,
        stable_id: index.stable_id.clone(),
    }
}

fn key_def(key: &KeyParam) -> KeyDef {
    KeyDef {
        name: key.name.clone(),
        ty: key.ty.clone(),
    }
}

/// Tracks member names seen at one nesting level so duplicates can be reported.
/// Fields, layers, and indexes share one flat namespace per level.
#[derive(Default)]
struct Namespace {
    seen: Vec<String>,
}

impl Namespace {
    fn check(&mut self, name: &str, span: SourceSpan, errors: &mut Vec<SchemaError>) {
        if self.seen.iter().any(|existing| existing == name) {
            errors.push(SchemaError {
                code: SCHEMA_DUPLICATE_MEMBER,
                message: format!("duplicate resource member `{name}`"),
                span,
            });
        } else {
            self.seen.push(name.to_string());
        }
    }
}
