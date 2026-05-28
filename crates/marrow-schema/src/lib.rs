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
    FieldDecl, GroupDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember, SourceSpan, TypeRef,
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
/// keep checking. This slice maps structure only and defers semantic checks.
///
// TODO(step 5+): type validation, rejecting `unknown` in managed fields,
// index-argument resolution, identity-key-vs-field collisions, one-owner-per-
// root, and stable-ID uniqueness.
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
    (schema, errors)
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
