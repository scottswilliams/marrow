//! Program and schema lookups, and saved-path classification.

use crate::*;

/// Whether the chain of layer names (outermost to innermost) is fully declared on
/// the resource at `root`: the first is a direct layer of the resource, each
/// deeper one a nested layer of the one before it. Used to reject a delete of an
/// undeclared layer entry before touching the store.
pub(crate) fn resource_layer_chain_exists(
    program: &CheckedProgram,
    root: &str,
    layers: &[&str],
) -> bool {
    find_resource(program, root).is_some_and(|resource| resource.descend_layers(layers).is_some())
}

/// The resource schema attached to a saved root, by root name. Saved roots are
/// project-wide (a `^books` write addresses the one `books` resource from any
/// module), so this routes through the resolver's project-wide root lookup; the
/// checker's `find_resource_schema` shares the same helper.
pub(crate) fn find_resource<'p>(
    program: &'p CheckedProgram,
    root: &str,
) -> Option<&'p ResourceSchema> {
    marrow_check::resolve::resolve_resource_by_root(program, root)
}

/// The enum named `name` owned by exactly `module`, if any. Mirrors the checker's
/// `enum_schema_in`: a `match`'s recorded enum identity is by owning module, so
/// dispatch reads that exact enum's ordinals and same-named enums never alias.
pub(crate) fn enum_in<'p>(
    program: &'p CheckedProgram,
    module: &str,
    name: &str,
) -> Option<&'p EnumSchema> {
    program
        .modules
        .iter()
        .find(|m| m.name == module)?
        .enums
        .iter()
        .find(|enum_schema| enum_schema.name == name)
}

/// Resolve a bare enum `name` referenced from `referencing_module`: that module's
/// own enum first, then the first project-wide match. Mirrors the checker's
/// `resolve_enum` so a `Enum::member` ordinal is read from the same enum the
/// checker validated against.
pub(crate) fn resolve_enum<'p>(
    program: &'p CheckedProgram,
    referencing_module: &str,
    name: &str,
) -> Option<&'p EnumSchema> {
    enum_in(program, referencing_module, name).or_else(|| {
        program
            .modules
            .iter()
            .flat_map(|module| &module.enums)
            .find(|enum_schema| enum_schema.name == name)
    })
}

/// The number of declared identity keys for the resource at saved root `name`,
/// or `None` when `name` is not a managed saved root. A keyless singleton has
/// arity 0; a keyed root such as `^books` has a positive arity, so it cannot be
/// read or addressed without an identity.
pub(crate) fn root_identity_arity(program: &CheckedProgram, name: &str) -> Option<usize> {
    find_resource(program, name)
        .and_then(|resource| resource.saved_root.as_ref())
        .map(|root| root.identity_keys.len())
}

/// The declared identity key parameters of the resource at saved root `name`, or
/// an empty slice when the root is unresolved or keyless — the key-type guard
/// reads these to reject a wrong-typed record key.
pub(crate) fn root_identity_keys<'p>(program: &'p CheckedProgram, name: &str) -> &'p [KeyDef] {
    find_resource(program, name)
        .and_then(|resource| resource.saved_root.as_ref())
        .map_or(&[], |root| root.identity_keys.as_slice())
}

/// The declared key parameters of a keyed layer named by its chain (outermost
/// first), or an empty slice when the chain does not resolve — the key-type guard
/// reads these to reject a wrong-typed index key.
pub(crate) fn layer_key_params<'p>(
    program: &'p CheckedProgram,
    root: &str,
    chain: &[&str],
) -> &'p [KeyDef] {
    find_resource(program, root)
        .and_then(|resource| resource.descend_layers(chain))
        .map_or(&[], |node| node.key_params.as_slice())
}

/// The resource schema declared with `name`, for an identity constructor
/// `Name::Id(...)`. Keyed on the resource name (not its saved root), since the
/// constructor names the resource; routed through the resolver's project-wide
/// name lookup so the runtime keeps no name resolution of its own.
pub(crate) fn find_resource_by_name<'p>(
    program: &'p CheckedProgram,
    name: &str,
) -> Option<&'p ResourceSchema> {
    marrow_check::resolve::resolve_resource_by_name_any(program, name)
}

