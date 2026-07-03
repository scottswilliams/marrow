use marrow_schema::{IndexSchema, KeyDef, Node, StoreSchema};

use crate::catalog::{
    enum_member_path, enum_path, resource_member_path, resource_path, store_index_path, store_path,
};
use crate::{CheckedConst, CheckedProgram};

/// A stable `sha256:<hex>` digest of the program's durable shape, derived from the canonical
/// schema structure — never from formatter text. It hashes a record per durable entity: every
/// `resource` member, `store`, store index, `enum` member, and module `const`, each by its
/// module-qualified path and its shape by source identity (see [`shape_records`]). This is what the
/// store stamps at commit and the activation-window fence enforces, so it binds exactly the facts a
/// stored snapshot must satisfy.
///
/// Hashing structure rather than rendered source means the digest moves for exactly the edits that
/// change what a stored snapshot must satisfy — a member retype, a re-key, an index reshape, an
/// added, removed, renamed, or reordered member, a required-flag toggle — and stays put for what is
/// not durable shape: a whitespace reformat, a doc or line comment, or a whole-declaration reorder.
/// No structural change is invisible to the fence, and no incidental edit reads as schema drift.
///
/// The `evolve` block is excluded: a consumed block describes work already recorded in the
/// accepted catalog, so binding it would read its deletion as schema drift; the fence tracks the
/// durable shape, not the transition that produced it.
pub(crate) fn analyzed_source_digest(program: &CheckedProgram) -> String {
    hash_records(shape_records(program))
}

/// Both digests from one structural pass, so the evolution preview witness computes the shape
/// digest the store stamps and the evolution digest it records together.
pub(crate) fn source_and_evolution_digests(program: &CheckedProgram) -> (String, String) {
    let shape = shape_records(program);
    let evolution = evolution_records(program, shape.clone());
    (hash_records(shape), hash_records(evolution))
}

/// A stable `sha256:<hex>` digest of the durable shape plus the evolve decision surface:
/// everything [`analyzed_source_digest`] binds plus each `evolve default` value and `evolve
/// transform` body, so a changed default value or transform body drifts it.
///
/// The evolution witness records this digest, not the shape digest, so apply aborts when the
/// source it activates no longer matches what was discharged — including a transform-body edit the
/// shape digest cannot see. The store fences on shape so a consumed block is deletable; the witness
/// fences on shape-plus-intent so the preview-to-apply transition cannot silently change.
pub(crate) fn evolution_digest(program: &CheckedProgram) -> String {
    hash_records(evolution_records(program, shape_records(program)))
}

/// The durable identity of one `evolve transform`: a `sha256:<hex>` of its target's stable
/// catalog id and its canonical body rendering. Apply stamps this on the target so a re-bind
/// recognizes the same transform, and discharge compares against it to skip a transform already
/// applied. Keying on the transform's own target and body — not the whole-program shape — means an
/// unrelated durable edit never moves the mark, so a discharged transform cannot re-execute and
/// corrupt already-migrated data, while a changed body computes a different identity and is
/// correctly a fresh obligation.
pub(crate) fn transform_identity(stable_id: &str, body_text: &str) -> String {
    marrow_project::sha256_digest(format!("transform-v1\0{stable_id}\0{body_text}").as_bytes())
}

/// One durable-shape record per declaration, resource member, store index, enum member, and module
/// `const`, each tagged by category. The records are sorted before hashing, so the digest depends
/// on the set of records rather than their discovery order — a declaration reorder does not move
/// it. A member's own record carries its ordinal within its siblings, so a member reorder, which
/// carries no data but is a tracked shape change the store restamps, does move it.
///
/// Every record identifies its entity by module-qualified path and encodes its shape by canonical
/// source identity — a member's value type and key shape by type name, a store's identity-key types
/// and target resource, an index's uniqueness and key columns, an enum member by its name chain, a
/// const by its canonical value expression. Type identity is the source spelling (`Type`'s
/// canonical form), never a minted catalog id, so the digest is a pure function of the declared
/// shape: it reproduces for the same source on any machine and moves for exactly the edits that
/// change what a stored snapshot must satisfy — a retype, a re-key, an added, removed, or reordered
/// member, a required-flag toggle — while a reformat, comment, or doc edit leaves it fixed.
fn shape_records(program: &CheckedProgram) -> Vec<String> {
    let mut records = Vec::new();
    for module in &program.modules {
        let module_name = module.name.as_str();
        for resource in &module.resources {
            records.push(format!(
                "resource\0{}",
                resource_path(module_name, &resource.name)
            ));
            collect_member_records(
                &mut records,
                module_name,
                &resource.name,
                &[],
                &resource.members,
            );
        }
        for store in &module.stores {
            records.push(format!(
                "store\0{}\0{}",
                store_path(module_name, &store.root),
                store_shape_token(store),
            ));
            for index in &store.indexes {
                records.push(format!(
                    "index\0{}\0{}",
                    store_index_path(module_name, &store.root, &index.name),
                    index_shape_token(index),
                ));
            }
        }
        for enum_schema in &module.enums {
            records.push(format!(
                "enum\0{}",
                enum_path(module_name, &enum_schema.name)
            ));
            for ordinal in 0..enum_schema.members.len() {
                records.push(format!(
                    "enum-member\0{ordinal}\0{}",
                    enum_member_path(module_name, &enum_schema.name, ordinal, enum_schema),
                ));
            }
        }
        for constant in &module.constants {
            records.push(format!(
                "const\0{}\0{}\0{}",
                module_name,
                constant.name,
                const_value(constant),
            ));
        }
    }
    records
}

