//! Member sugar: `sequence[T]` desugars to a keyed-leaf layer identical to its
//! canonical spelling, top-level and nested.

use marrow_schema::{
    Node, NodeKind, ResourceSchema, ScalarType, SchemaError, Type, check_saved_member_rules,
    compile_resource, compile_store,
};
use marrow_syntax::{Declaration, parse_source};

/// Compile `source`'s resource, asserting it produced no schema errors.
fn compile_ok(source: &str) -> ResourceSchema {
    let (schema, errors) = compile_source(source);
    assert!(errors.is_empty(), "unexpected schema errors: {errors:?}");
    schema
}

fn compile_source(source: &str) -> (ResourceSchema, Vec<SchemaError>) {
    let parsed = parse_source(source);
    assert!(
        !parsed.has_errors(),
        "source should parse cleanly: {:?}",
        parsed.diagnostics
    );
    let mut resource = None;
    let mut store = None;
    for declaration in parsed.file.declarations {
        match declaration {
            Declaration::Resource(decl) => resource = Some(decl),
            Declaration::Store(decl) => store = Some(decl),
            _ => {}
        }
    }
    let resource = resource.expect("resource declaration");
    if let Some(store) = store {
        let (schema, mut errors) = compile_resource(&resource);
        let (_, store_errors) = compile_store(&store, &schema);
        errors.extend(store_errors);
        errors.extend(check_saved_member_rules(&resource.members));
        (schema, errors)
    } else {
        compile_resource(&resource)
    }
}

/// The top-level node named `name` (a keyed leaf or a group).
fn layer<'a>(schema: &'a ResourceSchema, name: &str) -> &'a Node {
    schema
        .members
        .iter()
        .find(|node| node.name == name)
        .unwrap_or_else(|| panic!("layer `{name}` not found"))
}

/// The top-level nodes classified by the production schema API as plain fields.
fn top_level_fields(schema: &ResourceSchema) -> impl Iterator<Item = &Node> {
    schema.members.iter().filter(|node| node.is_plain_field())
}

#[test]
fn sequence_member_desugars_to_a_pos_int_keyed_leaf() {
    // `tags: sequence[string]` is sugar for `tags(pos: int): string`, so it
    // compiles to the same keyed leaf the canonical spelling produces.
    let source = "\
resource Book
    tags: sequence[string]
store ^books(id: int): Book
";
    let schema = compile_ok(source);
    // It is a keyed leaf, not a plain top-level field.
    assert!(!top_level_fields(&schema).any(|f| f.name == "tags"));
    let tags = layer(&schema, "tags");
    assert_eq!(tags.key_params.len(), 1);
    assert_eq!(tags.key_params[0].name, "pos");
    assert_eq!(tags.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert!(
        matches!(&tags.kind, NodeKind::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Str))
    );
    assert!(tags.members.is_empty(), "a keyed leaf has no members");
}

#[test]
fn sequence_member_matches_the_canonical_keyed_leaf() {
    // The desugared layer is identical to the canonical `tags(pos: int): string`.
    let sugar = layer(
        &compile_ok("resource Book\n    tags: sequence[string]\nstore ^books(id: int): Book\n"),
        "tags",
    )
    .clone();
    let canonical = layer(
        &compile_ok("resource Book\n    tags(pos: int): string\nstore ^books(id: int): Book\n"),
        "tags",
    )
    .clone();
    assert_eq!(sugar, canonical);
}

#[test]
fn nested_sequence_member_desugars_inside_a_group() {
    // A sequence nested inside a group desugars the same way.
    let source = "\
resource Book
    versions(version: int)
        notes: sequence[string]
store ^books(id: int): Book
";
    let schema = compile_ok(source);
    let versions = layer(&schema, "versions");
    let notes = &versions.members[0];
    assert!(
        matches!(&notes.kind, NodeKind::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Str)),
        "notes should desugar to a nested keyed-leaf layer"
    );
    assert_eq!(notes.name, "notes");
    assert_eq!(notes.key_params.len(), 1);
    assert_eq!(notes.key_params[0].name, "pos");
    assert_eq!(notes.key_params[0].ty, Type::Scalar(ScalarType::Int));
}