/// Build a resource identity value from a `Resource::Id(...)` constructor: its
/// keys lowered in declared identity-key order. Positional args lower in order;
/// named args (composite keys) match by key name. A singleton (keyless) resource
/// has no identity type, and an arity or name mismatch is a type error.
pub(crate) fn eval_identity_constructor(
    resource: &ResourceSchema,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let root = resource
        .saved_root
        .as_ref()
        .filter(|saved| !saved.identity_keys.is_empty());
    let Some(root) = root else {
        return Err(unsupported(
            "an identity for a resource with no identity keys",
            span,
        ));
    };
    if args.iter().any(|arg| arg.mode.is_some()) {
        return Err(type_error(
            "an identity key cannot be an out argument",
            span,
        ));
    }
    if args.len() != root.identity_keys.len() {
        return Err(type_error(
            &format!(
                "`{}::Id` takes {} key(s), got {}",
                resource.name,
                root.identity_keys.len(),
                args.len()
            ),
            span,
        ));
    }
    // Mixed positional and named arguments are ambiguous; require one shape.
    let named = args.iter().filter(|arg| arg.name.is_some()).count();
    if named != 0 && named != args.len() {
        return Err(type_error(
            "an identity takes either positional or named keys, not both",
            span,
        ));
    }
    let mut keys = Vec::with_capacity(root.identity_keys.len());
    if named == 0 {
        // Positional: each argument lowers to the key at the same position, then is
        // guarded against that position's declared key type so a wrong-scalar key —
        // a dynamic value the checker could not see — faults instead of settling
        // into the typed keyspace, the same guard a record lookup `^root(key)` runs.
        for (key_def, arg) in root.identity_keys.iter().zip(args) {
            let key = value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| type_error("a key of this type", span))?;
            guard_key_type(key_def, &key, span)?;
            keys.push(key);
        }
    } else {
        // Named (composite): for each declared key, find the matching argument,
        // so keys land in declared order regardless of argument order; each is
        // guarded against its declared key type the same way.
        for key_def in &root.identity_keys {
            let arg = args
                .iter()
                .find(|arg| arg.name.as_deref() == Some(key_def.name.as_str()))
                .ok_or_else(|| {
                    type_error(&format!("identity key `{}` is missing", key_def.name), span)
                })?;
            let key = value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| type_error("a key of this type", span))?;
            guard_key_type(key_def, &key, span)?;
            keys.push(key);
        }
    }
    Ok(identity_value(keys))
}

/// Whether `name` names a resource type (for an uninitialized `var book: Book`
/// to start as an empty resource value).
pub(crate) fn is_resource_type(program: &CheckedProgram, from_module: &str, name: &str) -> bool {
    if !name.contains("::") {
        return program
            .modules
            .iter()
            .flat_map(|module| &module.resources)
            .any(|resource| resource.name == name);
    }
    let segments: Vec<String> = name.split("::").map(str::to_string).collect();
    matches!(
        resolve(program, from_module, &segments, ResolvableKind::Resource),
        Resolution::Found(Def {
            item: DefItem::Resource(_),
            ..
        })
    )
}

/// Whether an expression denotes a saved path (rooted at a `^root`), as opposed
/// to a local value. Field access and key lookups on a saved path are saved
/// reads; on a local resource value they read its materialized fields.
pub(crate) fn is_saved_path(expr: &Expression) -> bool {
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Call { callee, .. } => is_saved_path(callee),
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            is_saved_path(base)
        }
        _ => false,
    }
}

/// Whether a field-read/write base reaches its field through a group layer (so the
/// nested-field reader/writer handles it): a keyed GROUP entry `^root(id…).layer(key…)`
/// (a layer call whose callee is a `.layer` access), or an unkeyed group hop
/// `^root(id…).name` (a `.field` off a saved path). A plain record base
/// `^root(id…)` or singleton `^root` is a top-level field, not a group base.
pub(crate) fn is_group_base(base: &Expression) -> bool {
    match base {
        Expression::Call { callee, .. } => matches!(callee.as_ref(), Expression::Field { .. }),
        Expression::Field { base, .. } => is_saved_path(base),
        _ => false,
    }
}

/// How a saved leaf field stores and reads: a plain scalar (its own type, or an
/// enum's ordinal `int`), or a typed reference to another resource's identity. An
/// identity leaf names the referenced resource and carries its identity arity, so
/// the runtime decodes the stored bytes back with `decode_identity` and encodes a
/// write with `encode_identity`, sharing the flat key encoding unique-index entries
/// already use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeafKind {
    Scalar(ScalarType),
    Identity { resource: String, arity: usize },
}