/// Record every resource member in the tree, outermost first, each by its full name-chain path, its
/// ordinal within its siblings, and its structural shape token. Recurses into group members so a
/// nested field, keyed layer, or group contributes its own record. The ordinal makes a member
/// reorder move the digest, matching the store's restamp-on-reorder contract, while it stays within
/// a parent so reordering one group's members never perturbs another's records.
fn collect_member_records(
    records: &mut Vec<String>,
    module: &str,
    resource: &str,
    parent_path: &[String],
    nodes: &[Node],
) {
    for (ordinal, node) in nodes.iter().enumerate() {
        let mut path = parent_path.to_vec();
        path.push(node.name.clone());
        records.push(format!(
            "member\0{ordinal}\0{}\0{}",
            resource_member_path(module, resource, &path),
            member_shape_token(node),
        ));
        collect_member_records(records, module, resource, &path, &node.members);
    }
}

/// A resource member's structural shape by source identity: a plain field records its value type
/// and whether it is `required`; a keyed leaf records its key types and value type; an unkeyed
/// group and a keyed group record their kind and key shape. A value or key retype, a plain field
/// becoming a keyed leaf, a group becoming a keyed group, or a required-flag toggle each yields a
/// different token; a pure rename of the member (which changes its path) or of a key parameter
/// (which is not a type) does not.
fn member_shape_token(node: &Node) -> String {
    match node.leaf_value_type() {
        Some(value) if node.key_params.is_empty() => {
            format!("field:{}:{value}", node.is_required_field())
        }
        Some(value) => format!("keyed-leaf:[{}]:{value}", key_types(&node.key_params)),
        None if node.key_params.is_empty() => "group".to_string(),
        None => format!("keyed-group:[{}]", key_types(&node.key_params)),
    }
}

/// A store's durable identity shape: the resource it stores and its identity-key types in order.
/// A re-key (a key type or arity change) or a change of target resource moves it; a pure store-root
/// rename (which changes the store's path) is recorded by the path, not this token.
fn store_shape_token(store: &StoreSchema) -> String {
    format!("{}:[{}]", store.resource, key_types(&store.identity_keys))
}

/// An index's declaration shape: its uniqueness and its ordered key columns by name. A uniqueness
/// flip or a change to the key columns moves it; a column's own type change is recorded by that
/// member's shape token.
fn index_shape_token(index: &IndexSchema) -> String {
    format!("unique={}:[{}]", index.unique, index.args.join(","))
}

/// The comma-joined canonical type names of a key list in order, so both arity and each key type
/// are recorded while key-parameter names — which carry no durable identity — are not.
fn key_types(keys: &[KeyDef]) -> String {
    keys.iter()
        .map(|key| key.ty.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// The shape records plus the evolve decision surface: each bound default's value and each bound
/// transform's body, keyed by the target member's source path. Keying on the path rather than a
/// minted catalog id keeps the digest reproducible across checks — a default or transform on a
/// not-yet-accepted member would otherwise carry a fresh random id each check — while a changed
/// default value or transform body still drifts it.
fn evolution_records(program: &CheckedProgram, mut records: Vec<String>) -> Vec<String> {
    for default in &program.catalog.evolve_defaults {
        records.push(format!(
            "default\0{}\0{}",
            default.target_path,
            marrow_syntax::format_expression(&default.value),
        ));
    }
    for transform in &program.catalog.evolve_transforms {
        records.push(format!(
            "transform\0{}\0{}",
            transform.target_path, transform.body_text,
        ));
    }
    records
}

/// The canonical rendering of a `const`'s value expression, or the empty marker for a value-less
/// constant. Rendering the expression alone keeps the digest independent of the declaration's
/// layout and documentation while still moving it when the value changes.
fn const_value(constant: &CheckedConst) -> String {
    match &constant.value {
        Some(value) => marrow_syntax::format_expression(value),
        None => String::new(),
    }
}

/// Hash a record set into the canonical `sha256:<hex>` digest. Sorting makes the digest depend on
/// the set of records, not their discovery order, so reordering whole declarations — whose records
/// carry no ordinal — leaves the digest fixed, while a member reorder, which shifts member
/// ordinals, changes the set and moves it.
fn hash_records(mut records: Vec<String>) -> String {
    records.sort();
    marrow_project::sha256_digest(records.join("\n\0\n").as_bytes())
}
