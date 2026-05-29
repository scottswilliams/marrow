//! Compiles a parsed Marrow resource declaration into a typed-tree
//! [`ResourceSchema`].
//!
//! It maps the parsed resource declaration produced by `marrow-syntax` onto the
//! saved/local tree shape: a saved root with identity keys, top-level fields,
//! keyed layers (sequences, keyed trees, groups, and history), and declared
//! indexes. Semantic validation beyond structure is deferred; see the notes on
//! [`compile_resource`].

use std::fmt;

use marrow_syntax::{
    FieldDecl, GroupDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember, SavedRoot, SourceSpan,
    TypeRef,
};

// The canonical scalar type lives in marrow-store; re-export it so resolution
// and downstream crates share one import path for the storable scalars.
pub use marrow_store::value::ScalarType;

/// A type annotation resolved once during schema compilation, so downstream
/// crates match on structure instead of re-parsing the source spelling.
///
/// Resolution is structural and module-blind: it decides everything a single
/// declaration can (a scalar, a `sequence[...]`, an `X::Id` identity, `unknown`),
/// and leaves any other bare or qualified name as [`Type::Named`]. The checker,
/// which knows the project's resource names, promotes a `Named` to a resource
/// reference or flags it unknown; the runtime only ever reads the scalar leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Scalar(ScalarType),
    Sequence(Box<Type>),
    /// A resource identity such as `Book::Id`, carrying the resource name.
    Identity(String),
    /// A bare or qualified name that is not a scalar, sequence, identity, or
    /// `unknown`: a resource reference (the checker confirms it) or a typo.
    Named(String),
    /// The explicit dynamic boundary type `unknown`.
    Unknown,
}

impl Type {
    /// Resolve a [`TypeRef`]'s source spelling to its structure. Total and
    /// module-blind: every spelling maps to exactly one [`Type`], with anything
    /// not decidable from the text alone landing in [`Type::Named`].
    pub fn resolve(ty: &TypeRef) -> Self {
        Self::resolve_text(ty.text.trim())
    }

    fn resolve_text(text: &str) -> Self {
        // `sequence[T]` is built-in element-type sugar; recurse on the element.
        if let Some(element) = sequence_element(text) {
            return Self::Sequence(Box::new(Self::resolve_text(element)));
        }
        if let Some(scalar) = ScalarType::from_scalar_name(text) {
            return Self::Scalar(scalar);
        }
        if text == "unknown" {
            return Self::Unknown;
        }
        // A resource identity such as `Book::Id` names the resource it wraps.
        if let Some(resource) = text.strip_suffix("::Id") {
            return Self::Identity(resource.to_string());
        }
        Self::Named(text.to_string())
    }

    /// The scalar this type denotes, or `None` for a sequence, identity, named,
    /// or unknown type. The runtime decodes a saved leaf by this scalar.
    pub fn scalar(&self) -> Option<ScalarType> {
        match self {
            Self::Scalar(scalar) => Some(*scalar),
            _ => None,
        }
    }

    /// Does this type embed `unknown`? A type embeds `unknown` when it is
    /// `unknown` itself or a `sequence[...]` whose element embeds it. Managed
    /// saved schemas reject `unknown` anywhere inside.
    pub fn embeds_unknown(&self) -> bool {
        match self {
            Self::Unknown => true,
            Self::Sequence(element) => element.embeds_unknown(),
            _ => false,
        }
    }
}

impl fmt::Display for Type {
    /// The canonical source spelling, the inverse of [`Type::resolve`]. Used in
    /// rejection messages that name the offending type.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar(scalar) => f.write_str(scalar.name()),
            Self::Sequence(element) => write!(f, "sequence[{element}]"),
            Self::Identity(resource) => write!(f, "{resource}::Id"),
            Self::Named(name) => f.write_str(name),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

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

impl SavedRootSchema {
    /// Does this saved root qualify for the default `nextId` allocation policy?
    /// Only a resource with exactly one `int` identity key does; composite
    /// identities, non-integer identities, and keyless singletons are
    /// application-provided. This is the one contract both the checker (which
    /// types `nextId(^root)`) and the runtime write planner (which allocates the
    /// next id) gate on, so it lives here on the shape they both key off.
    pub fn single_int_root(&self) -> bool {
        matches!(self.identity_keys.as_slice(), [key] if key.ty == Type::Scalar(ScalarType::Int))
    }

