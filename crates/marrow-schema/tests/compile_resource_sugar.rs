//! Member sugar: `sequence[T]` and `map[K, V]` desugar to keyed-leaf layers
//! identical to their canonical spellings, top-level and nested. Also pins where
//! a `map` is rejected as an unsupported saved type (local resources, nested in a
//! sequence, in identity/key positions, and as a `required` member).

use marrow_schema::{
    Node, NodeKind, ResourceSchema, SCHEMA_UNSUPPORTED_TYPE, ScalarType, SchemaError,
    SchemaErrorKind, SchemaUnsupportedTypeTarget, Type, check_saved_member_rules, compile_resource,
    compile_store, compile_stored_resource,
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
        let (schema, mut errors) = compile_stored_resource(&resource);
        let (_, store_errors) = compile_store(&store, &schema);
        errors.extend(store_errors);
        errors.extend(check_saved_member_rules(&resource.members));
        (schema, errors)
    } else {
        compile_resource(&resource)
    }
}

fn compile_source_errors(source: &str) -> Vec<SchemaError> {
    let (_, errors) = compile_source(source);
    errors
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

fn codes(errors: &[SchemaError]) -> Vec<&'static str> {
    errors.iter().map(|error| error.code).collect()
}

fn assert_kind(error: &SchemaError, kind: SchemaErrorKind) {
    assert_eq!(error.kind, kind);
}

fn unsupported(target: SchemaUnsupportedTypeTarget, name: &str) -> SchemaErrorKind {
    SchemaErrorKind::UnsupportedType {
        target,
        name: name.to_string(),
    }
}

#[test]
fn sequence_member_desugars_to_a_pos_int_keyed_leaf() {
    // `tags: sequence[string]` is sugar for `tags(pos: int): string`, so it
    // compiles to the same keyed leaf the canonical spelling produces.
    let source = "\
resource Book at ^books(id: int)
    tags: sequence[string]
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
        &compile_ok("resource Book at ^books(id: int)\n    tags: sequence[string]\n"),
        "tags",
    )
    .clone();
    let canonical = layer(
        &compile_ok("resource Book at ^books(id: int)\n    tags(pos: int): string\n"),
        "tags",
    )
    .clone();
    assert_eq!(sugar, canonical);
}

#[test]
fn map_member_matches_the_canonical_keyed_leaf() {
    // `scores: map[string, int]` is sugar for `scores(key: string): int`.
    let sugar = layer(
        &compile_ok("resource Book at ^books(id: int)\n    scores: map[string, int]\n"),
        "scores",
    )
    .clone();
    let canonical = layer(
        &compile_ok("resource Book at ^books(id: int)\n    scores(key: string): int\n"),
        "scores",
    )
    .clone();
    assert_eq!(sugar, canonical);
}

#[test]
fn nested_sequence_member_desugars_inside_a_group() {
    // A sequence nested inside a group desugars the same way.
    let source = "\
resource Book at ^books(id: int)
    versions(version: int)
        notes: sequence[string]
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

#[test]
fn nested_map_member_desugars_inside_a_group() {
    let source = "\
resource Book at ^books(id: int)
    versions(version: int)
        scores: map[string, int]
";
    let schema = compile_ok(source);
    let versions = layer(&schema, "versions");
    let scores = &versions.members[0];
    assert!(
        matches!(&scores.kind, NodeKind::Slot { ty, .. } if *ty == Type::Scalar(ScalarType::Int)),
        "scores should desugar to a nested keyed-leaf layer"
    );
    assert_eq!(scores.name, "scores");
    assert_eq!(scores.key_params.len(), 1);
    assert_eq!(scores.key_params[0].name, "key");
    assert_eq!(scores.key_params[0].ty, Type::Scalar(ScalarType::Str));
}

#[test]
fn map_member_sugar_is_rejected_on_local_resources() {
    let source = "\
resource Draft
    scores: map[string, int]
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert_kind(
        &errors[0],
        unsupported(SchemaUnsupportedTypeTarget::Field, "scores"),
    );
}

#[test]
fn map_type_nested_inside_sequence_is_rejected() {
    let source = "\
resource Book at ^books(id: int)
    scores: sequence[map[string, int]]
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert_kind(
        &errors[0],
        unsupported(SchemaUnsupportedTypeTarget::Field, "scores"),
    );
}

#[test]
fn map_type_in_identity_key_is_rejected() {
    let source = "\
resource Book at ^books(id: map[string, int])
    title: string
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert_kind(
        &errors[0],
        unsupported(SchemaUnsupportedTypeTarget::Key, "id"),
    );
}

#[test]
fn map_type_in_key_param_is_rejected() {
    let source = "\
resource Draft
    scores(k: map[string, int]): int
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert_kind(
        &errors[0],
        unsupported(SchemaUnsupportedTypeTarget::Key, "k"),
    );
}

#[test]
fn map_type_as_map_key_is_rejected_once() {
    let source = "\
resource Book at ^books(id: int)
    scores: map[map[string, int], int]
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert_kind(
        &errors[0],
        unsupported(SchemaUnsupportedTypeTarget::Key, "key"),
    );
}

#[test]
fn required_map_member_sugar_is_rejected() {
    let source = "\
resource Book at ^books(id: int)
    required scores: map[string, int]
";
    let errors = compile_source_errors(source);
    assert_eq!(codes(&errors), [SCHEMA_UNSUPPORTED_TYPE]);
    assert_kind(
        &errors[0],
        unsupported(SchemaUnsupportedTypeTarget::Field, "scores"),
    );
}