/// Resolve a declared member [`Type`] to its stored [`LeafKind`], given the program
/// so an identity type can resolve the referenced resource's identity arity. A
/// scalar or enum field is a scalar leaf; an `Author::Id` field is an identity leaf
/// referencing `Author`. Any other type (a sequence, a bare resource, an unresolved
/// identity reference) has no flat leaf form, so this is `None`.
pub(crate) fn leaf_kind(program: &CheckedProgram, ty: &Type) -> Option<LeafKind> {
    match ty {
        Type::Identity(resource) => {
            let referenced = find_resource_by_name(program, resource)?;
            let arity = referenced.saved_root.as_ref()?.identity_keys.len();
            Some(LeafKind::Identity {
                resource: resource.clone(),
                arity,
            })
        }
        other => other.stored_scalar().map(LeafKind::Scalar),
    }
}

/// The stored leaf kind of a saved root's top-level field — a scalar (its own type,
/// or `int` for an enum's ordinal) or a typed reference to another resource.
pub(crate) fn resource_field_leaf(
    program: &CheckedProgram,
    root: &str,
    field: &str,
) -> Option<LeafKind> {
    let ty = find_resource(program, root)?.field_type(&[field])?.clone();
    leaf_kind(program, &ty)
}

/// The stored leaf kind of a keyed-leaf layer on a saved root (e.g. the `string` of
/// `tags(pos: int): string`, or an identity-typed keyed leaf).
pub(crate) fn resource_layer_leaf(
    program: &CheckedProgram,
    root: &str,
    layer: &str,
) -> Option<LeafKind> {
    let ty = find_resource(program, root)?.leaf_type(&[layer])?.clone();
    leaf_kind(program, &ty)
}

/// The stored leaf kind of a keyed-leaf layer named by its full chain from the
/// resource, e.g. `rows.fields` for `^table(id).rows(row).fields(col)`.
pub(crate) fn resource_layer_leaf_chain(
    program: &CheckedProgram,
    root: &str,
    layers: &[&str],
) -> Option<LeafKind> {
    let ty = find_resource(program, root)?.leaf_type(layers)?.clone();
    leaf_kind(program, &ty)
}

/// The stored leaf kind of a scalar member field inside a saved root's GROUP layer,
/// at any nesting depth (e.g. the `string` of
/// `versions(version: int).comments(pos: int).text`). `layers` names the group
/// layers from outermost to innermost.
pub(crate) fn resource_nested_member_leaf(
    program: &CheckedProgram,
    root: &str,
    layers: &[&str],
    field: &str,
) -> Option<LeafKind> {
    let resource = find_resource(program, root)?;
    let mut chain = layers.to_vec();
    chain.push(field);
    let ty = resource.field_type(&chain)?.clone();
    leaf_kind(program, &ty)
}

/// Whether a nested group member is declared (scalar or identity), for a delete
/// that only needs the member's existence, not its kind.
pub(crate) fn resource_nested_member_exists(
    program: &CheckedProgram,
    root: &str,
    layers: &[&str],
    field: &str,
) -> bool {
    resource_nested_member_leaf(program, root, layers, field).is_some()
}

/// How a stored saved path relates to the project schema, for data-integrity
/// inspection. A path is classified by composing the same field/layer/index
/// resolution the runtime uses for reads, so the inspector and the runtime agree
/// on what each path means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedPathClass {
    /// The path is a declared scalar leaf of the given type; its stored bytes
    /// should decode as a canonical form of that type.
    Scalar(ScalarType),
    /// The path is a declared typed-reference leaf (`authorId: Author::Id`); its
    /// stored bytes are the referenced identity's canonical encoding, which decodes
    /// back with `decode_identity` against `arity` keys, not as a scalar.
    Identity { resource: String, arity: usize },
    /// The path is a generated index entry (`^root.index(keys…)`). Its value is a
    /// presence marker or a stored identity, raw-only by design — not a typed
    /// scalar, and a legal, expected store-level value.
    IndexMarker,
    /// The path's member chain resolves, but a record or index key has a scalar
    /// kind the schema does not declare for that key position — a key written
    /// (or restored) at the wrong type. The keyspace is corrupt even though the
    /// member is real, so this is distinct from an orphan.
    KeyTypeMismatch {
        expected: ScalarType,
        found: ScalarType,
    },
    /// The path is under no declared root, or names a member the schema does not
    /// declare — stale or foreign data the schema cannot account for.
    Orphan,
}