    /// Name the identity shape that disqualifies this root from the default
    /// `nextId` policy, as a noun phrase for the rejection message: a keyless
    /// singleton, a single non-`int` key, or a composite identity. Both the
    /// checker diagnostic and the runtime fault reuse this so their wording
    /// cannot drift apart.
    pub fn next_id_shape(&self) -> String {
        match self.identity_keys.as_slice() {
            [] => "a keyless singleton".into(),
            [key] => format!("a single `{}` key", key.ty),
            keys => format!("a composite identity of {} keys", keys.len()),
        }
    }
}

/// A named, typed key parameter of a saved root or keyed layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDef {
    pub name: String,
    pub ty: Type,
}

/// A scalar field stored directly on a resource or inside an unkeyed group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub docs: Vec<String>,
    pub required: bool,
    pub ty: Type,
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
    pub leaf_type: Option<Type>,
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
/// keyed child layers.
pub const SCHEMA_UNKNOWN_INDEX_ARG: &str = "schema.unknown_index_arg";

/// Two resource elements declare the same stable ID. Stable IDs must be unique.
pub const SCHEMA_DUPLICATE_STABLE_ID: &str = "schema.duplicate_stable_id";

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

/// An index is declared on a resource with no keyed saved root. Declared indexes
/// are members of keyed saved resources; a singleton (keyless) or local
/// (non-saved) resource has no generated identity for an entry to point to.
pub const SCHEMA_INDEX_REQUIRES_KEYED_ROOT: &str = "schema.index_requires_keyed_root";

/// A required field is declared inside an unkeyed group. The write planner does
/// not materialize unkeyed groups (their fields live in `layers`, not `fields`),
/// so it neither validates nor persists them on a whole-resource write. A
/// required field there is a compile error rather than a silently unenforced
/// constraint.
pub const SCHEMA_REQUIRED_IN_UNKEYED_GROUP: &str = "schema.required_in_unkeyed_group";

