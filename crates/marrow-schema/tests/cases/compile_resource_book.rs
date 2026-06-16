//! Schema compilation of the canonical `Book` resource: saved root and identity
//! key, required and sparse fields, a keyed-leaf sequence, group and history
//! layers, and an index. Also pins that one store-free shape compiles identically
//! whether local or saved, and that two stores over one resource keep distinct
//! identity types.

use crate::common;
use common::{
    compile_ok, compile_source_errors, layer, resource, resource_and_store, top_level_fields,
};
use marrow_schema::{NodeKind, ScalarType, Type, compile_resource, compile_store};

/// The canonical `Book` resource.
const BOOK: &str = "\
resource Book
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

store ^books(id: int): Book
    index byShelf(shelf, id)
    ";

#[test]
fn book_saved_root_has_one_identity_key() {
    let (resource, store) = resource_and_store(BOOK);
    let (resource_schema, resource_errors) = compile_resource(&resource);
    assert!(resource_errors.is_empty(), "{resource_errors:?}");
    let (store_schema, store_errors) = compile_store(&store, &resource_schema);
    assert!(store_errors.is_empty(), "{store_errors:?}");
    assert_eq!(store_schema.root, "books");
    assert_eq!(store_schema.identity_keys.len(), 1);
    assert_eq!(store_schema.identity_keys[0].name, "id");
    assert_eq!(
        store_schema.identity_keys[0].ty,
        Type::Scalar(ScalarType::Int)
    );
    assert_eq!(
        store_schema.identity_type(),
        Type::Identity("books".to_string())
    );
}

#[test]
fn book_top_level_fields() {
    let schema = compile_ok(BOOK);

    let names: Vec<&str> = top_level_fields(&schema).map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        ["title", "author", "shelf", "currentVersion", "loanedTo"]
    );

    let required: Vec<&str> = top_level_fields(&schema)
        .filter(|f| matches!(f.kind, NodeKind::Slot { required, .. } if required))
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(required, ["title", "author", "shelf", "currentVersion"]);

    let loaned_to = top_level_fields(&schema)
        .find(|f| f.name == "loanedTo")
        .expect("loanedTo field");
    let NodeKind::Slot { ty, required } = &loaned_to.kind else {
        panic!("loanedTo is a slot");
    };
    assert!(!required, "loanedTo is sparse");
    assert_eq!(*ty, Type::Scalar(ScalarType::Str));

    // `tags` is a keyed leaf, not a top-level field.
    assert!(!top_level_fields(&schema).any(|f| f.name == "tags"));
}

#[test]
fn book_tags_is_a_keyed_leaf() {
    let schema = compile_ok(BOOK);
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
fn book_notes_is_a_group() {
    let schema = compile_ok(BOOK);
    let notes = layer(&schema, "notes");
    assert_eq!(notes.key_params.len(), 1);
    assert_eq!(notes.key_params[0].name, "noteId");
    assert_eq!(notes.key_params[0].ty, Type::Scalar(ScalarType::Str));
    assert!(
        matches!(notes.kind, NodeKind::Group),
        "a group has no leaf type"
    );

    assert_eq!(notes.members.len(), 1);
    let text = &notes.members[0];
    let NodeKind::Slot { ty, required } = &text.kind else {
        panic!("notes.text should be a field");
    };
    assert_eq!(text.name, "text");
    assert_eq!(*ty, Type::Scalar(ScalarType::Str));
    assert!(!required);
}

#[test]
fn book_versions_is_a_history_group() {
    let schema = compile_ok(BOOK);
    let versions = layer(&schema, "versions");
    assert_eq!(versions.key_params.len(), 1);
    assert_eq!(versions.key_params[0].name, "version");
    assert_eq!(versions.key_params[0].ty, Type::Scalar(ScalarType::Int));
    assert!(matches!(versions.kind, NodeKind::Group));

    let fields: Vec<(&str, bool, String)> = versions
        .members
        .iter()
        .map(|member| match &member.kind {
            NodeKind::Slot { ty, required } => (member.name.as_str(), *required, ty.to_string()),
            NodeKind::Group => panic!("unexpected nested group `{}`", member.name),
        })
        .collect();
    assert_eq!(
        fields,
        [
            ("title", true, "string".to_string()),
            ("shelf", true, "string".to_string()),
            ("changedAt", true, "instant".to_string()),
        ]
    );
}

#[test]
fn book_byshelf_index() {
    let (resource, store) = resource_and_store(BOOK);
    let (resource_schema, resource_errors) = compile_resource(&resource);
    assert!(resource_errors.is_empty(), "{resource_errors:?}");
    let (store_schema, store_errors) = compile_store(&store, &resource_schema);
    assert!(store_errors.is_empty(), "{store_errors:?}");
    assert_eq!(store_schema.indexes.len(), 1);
    let by_shelf = &store_schema.indexes[0];
    assert_eq!(by_shelf.name, "byShelf");
    assert_eq!(by_shelf.args, ["shelf", "id"]);
    assert!(!by_shelf.unique);
}

#[test]
fn two_stores_over_one_resource_have_distinct_identity_types() {
    let resource = resource(
        "\
resource Book
    title: string
",
    );
    let (book, resource_errors) = compile_resource(&resource);
    assert!(resource_errors.is_empty(), "{resource_errors:?}");
    let (_, books) = resource_and_store(
        "\
resource Book
    title: string
store ^books(id: int): Book
",
    );
    let (_, archived) = resource_and_store(
        "\
resource Book
    title: string
store ^archivedBooks(id: int): Book
",
    );

    let (books, books_errors) = compile_store(&books, &book);
    let (archived, archived_errors) = compile_store(&archived, &book);

    assert!(books_errors.is_empty(), "{books_errors:?}");
    assert!(archived_errors.is_empty(), "{archived_errors:?}");
    assert_eq!(books.identity_type(), Type::Identity("books".to_string()));
    assert_eq!(
        archived.identity_type(),
        Type::Identity("archivedBooks".to_string())
    );
    assert_ne!(books.identity_type(), archived.identity_type());
}

#[test]
fn clean_book_has_no_new_errors() {
    let errors = compile_source_errors(BOOK);
    assert!(errors.is_empty(), "Book is clean: {errors:?}");
}

#[test]
fn one_shape_compiles_as_both_local_and_saved() {
    // A resource's field and layer shape is checked through one schema whether it
    // is attached to a store or used as a local value.
    let saved = compile_ok(
        "\
resource Book
    required title: string
    tags(pos: int): string
store ^books(id: int): Book
",
    );
    let local = compile_ok(
        "\
resource Book
    required title: string
    tags(pos: int): string
",
    );
    // The stored shape is identical regardless of where the resource lives.
    assert_eq!(saved.members, local.members);
}