/// Classify a decoded saved path against the program's schemas.
/// Named segments arrive uniformly as
/// [`PathSegment::Field`] because the store cannot tell a field, layer, or index
/// name apart from bytes — this resolves the ambiguity with the schema:
///
/// - `^root(identity…).field` is a top-level field;
/// - `^root(identity…).layer(keys…)` is a keyed-leaf layer entry;
/// - `^root(identity…).layer(keys…).field` (any depth) is a nested group field;
/// - `^root.index(keys…)` is a generated index entry;
/// - anything under an unknown root or naming an undeclared member is an orphan.
///
/// It composes [`resource_field_leaf`], [`resource_layer_leaf`], and
/// [`resource_nested_member_leaf`] so the schema-walk stays in one place.
pub fn classify_saved_path(program: &CheckedProgram, segments: &[PathSegment]) -> SavedPathClass {
    let Some((PathSegment::Root(root), rest)) = segments.split_first() else {
        return SavedPathClass::Orphan;
    };
    let Some(arity) = root_identity_arity(program, root) else {
        return SavedPathClass::Orphan;
    };

    // The identity record keys directly under the root.
    let identity_keys = rest
        .iter()
        .take_while(|segment| matches!(segment, PathSegment::RecordKey(_)))
        .count();
    let after_identity = &rest[identity_keys..];

    // An index lives directly under the root, before any identity keys:
    // `^root.index(keys…)`. A named segment with no preceding record key is an
    // index name (or an orphan if undeclared).
    if identity_keys == 0
        && let Some((PathSegment::Field(name), keys)) = after_identity.split_first()
        && keys
            .iter()
            .all(|segment| matches!(segment, PathSegment::IndexKey(_)))
    {
        let Some(resource) = find_resource(program, root) else {
            return SavedPathClass::Orphan;
        };
        if resource.indexes.iter().any(|index| index.name == *name) {
            return SavedPathClass::IndexMarker;
        }
        if arity == 0 {
            return classify_member(program, root, after_identity);
        }
        return SavedPathClass::Orphan;
    }

    // A record value path carries the full identity, then a member chain.
    if identity_keys != arity {
        return SavedPathClass::Orphan;
    }
    // The identity is the right length; each record key must also be the declared
    // scalar kind, or the keyspace is corrupt at the wrong type.
    if let Some(resource) = find_resource(program, root)
        && let Some(saved) = resource.saved_root.as_ref()
        && let Some(mismatch) = key_type_mismatch(
            &saved.identity_keys,
            rest[..identity_keys].iter().filter_map(record_key),
        )
    {
        return mismatch;
    }
    classify_member(program, root, after_identity)
}

/// The record key carried by a segment, or `None` for any other segment kind.
pub(crate) fn record_key(segment: &PathSegment) -> Option<&SavedKey> {
    match segment {
        PathSegment::RecordKey(key) => Some(key),
        _ => None,
    }
}

/// The first inner-key type mismatch of a stored identity-leaf reference, given
/// the referenced resource and the keys decoded from the leaf — for data-integrity
/// inspection of a typed-reference leaf (`authorId: Author::Id`). A wrong-scalar
/// key encodes by arity alone, so the arity check passes it; this catches the
/// reference that points at a record the referenced keyspace could never hold. The
/// referenced resource's declared identity keys are the authority; an unresolved
/// or keyless resource declares none, so the check finds nothing.
pub fn identity_leaf_key_mismatch(
    program: &CheckedProgram,
    resource: &str,
    keys: &[SavedKey],
) -> Option<(ScalarType, ScalarType)> {
    let declared = find_resource_by_name(program, resource)
        .and_then(|resource| resource.saved_root.as_ref())?
        .identity_keys
        .as_slice();
    match key_type_mismatch(declared, keys.iter()) {
        Some(SavedPathClass::KeyTypeMismatch { expected, found }) => Some((expected, found)),
        _ => None,
    }
}

