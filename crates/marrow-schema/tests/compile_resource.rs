//! Schema compilation tests.
//!
//! The primary case is the `Book` resource from `docs/language/sample.md`,
//! which exercises the saved root, required and sparse fields, a keyed-leaf
//! sequence, group and history layers, and an index. The remaining cases pin
//! the structural errors this slice reports.

use marrow_schema::{
    LayerMember, LayerSchema, ResourceSchema, SCHEMA_DUPLICATE_MEMBER, SCHEMA_INDEX_IN_GROUP,
    SCHEMA_KEY_MEMBER_COLLISION, SCHEMA_UNKNOWN_IN_SAVED, compile_resource,
};
use marrow_syntax::{Declaration, ResourceDecl, parse_source};

/// Parse `source` and return its single resource declaration.
fn resource(source: &str) -> ResourceDecl {
    let parsed = parse_source(source);
    assert!(
        !parsed.has_errors(),
        "source should parse cleanly: {:?}",
        parsed.diagnostics
    );
    parsed
        .file
        .declarations
        .into_iter()
        .find_map(|declaration| match declaration {
            Declaration::Resource(resource) => Some(resource),
            _ => None,
        })
        .expect("a resource declaration")
}

/// Compile `source`'s resource, asserting it produced no schema errors.
fn compile_ok(source: &str) -> ResourceSchema {
    let (schema, errors) = compile_resource(&resource(source));
    assert!(errors.is_empty(), "unexpected schema errors: {errors:?}");
    schema
}

fn layer<'a>(schema: &'a ResourceSchema, name: &str) -> &'a LayerSchema {
    schema
        .layers
        .iter()
        .find(|layer| layer.name == name)
        .unwrap_or_else(|| panic!("layer `{name}` not found"))
}

/// The `Book` resource from `docs/language/sample.md`.
const BOOK: &str = "\
resource Book at ^books(id: int)
    required title: string
    required author: string
    required shelf: string
    required currentVersion: int
    loanedTo: string
    tags(pos: int): string

    notes(noteId: string)
        text: string

    versions(version: int)
        required title: string
        required shelf: string
        required changedAt: instant

    index byShelf(shelf, id)
";

#[test]
fn book_saved_root_has_one_identity_key() {
    let schema = compile_ok(BOOK);
    let root = schema.saved_root.expect("Book has a saved root");
    assert_eq!(root.root, "books");
    assert_eq!(root.identity_keys.len(), 1);
    assert_eq!(root.identity_keys[0].name, "id");
    assert_eq!(root.identity_keys[0].ty.text, "int");
}

#[test]
fn book_top_level_fields() {
    let schema = compile_ok(BOOK);

    let names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        ["title", "author", "shelf", "currentVersion", "loanedTo"]
    );

    let required: Vec<&str> = schema
        .fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(required, ["title", "author", "shelf", "currentVersion"]);

    let loaned_to = schema
        .fields
        .iter()
        .find(|f| f.name == "loanedTo")
        .expect("loanedTo field");
    assert!(!loaned_to.required, "loanedTo is sparse");
    assert_eq!(loaned_to.ty.text, "string");

    // `tags` is a keyed leaf, not a top-level field.
    assert!(!schema.fields.iter().any(|f| f.name == "tags"));
}

#[test]
fn book_tags_is_a_keyed_leaf() {
    let schema = compile_ok(BOOK);
    let tags = layer(&schema, "tags");
    assert_eq!(tags.key_params.len(), 1);
    assert_eq!(tags.key_params[0].name, "pos");
    assert_eq!(tags.key_params[0].ty.text, "int");
    assert_eq!(
        tags.leaf_type.as_ref().map(|t| t.text.as_str()),
        Some("string")
    );
    assert!(tags.members.is_empty(), "a keyed leaf has no members");
}