/// An index argument names a field nested through an unkeyed group. The write
/// planner matches index arguments by flat top-level name, so it would silently
/// never maintain such an entry. Until nested index resolution lands, reject it.
pub const SCHEMA_NESTED_INDEX_ARG: &str = "schema.nested_index_arg";

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
/// keep checking. Maps structure and the single-resource rules the schema alone
/// can decide: `unknown` is rejected in managed saved fields and
/// keys, an identity key may not share a name with a top-level member, index
/// arguments must resolve within the resource, and stable IDs are unique within
/// the resource.
///
/// Deferred: full type validation, one-owner-per-root, and project-wide
/// (cross-resource) stable-ID uniqueness.
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
                // `name: sequence[T]` is sugar for the `name(pos: int): T` keyed
                // leaf, so it joins the layers rather than the scalar fields.
                match Type::resolve(&field.ty) {
                    Type::Sequence(element) => {
                        layers.push(keyed_leaf_from_sequence(field, *element))
                    }
                    ty => fields.push(field_schema(field, ty)),
                }
            }
            ResourceMember::Field(field) => {
                names.check(&field.name, field.span, &mut errors);
                layers.push(keyed_leaf(field, &mut errors));
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
    check_stable_ids(decl, &mut errors);

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
    check_duplicate_key_params(&store.keys, decl_span, errors);
    for key in &store.keys {
        let ty = Type::resolve(&key.ty);
        if ty.embeds_unknown() {
            errors.push(unknown_error("identity key", &key.name, decl_span));
        }
        if is_unorderable_key_type(&ty) {
            errors.push(unorderable_key_error("identity key", &key.name, decl_span));
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
        check_member_keys(member, errors);
        check_required_in_unkeyed_group(member, false, errors);
    }
}

/// Reject a required field reachable only through an unkeyed group. The write
/// planner does not materialize unkeyed groups, so a required field there is
/// never validated or persisted. `under_unkeyed` is true once an enclosing group
/// has no key parameters; a keyed group resets nothing, because a field already
/// under an unkeyed group stays unreachable.
fn check_required_in_unkeyed_group(
    member: &ResourceMember,
    under_unkeyed: bool,
    errors: &mut Vec<SchemaError>,
) {
    match member {
        ResourceMember::Field(field) if field.keys.is_empty() => {
            if under_unkeyed && field.required {
                errors.push(SchemaError {
                    code: SCHEMA_REQUIRED_IN_UNKEYED_GROUP,
                    message: format!(
                        "required field `{}` is inside an unkeyed group, which the \
                         write planner does not maintain",
                        field.name
                    ),
                    span: field.span,
                });
            }
        }
        ResourceMember::Group(group) => {
            let under_unkeyed = under_unkeyed || group.keys.is_empty();
            for nested in &group.members {
                check_required_in_unkeyed_group(nested, under_unkeyed, errors);
            }
        }
        // A keyed leaf carries a value, not a required-field tree to descend.
        ResourceMember::Field(_) | ResourceMember::Index(_) => {}
    }
}

/// Validate a keyed-layer's own key parameters, descending into groups. A keyed
/// layer's key must be a saved key, so it may not embed `unknown` and may not be
/// an unorderable type such as `decimal`. A keyed leaf and a keyed group both
/// carry their key parameters in `keys`; an unkeyed field or group has none.
/// Identity keys are checked separately in [`check_saved_data`].
fn check_member_keys(member: &ResourceMember, errors: &mut Vec<SchemaError>) {
    match member {
        ResourceMember::Field(field) => check_key_params(&field.keys, field.span, errors),
        ResourceMember::Group(group) => {
            check_key_params(&group.keys, group.span, errors);
            for nested in &group.members {
                check_member_keys(nested, errors);
            }
        }
        ResourceMember::Index(_) => {}
    }
}

/// Report each key parameter whose type cannot be a saved key. Key params have
/// no span of their own, so errors point at the keyed layer's `span`.
fn check_key_params(keys: &[KeyParam], span: SourceSpan, errors: &mut Vec<SchemaError>) {
    for key in keys {
        let ty = Type::resolve(&key.ty);
        if ty.embeds_unknown() {
            errors.push(unknown_error("key", &key.name, span));
        }
        if is_unorderable_key_type(&ty) {
            errors.push(unorderable_key_error("key", &key.name, span));
        }
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
            let what = if field.keys.is_empty() {
                "field"
            } else {
                "keyed leaf"
            };
            if Type::resolve(&field.ty).embeds_unknown() {
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
/// fields, layers, and index names share the resource namespace, so an identity
/// key may not reuse any of them.
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
/// reached through unkeyed groups; they do not walk keyed child layers. Each
/// unresolved argument is reported at its index's span, in index then argument
/// order.
///
/// An index also requires a keyed saved root: a singleton (keyless) or local
/// (non-saved) resource has no identity for an entry to point to, which is
/// reported once per index and short-circuits the per-argument checks.
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
        if keys.is_empty() {
            errors.push(SchemaError {
                code: SCHEMA_INDEX_REQUIRES_KEYED_ROOT,
                message: format!(
                    "index `{}` requires a keyed saved root; a singleton or local \
                     resource has no identity for an index entry to point to",
                    index.name
                ),
                span: index.span,
            });
            continue;
        }
        for arg in &index.args {
            match index_arg_type(arg, keys, &decl.members) {
                None => errors.push(SchemaError {
                    code: SCHEMA_UNKNOWN_INDEX_ARG,
                    message: format!(
                        "index `{}` argument `{arg}` does not name an identity \
                         key, a field, or a nested field through unkeyed groups",
                        index.name
                    ),
                    span: index.span,
                }),
                // A dotted argument resolves through an unkeyed group, which the
                // write planner does not maintain.
                Some(_) if arg.contains('.') => errors.push(SchemaError {
                    code: SCHEMA_NESTED_INDEX_ARG,
                    message: format!(
                        "index `{}` argument `{arg}` names a field nested through an \
                         unkeyed group, which the write planner does not maintain",
                        index.name
                    ),
                    span: index.span,
                }),
                Some(ty) if is_unorderable_key_type(&Type::resolve(ty)) => {
                    errors.push(SchemaError {
                        code: SCHEMA_UNORDERABLE_KEY,
                        message: format!(
                            "index `{}` argument `{arg}` is a `decimal`, which has no \
                         key encoding; index arguments use ordered key types",
                            index.name
                        ),
                        span: index.span,
                    })
                }
                Some(_) => {}
            }
        }
        if !index.unique && !ends_with_identity_keys(&index.args, keys) {
            errors.push(SchemaError {
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

/// The type `arg` resolves to in this resource, or `None` if it resolves to
/// nothing. A single segment may name an identity key or a top-level unkeyed
/// scalar field. A dotted path walks unkeyed groups (each non-final segment an
/// unkeyed group, the final segment a scalar unkeyed field); identity keys are
/// single-segment only.
fn index_arg_type<'a>(
    arg: &str,
    keys: &'a [KeyParam],
    members: &'a [ResourceMember],
) -> Option<&'a TypeRef> {
    let segments: Vec<&str> = arg.split('.').collect();
    if segments.len() == 1
        && let Some(key) = keys.iter().find(|key| key.name == segments[0])
    {
        return Some(&key.ty);
    }
    resolve_field_type(&segments, members)
}

/// Resolve a non-empty field path against `members` to its field type. The final
/// segment must be an unkeyed scalar field; every earlier segment must be an
/// unkeyed group whose members resolve the rest. Keyed fields and groups are
/// keyed layers that index arguments do not walk.
fn resolve_field_type<'a>(segments: &[&str], members: &'a [ResourceMember]) -> Option<&'a TypeRef> {
    let (name, rest) = segments.split_first().expect("non-empty field path");
    members.iter().find_map(|member| match member {
        ResourceMember::Field(field)
            if rest.is_empty() && field.name == *name && field.keys.is_empty() =>
        {
            Some(&field.ty)
        }
        ResourceMember::Group(group)
            if !rest.is_empty() && group.keys.is_empty() && group.name == *name =>
        {
            resolve_field_type(rest, &group.members)
        }
        _ => None,
    })
}

/// Report stable IDs that repeat within this resource. Stable IDs must be
/// unique; the later element is the error. Elements are visited in source order,
/// descending into each group before the next sibling, so the first occurrence
/// wins deterministically.
///
/// This covers the within-resource subset only; cross-resource uniqueness is
/// deferred to a later project-wide pass.
fn check_stable_ids(decl: &ResourceDecl, errors: &mut Vec<SchemaError>) {
    let mut seen: Vec<String> = Vec::new();
    for (id, span) in stable_ids(decl) {
        if seen.contains(&id) {
            errors.push(SchemaError {
                code: SCHEMA_DUPLICATE_STABLE_ID,
                message: format!("duplicate stable id `{id}`"),
                span,
            });
        } else {
            seen.push(id);
        }
    }
}

/// Every stable ID declared in a resource, paired with the span of the element
/// that carries it, in declaration order (descending into a group before the
/// next sibling). Drives within-resource uniqueness here and project-wide
/// uniqueness in the checker. Repeats are kept so callers can report them.
pub fn stable_ids(decl: &ResourceDecl) -> Vec<(String, SourceSpan)> {
    let mut ids = Vec::new();
    collect_stable_ids(&decl.members, &mut ids);
    ids
}

fn collect_stable_ids(members: &[ResourceMember], ids: &mut Vec<(String, SourceSpan)>) {
    for member in members {
        let (stable_id, span) = match member {
            ResourceMember::Field(field) => (&field.stable_id, field.span),
            ResourceMember::Group(group) => (&group.stable_id, group.span),
            ResourceMember::Index(index) => (&index.stable_id, index.span),
        };
        if let Some(id) = stable_id {
            ids.push((id.clone(), span));
        }
        if let ResourceMember::Group(group) = member {
            collect_stable_ids(&group.members, ids);
        }
    }
}

/// The element type spelling of a `sequence[T]`, or `None` for a non-sequence
/// type. The one place the `sequence[...]` spelling is parsed; [`Type::resolve`]
/// drives off it. `sequence[T]` is sugar for the 1-based `pos: int` keyed tree.
fn sequence_element(text: &str) -> Option<&str> {
    text.trim()
        .strip_prefix("sequence[")
        .and_then(|rest| rest.strip_suffix(']'))
        .map(str::trim)
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

/// Can this type be a saved key? Saved keys use ordered key types: scalars
/// (except `decimal`, which has no order-preserving key encoding) and generated
/// resource identity types. `decimal` is the one scalar the store cannot encode
/// as a key, so it is rejected wherever a key is expected: identity keys,
/// keyed-layer key parameters, and index arguments.
fn is_unorderable_key_type(ty: &Type) -> bool {
    *ty == Type::Scalar(ScalarType::Decimal)
}

fn unorderable_key_error(what: &str, name: &str, span: SourceSpan) -> SchemaError {
    SchemaError {
        code: SCHEMA_UNORDERABLE_KEY,
        message: format!(
            "saved {what} `{name}` cannot use `decimal`; saved keys use ordered \
             key types and `decimal` has no key encoding"
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
                // A nested `name: sequence[T]` desugars to the same `pos: int`
                // keyed leaf as at the top level.
                match Type::resolve(&field.ty) {
                    Type::Sequence(element) => members.push(LayerMember::Layer(
                        keyed_leaf_from_sequence(field, *element),
                    )),
                    ty => members.push(LayerMember::Field(field_schema(field, ty))),
                }
            }
            ResourceMember::Field(field) => {
                names.check(&field.name, field.span, errors);
                members.push(LayerMember::Layer(keyed_leaf(field, errors)));
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

fn field_schema(field: &FieldDecl, ty: Type) -> FieldSchema {
    FieldSchema {
        name: field.name.clone(),
        docs: field.docs.clone(),
        required: field.required,
        ty,
        stable_id: field.stable_id.clone(),
    }
}

/// A field with key parameters is a keyed leaf layer (a sequence or keyed
/// tree), where the field type is the layer's leaf value type.
fn keyed_leaf(field: &FieldDecl, errors: &mut Vec<SchemaError>) -> LayerSchema {
    check_duplicate_key_params(&field.keys, field.span, errors);
    LayerSchema {
        name: field.name.clone(),
        docs: field.docs.clone(),
        key_params: field.keys.iter().map(key_def).collect(),
        leaf_type: Some(Type::resolve(&field.ty)),
        members: Vec::new(),
        stable_id: field.stable_id.clone(),
    }
}

/// Desugar `name: sequence[T]` into the keyed leaf `name(pos: int): T`. The
/// implicit `pos: int` key matches the canonical sequence spelling, so the
/// resulting layer is identical to the one `name(pos: int): T` produces and
/// append/read/traverse work unchanged.
fn keyed_leaf_from_sequence(field: &FieldDecl, element: Type) -> LayerSchema {
    LayerSchema {
        name: field.name.clone(),
        docs: field.docs.clone(),
        key_params: vec![KeyDef {
            name: "pos".to_string(),
            ty: Type::Scalar(ScalarType::Int),
        }],
        leaf_type: Some(element),
        members: Vec::new(),
        stable_id: field.stable_id.clone(),
    }
}

fn group_layer(group: &GroupDecl, errors: &mut Vec<SchemaError>) -> LayerSchema {
    check_duplicate_key_params(&group.keys, group.span, errors);
    LayerSchema {
        name: group.name.clone(),
        docs: group.docs.clone(),
        key_params: group.keys.iter().map(key_def).collect(),
        leaf_type: None,
        members: layer_members(group, errors),
        stable_id: group.stable_id.clone(),
    }
}

/// Report a keyed layer's key parameters that repeat a name. Key params share a
/// per-layer namespace; two keys of the same name are unaddressable. Key params
/// have no span of their own, so errors point at the layer's `span`.
fn check_duplicate_key_params(keys: &[KeyParam], span: SourceSpan, errors: &mut Vec<SchemaError>) {
    let mut seen: Vec<&str> = Vec::new();
    for key in keys {
        if seen.contains(&key.name.as_str()) {
            errors.push(duplicate_key_error(&key.name, span));
        } else {
            seen.push(&key.name);
        }
    }
}

fn duplicate_key_error(name: &str, span: SourceSpan) -> SchemaError {
    SchemaError {
        code: SCHEMA_DUPLICATE_MEMBER,
        message: format!("duplicate key `{name}`"),
        span,
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
        ty: Type::resolve(&key.ty),
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