/// The first key-type mismatch between a layer's declared key parameters and the
/// keys addressing it, or `None` when every key matches its declared scalar kind.
/// An arity mismatch is the caller's to flag, so a shorter key run simply ends
/// the comparison; a key under a non-scalar (defer) declaration is left alone.
pub(crate) fn key_type_mismatch<'a>(
    declared: &[KeyDef],
    found: impl Iterator<Item = &'a SavedKey>,
) -> Option<SavedPathClass> {
    declared
        .iter()
        .zip(found)
        .find_map(|(def, key)| match def.ty.scalar() {
            Some(expected) if expected != key.scalar_type() => {
                Some(SavedPathClass::KeyTypeMismatch {
                    expected,
                    found: key.scalar_type(),
                })
            }
            _ => None,
        })
}

/// Classify the member chain of a record path (everything after the identity
/// keys): a sequence of named segments, each optionally followed by its index
/// keys. The named segments are the field/layer names; their interleaved index
/// keys position a layer entry. The chain resolves to a scalar leaf — a
/// top-level field, a keyed-leaf layer entry, or a nested group field — or an
/// orphan when the schema does not declare it.
pub(crate) fn classify_member(
    program: &CheckedProgram,
    root: &str,
    members: &[PathSegment],
) -> SavedPathClass {
    // Split the chain into its named segments, each carrying the index keys that
    // immediately follow it, and reject any stray structure (a record key in
    // member position, etc.). A bare identity path has no member and carries no
    // scalar leaf, so a value stored there is an orphan.
    let mut named: Vec<(&str, Vec<&SavedKey>)> = Vec::new();
    for segment in members {
        match segment {
            PathSegment::Field(name) => named.push((name.as_str(), Vec::new())),
            // Index keys position the preceding layer, so they belong to the most
            // recent named segment; one before any name is malformed.
            PathSegment::IndexKey(key) => match named.last_mut() {
                Some((_, keys)) => keys.push(key),
                None => return SavedPathClass::Orphan,
            },
            // A record key or root in member position is malformed for a record.
            PathSegment::RecordKey(_)
            | PathSegment::Root(_)
            | PathSegment::ChildLayer(_)
            | PathSegment::Index(_) => return SavedPathClass::Orphan,
        }
    }
    let names: Vec<&str> = named.iter().map(|(name, _)| *name).collect();
    let Some((&last, layers)) = names.split_last() else {
        return SavedPathClass::Orphan;
    };

    // Every layer name that carries index keys must address its layer with keys of
    // the declared scalar kinds; a wrong-typed index key is a corrupt keyspace, not
    // an orphan. The terminal name is a leaf, whose own keys (a keyed-leaf entry)
    // are the keys of the layer it names, so it is checked alongside the rest.
    if let Some(resource) = find_resource(program, root) {
        for (depth, (name, keys)) in named.iter().enumerate() {
            if keys.is_empty() {
                continue;
            }
            let chain: Vec<&str> = names[..depth]
                .iter()
                .copied()
                .chain(std::iter::once(*name))
                .collect();
            let Some(node) = resource.descend_layers(&chain) else {
                continue;
            };
            if let Some(mismatch) = key_type_mismatch(&node.key_params, keys.iter().copied()) {
                return mismatch;
            }
        }
    }

    // No leading layers: either a top-level field `^root(id).field` or a single
    // keyed-leaf layer entry `^root(id).layer(keys…)`.
    if layers.is_empty() {
        if let Some(leaf) = resource_field_leaf(program, root, last) {
            return leaf_class(leaf);
        }
        if let Some(leaf) = resource_layer_leaf(program, root, last) {
            return leaf_class(leaf);
        }
        return SavedPathClass::Orphan;
    }

    // Leading layers: a nested group field `^root(id).layer(keys…)….field`, or a
    // deeper keyed-leaf layer whose own name is the tail.
    if let Some(leaf) = resource_nested_member_leaf(program, root, layers, last) {
        return leaf_class(leaf);
    }
    SavedPathClass::Orphan
}

/// Map a resolved [`LeafKind`] to the integrity classification of its stored bytes:
/// a scalar leaf decodes as its scalar type; an identity leaf decodes as the
/// referenced identity's canonical encoding.
fn leaf_class(leaf: LeafKind) -> SavedPathClass {
    match leaf {
        LeafKind::Scalar(ty) => SavedPathClass::Scalar(ty),
        LeafKind::Identity { resource, arity } => SavedPathClass::Identity { resource, arity },
    }
}