#[test]
fn book_notes_is_a_group() {
    let schema = compile_ok(BOOK);
    let notes = layer(&schema, "notes");
    assert_eq!(notes.key_params.len(), 1);
    assert_eq!(notes.key_params[0].name, "noteId");
    assert_eq!(notes.key_params[0].ty.text, "string");
    assert!(notes.leaf_type.is_none(), "a group has no leaf type");

    assert_eq!(notes.members.len(), 1);
    let LayerMember::Field(text) = &notes.members[0] else {
        panic!("notes.text should be a field");
    };
    assert_eq!(text.name, "text");
    assert_eq!(text.ty.text, "string");
    assert!(!text.required);
}

#[test]
fn book_versions_is_a_history_group() {
    let schema = compile_ok(BOOK);
    let versions = layer(&schema, "versions");
    assert_eq!(versions.key_params.len(), 1);
    assert_eq!(versions.key_params[0].name, "version");
    assert_eq!(versions.key_params[0].ty.text, "int");
    assert!(versions.leaf_type.is_none());

    let fields: Vec<(&str, bool, &str)> = versions
        .members
        .iter()
        .map(|member| match member {
            LayerMember::Field(field) => {
                (field.name.as_str(), field.required, field.ty.text.as_str())
            }
            LayerMember::Layer(layer) => panic!("unexpected nested layer `{}`", layer.name),
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("title", true, "string"),
            ("shelf", true, "string"),
            ("changedAt", true, "instant"),
        ]
    );
}

#[test]
fn book_byshelf_index() {
    let schema = compile_ok(BOOK);
    assert_eq!(schema.indexes.len(), 1);
    let by_shelf = &schema.indexes[0];
    assert_eq!(by_shelf.name, "byShelf");
    assert_eq!(by_shelf.args, ["shelf", "id"]);
    assert!(!by_shelf.unique);
}

#[test]
fn index_inside_group_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    notes(noteId: string)
        text: string
        index byText(text)
";
    let (schema, errors) = compile_resource(&resource(source));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, SCHEMA_INDEX_IN_GROUP);
    // The misplaced index is dropped, not promoted to a resource index.
    assert!(schema.indexes.is_empty());
}

#[test]
fn duplicate_member_name_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    required title: string
    title: string
";
    let (schema, errors) = compile_resource(&resource(source));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, SCHEMA_DUPLICATE_MEMBER);
    // Best-effort: every parsed member is kept in source order; the collision
    // is reported, not silently dropped. The duplicate's span points at the
    // second `title`.
    assert_eq!(schema.fields.len(), 2);
    assert_eq!(errors[0].span.line, 3);
}

/// Only this code, to keep `unknown`/collision assertions specific.
fn codes(errors: &[marrow_schema::SchemaError]) -> Vec<&'static str> {
    errors.iter().map(|error| error.code).collect()
}

#[test]
fn saved_field_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    required title: string
    note: unknown
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("note"));
}

#[test]
fn saved_identity_key_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: unknown)
    required title: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("id"));
}

#[test]
fn saved_keyed_leaf_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    tags(pos: int): unknown
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("tags"));
}

#[test]
fn saved_nested_field_typed_unknown_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    notes(noteId: string)
        body: unknown
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_UNKNOWN_IN_SAVED]);
    assert!(errors[0].message.contains("body"));
}

#[test]
fn local_field_typed_unknown_is_allowed() {
    let source = "\
resource Draft
    title: string
    note: unknown
";
    let (_, errors) = compile_resource(&resource(source));
    assert!(
        errors.is_empty(),
        "local resources may use `unknown`: {errors:?}"
    );
}

#[test]
fn identity_key_name_colliding_with_field_is_an_error() {
    let source = "\
resource Book at ^books(id: int)
    required id: int
    required title: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_KEY_MEMBER_COLLISION]);
    assert!(errors[0].message.contains("id"));
}

#[test]
fn identity_key_name_colliding_with_layer_is_an_error() {
    let source = "\
resource Book at ^books(notes: int)
    notes(noteId: string)
        text: string
";
    let (_, errors) = compile_resource(&resource(source));
    assert_eq!(codes(&errors), [SCHEMA_KEY_MEMBER_COLLISION]);
    assert!(errors[0].message.contains("notes"));
}

#[test]
fn clean_book_has_no_new_errors() {
    let (_, errors) = compile_resource(&resource(BOOK));
    assert!(errors.is_empty(), "Book is clean: {errors:?}");
}
