//! Managed whole-resource writes: validate against the schema, lower the fields
//! into the store, and keep generated index entries coherent.

use marrow_schema::{ResourceSchema, compile_resource};
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, decode_key_value, encode_path};
use marrow_store::value::{SavedValue, ValueType, decode_value};
use marrow_syntax::{Declaration, parse_source};
use marrow_write::{
    FieldValue, ResourceValue, WRITE_IDENTITY_MISMATCH, WRITE_LAYER_KEY_ARITY, WRITE_NO_SAVED_ROOT,
    WRITE_NOT_A_GROUP_LAYER, WRITE_NOT_A_LEAF_LAYER, WRITE_REQUIRED_ABSENT, WRITE_TYPE_MISMATCH,
    WRITE_UNIQUE_CONFLICT, WRITE_UNKNOWN_FIELD, WRITE_UNKNOWN_LAYER, next_id, next_layer_pos,
    plan_field_write, plan_layer_field_write, plan_layer_group_write, plan_layer_leaf_write,
    plan_layer_merge, plan_resource_delete, plan_resource_merge, plan_resource_write,
};

/// Compile the single resource declared in `source`.
fn schema(source: &str) -> ResourceSchema {
    let parsed = parse_source(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
    let decl = parsed
        .file
        .declarations
        .into_iter()
        .find_map(|declaration| match declaration {
            Declaration::Resource(resource) => Some(resource),
            _ => None,
        })
        .expect("a resource declaration");
    let (schema, errors) = compile_resource(&decl);
    assert!(errors.is_empty(), "{errors:?}");
    schema
}

/// The `Book` resource: a saved root with one required and one sparse field.
const BOOK: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string
";

/// `Book` with a non-unique index over the shelf and identity.
const BOOK_INDEXED: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)
";

/// `Book` with a non-unique index spanning two plain fields plus identity, so a
/// field write must rebuild a composite key from another field's stored value.
const BOOK_TWO_INDEX_FIELDS: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string
    category: string

    index byShelfCategory(shelf, category, id)
";

/// `Book` with a UNIQUE index over the isbn alone: the entry path is the isbn
/// only, and the entry value is the owning identity.
const BOOK_UNIQUE: &str = "\
resource Book at ^books(id: int)
    required title: string
    isbn: string

    index byIsbn(isbn) unique
";

/// A resource with a COMPOSITE identity and a unique index, so a unique entry's
/// value is a two-key identity that must round-trip in order.
const ITEM_UNIQUE: &str = "\
resource Item at ^items(tenant: string, id: int)
    required name: string
    sku: string

    index bySku(sku) unique
";

/// `Book` with a keyed-leaf layer (`tags`) and two group layers — `notes`
/// (single-member) and `versions` (multi-member) — to exercise keyed-leaf
/// writes, group-entry field writes, and the leaf-vs-group distinction.
const BOOK_LAYERS: &str = "\
resource Book at ^books(id: int)
    required title: string
    tags(pos: int): string

    notes(noteId: string)
        text: string

    versions(version: int)
        required title: string
        required shelf: string
";

/// A LOCAL `Book` (no saved root) with a keyed-leaf layer, to exercise the
/// no-saved-root rejection through the layer planner.
const LOCAL_BOOK_LAYERS: &str = "\
resource Book
    required title: string
    tags(pos: int): string
";

fn saved(text: &str) -> FieldValue {
    FieldValue::Saved(SavedValue::Str(text.into()))
}

/// Plan and commit a whole-resource write against the current store state.
fn write(
    store: &mut MemStore,
    schema: &ResourceSchema,
    identity: &[SavedKey],
    value: ResourceValue,
) {
    plan_resource_write(schema, identity, &value, store)
        .expect("valid write")
        .commit(store)
        .expect("commit succeeds");
}

/// Plan and commit a single-field write against the current store state.
fn write_field(
    store: &mut MemStore,
    schema: &ResourceSchema,
    identity: &[SavedKey],
    field: &str,
    value: SavedValue,
) {
    plan_field_write(schema, identity, field, &value, store)
        .expect("valid field write")
        .commit(store)
        .expect("commit succeeds");
}

/// Plan and commit a merge against the current store state.
fn merge(
    store: &mut MemStore,
    schema: &ResourceSchema,
    identity: &[SavedKey],
    value: ResourceValue,
) {
    plan_resource_merge(schema, identity, &value, store)
        .expect("valid merge")
        .commit(store)
        .expect("commit succeeds");
}

/// Plan and commit a keyed-leaf write of `^books(id).tags(pos)`.
fn write_tag(store: &mut MemStore, schema: &ResourceSchema, id: i64, pos: i64, value: &str) {
    plan_layer_leaf_write(
        schema,
        &[SavedKey::Int(id)],
        "tags",
        &[SavedKey::Int(pos)],
        &SavedValue::Str(value.into()),
    )
    .expect("valid keyed-leaf write")
    .commit(store)
    .expect("commit succeeds");
}

/// The encoded path `^books(id).tags(pos)` of a keyed-leaf entry.
fn tag_entry(id: i64, pos: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::ChildLayer("tags".into()),
        PathSegment::IndexKey(SavedKey::Int(pos)),
    ])
}

/// The encoded path `^books(id).field`.
fn field_path(id: i64, field: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::Field(field.into()),
    ])
}

/// Plan and commit a group-entry field write of `^books(id).notes(noteId).text`.
fn write_note(store: &mut MemStore, schema: &ResourceSchema, id: i64, note: &str, text: &str) {
    plan_layer_field_write(
        schema,
        &[SavedKey::Int(id)],
        "notes",
        &[SavedKey::Str(note.into())],
        "text",
        &SavedValue::Str(text.into()),
    )
    .expect("valid group-entry field write")
    .commit(store)
    .expect("commit succeeds");
}

/// The encoded path `^books(id).notes(noteId).text` of a group-entry field.
fn note_text_entry(id: i64, note: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::ChildLayer("notes".into()),
        PathSegment::IndexKey(SavedKey::Str(note.into())),
        PathSegment::Field("text".into()),
    ])
}

/// The encoded path `^books(id).versions(version).field` of a group-entry field.
fn version_field(id: i64, version: i64, field: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::ChildLayer("versions".into()),
        PathSegment::IndexKey(SavedKey::Int(version)),
        PathSegment::Field(field.into()),
    ])
}

/// A resource with a `versions(version)` GROUP layer (a required and a sparse
/// field) and a `tags(pos)` LEAF layer.
const VERSIONED: &str = "\
resource Book at ^books(id: int)
    required title: string
    versions(version: int)
        required title: string
        note: string
    tags(pos: int): string
";

#[test]
fn a_whole_group_entry_write_replaces_the_entry() {
    let book = schema(VERSIONED);
    let mut store = MemStore::new();
    // Write a version entry with both fields, then overwrite it with only the
    // required one; the entry is replaced, so the old note is gone.
    plan_layer_group_write(
        &book,
        &[SavedKey::Int(1)],
        "versions",
        &[SavedKey::Int(2)],
        &ResourceValue {
            fields: vec![
                ("title".into(), saved("v2")),
                ("note".into(), saved("first")),
            ],
        },
    )
    .expect("valid group write")
    .commit(&mut store)
    .expect("commit");
    plan_layer_group_write(
        &book,
        &[SavedKey::Int(1)],
        "versions",
        &[SavedKey::Int(2)],
        &ResourceValue {
            fields: vec![("title".into(), saved("v2-edited"))],
        },
    )
    .expect("valid group write")
    .commit(&mut store)
    .expect("commit");

    assert_eq!(
        decode_value(
            store.read(&version_field(1, 2, "title")).expect("title"),
            ValueType::Str
        ),
        Some(SavedValue::Str("v2-edited".into()))
    );
    // The replace dropped the previously-written note.
    assert_eq!(store.read(&version_field(1, 2, "note")), None);
}

#[test]
fn a_whole_group_entry_write_requires_required_fields() {
    let book = schema(VERSIONED);
    let result = plan_layer_group_write(
        &book,
        &[SavedKey::Int(1)],
        "versions",
        &[SavedKey::Int(2)],
        &ResourceValue {
            fields: vec![("note".into(), saved("x"))],
        },
    );
    assert_eq!(result.unwrap_err().code, WRITE_REQUIRED_ABSENT);
}

#[test]
fn a_whole_group_entry_write_rejects_a_leaf_layer() {
    let book = schema(VERSIONED);
    let result = plan_layer_group_write(
        &book,
        &[SavedKey::Int(1)],
        "tags",
        &[SavedKey::Int(0)],
        &ResourceValue { fields: vec![] },
    );
    assert_eq!(result.unwrap_err().code, WRITE_NOT_A_GROUP_LAYER);
}

#[test]
fn a_whole_resource_write_lowers_fields_into_the_store() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Small Gods")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );

    for (field, expected) in [("title", "Small Gods"), ("shelf", "fiction")] {
        let bytes = store
            .read(&field_path(42, field))
            .unwrap_or_else(|| panic!("{field} present"));
        assert_eq!(
            decode_value(bytes, ValueType::Str),
            Some(SavedValue::Str(expected.into()))
        );
    }
}

#[test]
fn a_sparse_field_may_be_omitted() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(7)],
        ResourceValue {
            fields: vec![("title".into(), saved("Mort"))],
        },
    );
    assert_eq!(
        store.read(&field_path(7, "shelf")),
        None,
        "sparse field omitted"
    );
}

#[test]
fn a_missing_required_field_is_rejected_with_no_write() {
    let book = schema(BOOK);
    let value = ResourceValue {
        fields: vec![("shelf".into(), saved("fiction"))],
    };
    let result = plan_resource_write(&book, &[SavedKey::Int(42)], &value, &MemStore::new());
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_REQUIRED_ABSENT),
        "{result:?}"
    );
}

#[test]
fn a_field_type_mismatch_is_rejected() {
    let book = schema(BOOK);
    // `title` is a string; an int does not satisfy it.
    let value = ResourceValue {
        fields: vec![("title".into(), FieldValue::Saved(SavedValue::Int(5)))],
    };
    let result = plan_resource_write(&book, &[SavedKey::Int(42)], &value, &MemStore::new());
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_TYPE_MISMATCH),
        "{result:?}"
    );
}

#[test]
fn a_whole_resource_write_replaces_the_previous_value() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("draft")),
                ("shelf".into(), saved("new")),
            ],
        },
    );
    // The second write omits the sparse field; replace semantics remove it.
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![("title".into(), saved("final"))],
        },
    );

    assert_eq!(
        decode_value(
            store.read(&field_path(1, "title")).expect("title"),
            ValueType::Str
        ),
        Some(SavedValue::Str("final".into()))
    );
    assert_eq!(store.read(&field_path(1, "shelf")), None, "replaced away");
}

#[test]
fn next_id_allocates_after_the_highest_record() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    assert_eq!(next_id("books", &store), Ok(1));

    // Write records 5 and 1 (out of order); the next id is one past the highest.
    for id in [5, 1] {
        write(
            &mut store,
            &book,
            &[SavedKey::Int(id)],
            ResourceValue {
                fields: vec![("title".into(), saved("t"))],
            },
        );
    }
    assert_eq!(next_id("books", &store), Ok(6));
}

/// The encoded path `^books.byShelf(shelf, id)`.
fn by_shelf_entry(shelf: &str, id: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Index("byShelf".into()),
        PathSegment::IndexKey(SavedKey::Str(shelf.into())),
        PathSegment::IndexKey(SavedKey::Int(id)),
    ])
}

/// The encoded path `^books.byShelfCategory(shelf, category, id)`.
fn by_shelf_category_entry(shelf: &str, category: &str, id: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Index("byShelfCategory".into()),
        PathSegment::IndexKey(SavedKey::Str(shelf.into())),
        PathSegment::IndexKey(SavedKey::Str(category.into())),
        PathSegment::IndexKey(SavedKey::Int(id)),
    ])
}

/// The encoded path `^books.byIsbn(isbn)` of a unique index entry.
fn by_isbn_entry(isbn: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Index("byIsbn".into()),
        PathSegment::IndexKey(SavedKey::Str(isbn.into())),
    ])
}

/// The encoded path `^items.bySku(sku)` of a unique index entry.
fn by_sku_entry(sku: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("items".into()),
        PathSegment::Index("bySku".into()),
        PathSegment::IndexKey(SavedKey::Str(sku.into())),
    ])
}

/// A `Book` with the given id, title, and isbn.
fn book_with_isbn(title: &str, isbn: &str) -> ResourceValue {
    ResourceValue {
        fields: vec![("title".into(), saved(title)), ("isbn".into(), saved(isbn))],
    }
}

/// Assert that a unique index entry exists and points at the given integer id.
fn assert_owns(store: &MemStore, entry: &[u8], id: i64) {
    let bytes = store.read(entry).expect("unique entry present");
    assert_eq!(
        decode_key_value(bytes),
        Some((SavedKey::Int(id), bytes.len())),
        "entry points at id {id}"
    );
}

#[test]
fn a_write_emits_non_unique_index_entries() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        Some(&b"1"[..]),
        "index entry present"
    );
}

#[test]
fn no_index_entry_when_an_indexed_field_is_absent() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    // `shelf` (an index argument) is omitted, so the byShelf entry is not written.
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![("title".into(), saved("Mort"))],
        },
    );
    let prefix = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Index("byShelf".into()),
    ]);
    assert_eq!(store.scan(&prefix, usize::MAX).entries.len(), 0);
}

#[test]
fn re_writing_an_indexed_field_moves_the_index_entry() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    // Re-write with a different shelf; the old index entry is torn down.
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("history")),
            ],
        },
    );

    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        None,
        "old entry gone"
    );
    assert_eq!(
        store.read(&by_shelf_entry("history", 42)),
        Some(&b"1"[..]),
        "new entry present"
    );
}

#[test]
fn deleting_a_resource_removes_its_fields_and_index_entries() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );

    let plan = plan_resource_delete(&book, &[SavedKey::Int(42)], &store).expect("delete");
    plan.commit(&mut store).expect("commit succeeds");

    assert_eq!(store.read(&field_path(42, "title")), None, "field removed");
    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        None,
        "index entry removed"
    );
}

#[test]
fn a_field_write_updates_one_field_and_leaves_the_rest() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(3)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("draft")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    write_field(
        &mut store,
        &book,
        &[SavedKey::Int(3)],
        "title",
        SavedValue::Str("final".into()),
    );

    assert_eq!(
        decode_value(
            store.read(&field_path(3, "title")).expect("title"),
            ValueType::Str
        ),
        Some(SavedValue::Str("final".into()))
    );
    assert_eq!(
        decode_value(
            store.read(&field_path(3, "shelf")).expect("shelf"),
            ValueType::Str
        ),
        Some(SavedValue::Str("fiction".into())),
        "the untouched field is left in place"
    );
}

#[test]
fn a_field_write_to_an_unknown_field_is_rejected() {
    let book = schema(BOOK);
    let result = plan_field_write(
        &book,
        &[SavedKey::Int(3)],
        "publisher",
        &SavedValue::Str("x".into()),
        &MemStore::new(),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNKNOWN_FIELD),
        "{result:?}"
    );
}

#[test]
fn a_field_write_with_a_mismatched_type_is_rejected() {
    let book = schema(BOOK);
    // `title` is a string; an int does not satisfy it.
    let result = plan_field_write(
        &book,
        &[SavedKey::Int(3)],
        "title",
        &SavedValue::Int(5),
        &MemStore::new(),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_TYPE_MISMATCH),
        "{result:?}"
    );
}

#[test]
fn a_field_write_moves_the_index_entry_of_an_indexed_field() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    write_field(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        "shelf",
        SavedValue::Str("history".into()),
    );

    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        None,
        "old entry gone"
    );
    assert_eq!(
        store.read(&by_shelf_entry("history", 42)),
        Some(&b"1"[..]),
        "new entry present"
    );
    assert_eq!(
        decode_value(
            store.read(&field_path(42, "title")).expect("title"),
            ValueType::Str
        ),
        Some(SavedValue::Str("Mort".into())),
        "the untouched field is left in place"
    );
}

#[test]
fn a_field_write_that_populates_an_indexed_field_adds_its_entry() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    // Seed without the indexed `shelf`, so no byShelf entry exists yet.
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![("title".into(), saved("Mort"))],
        },
    );
    write_field(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        "shelf",
        SavedValue::Str("fiction".into()),
    );
    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        Some(&b"1"[..]),
        "entry added when the field becomes populated"
    );
}

#[test]
fn a_field_write_rebuilds_a_composite_index_key_from_stored_args() {
    let book = schema(BOOK_TWO_INDEX_FIELDS);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
                ("category".into(), saved("fantasy")),
            ],
        },
    );
    // Writing only `shelf` rebuilds the composite key, reading the untouched
    // `category` from the store for the new entry.
    write_field(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        "shelf",
        SavedValue::Str("history".into()),
    );

    assert_eq!(
        store.read(&by_shelf_category_entry("fiction", "fantasy", 42)),
        None,
        "old composite entry gone"
    );
    assert_eq!(
        store.read(&by_shelf_category_entry("history", "fantasy", 42)),
        Some(&b"1"[..]),
        "new composite entry keeps the untouched category"
    );
}

#[test]
fn a_field_write_with_an_unchanged_value_keeps_the_index_entry() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    // Re-writing the same value deletes then re-writes the same entry path, so
    // the entry survives.
    write_field(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        "shelf",
        SavedValue::Str("fiction".into()),
    );
    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        Some(&b"1"[..]),
        "unchanged-value field write keeps the entry"
    );
}

#[test]
fn a_unique_index_entry_stores_the_owning_identity() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(7)],
        book_with_isbn("Mort", "978-0"),
    );
    // The entry value is the identity, not the non-unique presence marker.
    assert_ne!(store.read(&by_isbn_entry("978-0")), Some(&b"1"[..]));
    assert_owns(&store, &by_isbn_entry("978-0"), 7);
}

#[test]
fn no_unique_entry_when_the_indexed_field_is_absent() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    // `isbn` omitted: absence is not a unique value, so no entry exists.
    write(
        &mut store,
        &book,
        &[SavedKey::Int(7)],
        ResourceValue {
            fields: vec![("title".into(), saved("Mort"))],
        },
    );
    let prefix = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Index("byIsbn".into()),
    ]);
    assert_eq!(store.scan(&prefix, usize::MAX).entries.len(), 0);
}

#[test]
fn a_unique_conflict_is_rejected_with_no_write() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );

    // A different record claiming the same isbn is rejected.
    let result = plan_resource_write(
        &book,
        &[SavedKey::Int(2)],
        &book_with_isbn("B", "X"),
        &store,
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNIQUE_CONFLICT),
        "{result:?}"
    );
    // Record 2 was never written, and record 1 still owns the entry.
    assert_eq!(
        store.read(&field_path(2, "title")),
        None,
        "no trace of record 2"
    );
    assert_owns(&store, &by_isbn_entry("X"), 1);
}

#[test]
fn re_writing_the_same_record_with_the_same_unique_key_is_not_a_conflict() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    // Re-writing record 1 with its own isbn must succeed.
    plan_resource_write(
        &book,
        &[SavedKey::Int(1)],
        &book_with_isbn("A2", "X"),
        &store,
    )
    .expect("self re-write is not a conflict")
    .commit(&mut store)
    .expect("commit succeeds");
    assert_owns(&store, &by_isbn_entry("X"), 1);
}

#[test]
fn re_writing_a_unique_field_moves_the_entry() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "Y"),
    );
    assert_eq!(store.read(&by_isbn_entry("X")), None, "old key released");
    assert_owns(&store, &by_isbn_entry("Y"), 1);
}

#[test]
fn freeing_a_unique_key_lets_another_record_take_it() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    // Record 1 moves off X, then record 2 can claim it.
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "Y"),
    );
    write(
        &mut store,
        &book,
        &[SavedKey::Int(2)],
        book_with_isbn("B", "X"),
    );
    assert_owns(&store, &by_isbn_entry("X"), 2);
}

#[test]
fn deleting_a_resource_removes_its_unique_index_entry() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    plan_resource_delete(&book, &[SavedKey::Int(1)], &store)
        .expect("delete")
        .commit(&mut store)
        .expect("commit succeeds");
    assert_eq!(
        store.read(&by_isbn_entry("X")),
        None,
        "unique entry removed"
    );
}

#[test]
fn a_field_write_to_a_unique_field_moves_the_entry() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    write_field(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        "isbn",
        SavedValue::Str("Y".into()),
    );
    assert_eq!(store.read(&by_isbn_entry("X")), None, "old key released");
    assert_owns(&store, &by_isbn_entry("Y"), 1);
}

#[test]
fn a_field_write_to_a_unique_field_detects_a_conflict() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    write(
        &mut store,
        &book,
        &[SavedKey::Int(2)],
        book_with_isbn("B", "Y"),
    );

    // Record 2 trying to take record 1's isbn via a field write is rejected.
    let result = plan_field_write(
        &book,
        &[SavedKey::Int(2)],
        "isbn",
        &SavedValue::Str("X".into()),
        &store,
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNIQUE_CONFLICT),
        "{result:?}"
    );
    // Record 2 still owns its own entry, unchanged.
    assert_owns(&store, &by_isbn_entry("Y"), 2);
}

#[test]
fn replacing_a_record_without_its_unique_field_tears_down_the_entry() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    // Re-write record 1 without an isbn: absence is not a unique value.
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![("title".into(), saved("A"))],
        },
    );
    assert_eq!(
        store.read(&by_isbn_entry("X")),
        None,
        "unique entry torn down"
    );
}

#[test]
fn a_composite_identity_unique_entry_round_trips_and_detects_conflict() {
    let item = schema(ITEM_UNIQUE);
    let mut store = MemStore::new();
    let acme5 = [SavedKey::Str("acme".into()), SavedKey::Int(5)];
    write(
        &mut store,
        &item,
        &acme5,
        ResourceValue {
            fields: vec![
                ("name".into(), saved("Widget")),
                ("sku".into(), saved("ABC")),
            ],
        },
    );

    // The entry value decodes back to the full composite identity, in order,
    // with nothing left over.
    let bytes = store.read(&by_sku_entry("ABC")).expect("entry");
    let (first, used) = decode_key_value(bytes).expect("first identity key");
    let (second, rest) = decode_key_value(&bytes[used..]).expect("second identity key");
    assert_eq!(
        (first, second),
        (SavedKey::Str("acme".into()), SavedKey::Int(5))
    );
    assert_eq!(used + rest, bytes.len(), "exactly the identity");

    // A different identity claiming the same sku conflicts...
    let acme6 = [SavedKey::Str("acme".into()), SavedKey::Int(6)];
    let dup = ResourceValue {
        fields: vec![
            ("name".into(), saved("Gadget")),
            ("sku".into(), saved("ABC")),
        ],
    };
    assert!(
        matches!(
            plan_resource_write(&item, &acme6, &dup, &store),
            Err(ref error) if error.code == WRITE_UNIQUE_CONFLICT
        ),
        "a different identity must conflict"
    );
    // ...but the owner re-writing itself does not.
    let same = ResourceValue {
        fields: vec![
            ("name".into(), saved("Widget v2")),
            ("sku".into(), saved("ABC")),
        ],
    };
    plan_resource_write(&item, &acme5, &same, &store).expect("self re-write is not a conflict");
}

#[test]
fn a_merge_overwrites_supplied_fields_and_leaves_the_rest() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    // Merge supplies only `shelf`; `title` is left as stored.
    merge(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![("shelf".into(), saved("history"))],
        },
    );

    let read = |field| {
        decode_value(
            store.read(&field_path(1, field)).expect(field),
            ValueType::Str,
        )
    };
    assert_eq!(
        read("shelf"),
        Some(SavedValue::Str("history".into())),
        "overwritten"
    );
    assert_eq!(
        read("title"),
        Some(SavedValue::Str("Mort".into())),
        "untouched"
    );
}

#[test]
fn a_merge_into_an_empty_identity_writes_supplied_fields() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    merge(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![("title".into(), saved("Mort"))],
        },
    );
    assert_eq!(
        decode_value(
            store.read(&field_path(1, "title")).expect("title"),
            ValueType::Str
        ),
        Some(SavedValue::Str("Mort".into()))
    );
    assert_eq!(
        store.read(&field_path(1, "shelf")),
        None,
        "sparse field stays absent"
    );
}

#[test]
fn an_explicit_absent_in_a_merge_leaves_the_target_field() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    // An explicit Absent means "not contributed", same as omission: leave it.
    merge(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![("shelf".into(), FieldValue::Absent)],
        },
    );
    assert_eq!(
        decode_value(
            store.read(&field_path(1, "shelf")).expect("shelf"),
            ValueType::Str
        ),
        Some(SavedValue::Str("fiction".into())),
        "merge does not clear a field"
    );
}

#[test]
fn a_merge_with_a_mismatched_type_is_rejected() {
    let book = schema(BOOK);
    let value = ResourceValue {
        fields: vec![("title".into(), FieldValue::Saved(SavedValue::Int(5)))],
    };
    let result = plan_resource_merge(&book, &[SavedKey::Int(1)], &value, &MemStore::new());
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_TYPE_MISMATCH),
        "{result:?}"
    );
}

#[test]
fn a_merge_omitting_a_required_field_already_stored_succeeds() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    // `title` (required) is not supplied, but it is already stored.
    merge(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![("shelf".into(), saved("history"))],
        },
    );
    assert_eq!(
        decode_value(
            store.read(&field_path(1, "title")).expect("title"),
            ValueType::Str
        ),
        Some(SavedValue::Str("Mort".into()))
    );
}

#[test]
fn a_merge_omitting_a_required_field_not_yet_stored_is_rejected() {
    let book = schema(BOOK);
    // Empty store: `title` (required) is neither supplied nor stored.
    let value = ResourceValue {
        fields: vec![("shelf".into(), saved("fiction"))],
    };
    let store = MemStore::new();
    let result = plan_resource_merge(&book, &[SavedKey::Int(1)], &value, &store);
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_REQUIRED_ABSENT),
        "{result:?}"
    );
}

#[test]
fn merging_an_indexed_field_moves_the_index_entry() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    merge(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![("shelf".into(), saved("history"))],
        },
    );
    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        None,
        "old entry gone"
    );
    assert_eq!(
        store.read(&by_shelf_entry("history", 42)),
        Some(&b"1"[..]),
        "new entry present"
    );
}

#[test]
fn merging_a_non_indexed_field_preserves_the_index_entry() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![
                ("title".into(), saved("Mort")),
                ("shelf".into(), saved("fiction")),
            ],
        },
    );
    // The merge touches `title`, not the indexed `shelf`: the entry — whose key
    // rests on the untouched `shelf` — must survive, not be torn down.
    merge(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![("title".into(), saved("Mort the Second"))],
        },
    );
    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        Some(&b"1"[..]),
        "untouched index entry preserved"
    );
}

#[test]
fn a_merge_onto_a_unique_key_held_by_another_is_rejected() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    write(
        &mut store,
        &book,
        &[SavedKey::Int(2)],
        book_with_isbn("B", "Y"),
    );

    // Record 2 merging onto record 1's isbn is rejected.
    let value = ResourceValue {
        fields: vec![("isbn".into(), saved("X"))],
    };
    let result = plan_resource_merge(&book, &[SavedKey::Int(2)], &value, &store);
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNIQUE_CONFLICT),
        "{result:?}"
    );
    assert_owns(&store, &by_isbn_entry("Y"), 2);
}

#[test]
fn merging_a_unique_field_moves_the_entry() {
    let book = schema(BOOK_UNIQUE);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        book_with_isbn("A", "X"),
    );
    // Merge a new isbn over the same record: the unique entry moves.
    merge(
        &mut store,
        &book,
        &[SavedKey::Int(1)],
        ResourceValue {
            fields: vec![("isbn".into(), saved("Z"))],
        },
    );
    assert_eq!(
        store.read(&by_isbn_entry("X")),
        None,
        "old unique key released"
    );
    assert_owns(&store, &by_isbn_entry("Z"), 1);
}

#[test]
fn merging_an_indexed_field_into_a_record_without_it_creates_the_entry() {
    let book = schema(BOOK_INDEXED);
    let mut store = MemStore::new();
    // Seed without the indexed `shelf`, so no byShelf entry exists yet.
    write(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![("title".into(), saved("Mort"))],
        },
    );
    merge(
        &mut store,
        &book,
        &[SavedKey::Int(42)],
        ResourceValue {
            fields: vec![("shelf".into(), saved("fiction"))],
        },
    );
    assert_eq!(
        store.read(&by_shelf_entry("fiction", 42)),
        Some(&b"1"[..]),
        "entry created when the merge first populates the indexed field"
    );
}

#[test]
fn a_keyed_leaf_write_lowers_a_value_into_the_store() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write_tag(&mut store, &book, 5, 1, "favorite");
    assert_eq!(
        decode_value(
            store.read(&tag_entry(5, 1)).expect("tag entry"),
            ValueType::Str
        ),
        Some(SavedValue::Str("favorite".into()))
    );
}

#[test]
fn a_keyed_leaf_write_with_a_mismatched_value_type_is_rejected() {
    let book = schema(BOOK_LAYERS);
    // `tags` holds strings; an int does not satisfy the leaf type.
    let result = plan_layer_leaf_write(
        &book,
        &[SavedKey::Int(5)],
        "tags",
        &[SavedKey::Int(1)],
        &SavedValue::Int(7),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_TYPE_MISMATCH),
        "{result:?}"
    );
}

#[test]
fn a_keyed_leaf_write_to_an_unknown_layer_is_rejected() {
    let book = schema(BOOK_LAYERS);
    let result = plan_layer_leaf_write(
        &book,
        &[SavedKey::Int(5)],
        "chapters",
        &[SavedKey::Int(1)],
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNKNOWN_LAYER),
        "{result:?}"
    );
}

#[test]
fn writing_a_group_layer_through_the_leaf_planner_is_rejected() {
    let book = schema(BOOK_LAYERS);
    // `notes` is a group (nested members), not a keyed leaf.
    let result = plan_layer_leaf_write(
        &book,
        &[SavedKey::Int(5)],
        "notes",
        &[SavedKey::Str("n1".into())],
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_NOT_A_LEAF_LAYER),
        "{result:?}"
    );
}

#[test]
fn a_keyed_leaf_write_with_the_wrong_key_arity_is_rejected() {
    let book = schema(BOOK_LAYERS);
    // `tags` takes one key; supplying two is rejected.
    let result = plan_layer_leaf_write(
        &book,
        &[SavedKey::Int(5)],
        "tags",
        &[SavedKey::Int(1), SavedKey::Int(2)],
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_LAYER_KEY_ARITY),
        "{result:?}"
    );
}

#[test]
fn a_keyed_leaf_write_replaces_only_that_entry() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write_tag(&mut store, &book, 5, 1, "a");
    write_tag(&mut store, &book, 5, 2, "b");
    write_tag(&mut store, &book, 5, 1, "c");

    assert_eq!(
        decode_value(store.read(&tag_entry(5, 1)).expect("tag 1"), ValueType::Str),
        Some(SavedValue::Str("c".into())),
        "the keyed entry is replaced in place"
    );
    assert_eq!(
        decode_value(store.read(&tag_entry(5, 2)).expect("tag 2"), ValueType::Str),
        Some(SavedValue::Str("b".into())),
        "a sibling entry is untouched"
    );
}

#[test]
fn a_keyed_leaf_write_leaves_top_level_fields_untouched() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write(
        &mut store,
        &book,
        &[SavedKey::Int(5)],
        ResourceValue {
            fields: vec![("title".into(), saved("Mort"))],
        },
    );
    write_tag(&mut store, &book, 5, 1, "favorite");

    assert_eq!(
        decode_value(
            store.read(&field_path(5, "title")).expect("title"),
            ValueType::Str
        ),
        Some(SavedValue::Str("Mort".into())),
        "the top-level field is left in place"
    );
    assert!(
        store.read(&tag_entry(5, 1)).is_some(),
        "the tag entry is present"
    );
}

#[test]
fn a_keyed_leaf_write_to_a_resource_without_a_saved_root_is_rejected() {
    let book = schema(LOCAL_BOOK_LAYERS);
    let result = plan_layer_leaf_write(
        &book,
        &[SavedKey::Int(5)],
        "tags",
        &[SavedKey::Int(1)],
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_NO_SAVED_ROOT),
        "{result:?}"
    );
}

#[test]
fn next_layer_pos_starts_at_one_when_empty() {
    let book = schema(BOOK_LAYERS);
    let store = MemStore::new();
    assert_eq!(
        next_layer_pos(&book, &[SavedKey::Int(5)], "tags", &store),
        Ok(1)
    );
}

#[test]
fn next_layer_pos_is_one_past_the_highest_and_does_not_fill_holes() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write_tag(&mut store, &book, 5, 1, "a");
    write_tag(&mut store, &book, 5, 3, "c"); // leaves a hole at 2
    assert_eq!(
        next_layer_pos(&book, &[SavedKey::Int(5)], "tags", &store),
        Ok(4),
        "after the highest, not filling the hole"
    );
}

#[test]
fn next_layer_pos_is_per_record() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write_tag(&mut store, &book, 5, 1, "a");
    write_tag(&mut store, &book, 5, 2, "b");
    // A different record's layer starts fresh; record 5's continues past its own.
    assert_eq!(
        next_layer_pos(&book, &[SavedKey::Int(9)], "tags", &store),
        Ok(1)
    );
    assert_eq!(
        next_layer_pos(&book, &[SavedKey::Int(5)], "tags", &store),
        Ok(3)
    );
}

#[test]
fn next_layer_pos_for_an_unknown_layer_is_rejected() {
    let book = schema(BOOK_LAYERS);
    let store = MemStore::new();
    let result = next_layer_pos(&book, &[SavedKey::Int(5)], "chapters", &store);
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNKNOWN_LAYER),
        "{result:?}"
    );
}

#[test]
fn a_group_entry_field_write_lowers_a_value_into_the_store() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write_note(&mut store, &book, 5, "n1", "first thought");
    assert_eq!(
        decode_value(
            store.read(&note_text_entry(5, "n1")).expect("note text"),
            ValueType::Str
        ),
        Some(SavedValue::Str("first thought".into()))
    );
}

#[test]
fn a_group_entry_field_write_touches_only_that_member() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    // `versions` is a multi-member group; writing one member must not clear its
    // siblings in the same entry.
    plan_layer_field_write(
        &book,
        &[SavedKey::Int(5)],
        "versions",
        &[SavedKey::Int(1)],
        "title",
        &SavedValue::Str("Mort".into()),
    )
    .expect("title write")
    .commit(&mut store)
    .expect("commit succeeds");
    plan_layer_field_write(
        &book,
        &[SavedKey::Int(5)],
        "versions",
        &[SavedKey::Int(1)],
        "shelf",
        &SavedValue::Str("fiction".into()),
    )
    .expect("shelf write")
    .commit(&mut store)
    .expect("commit succeeds");

    assert_eq!(
        decode_value(
            store.read(&version_field(5, 1, "title")).expect("title"),
            ValueType::Str
        ),
        Some(SavedValue::Str("Mort".into())),
        "the first member is kept"
    );
    assert_eq!(
        decode_value(
            store.read(&version_field(5, 1, "shelf")).expect("shelf"),
            ValueType::Str
        ),
        Some(SavedValue::Str("fiction".into())),
        "the second member lands alongside it"
    );
}

#[test]
fn a_group_entry_field_write_replaces_only_that_entry() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write_note(&mut store, &book, 5, "n1", "a");
    write_note(&mut store, &book, 5, "n2", "b");
    write_note(&mut store, &book, 5, "n1", "c");
    assert_eq!(
        decode_value(
            store.read(&note_text_entry(5, "n1")).expect("note n1"),
            ValueType::Str
        ),
        Some(SavedValue::Str("c".into())),
        "the entry is replaced in place"
    );
    assert_eq!(
        decode_value(
            store.read(&note_text_entry(5, "n2")).expect("note n2"),
            ValueType::Str
        ),
        Some(SavedValue::Str("b".into())),
        "a sibling entry is untouched"
    );
}

#[test]
fn a_group_entry_field_write_to_an_unknown_member_is_rejected() {
    let book = schema(BOOK_LAYERS);
    let result = plan_layer_field_write(
        &book,
        &[SavedKey::Int(5)],
        "notes",
        &[SavedKey::Str("n1".into())],
        "bogus",
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNKNOWN_FIELD),
        "{result:?}"
    );
}

#[test]
fn a_group_entry_field_write_to_a_leaf_layer_is_rejected() {
    let book = schema(BOOK_LAYERS);
    // `tags` is a keyed leaf, not a group, so it has no member fields to write.
    let result = plan_layer_field_write(
        &book,
        &[SavedKey::Int(5)],
        "tags",
        &[SavedKey::Int(1)],
        "text",
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_NOT_A_GROUP_LAYER),
        "{result:?}"
    );
}

#[test]
fn a_group_entry_field_write_to_an_unknown_layer_is_rejected() {
    let book = schema(BOOK_LAYERS);
    let result = plan_layer_field_write(
        &book,
        &[SavedKey::Int(5)],
        "chapters",
        &[SavedKey::Str("c1".into())],
        "text",
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNKNOWN_LAYER),
        "{result:?}"
    );
}

#[test]
fn a_group_entry_field_write_with_the_wrong_key_arity_is_rejected() {
    let book = schema(BOOK_LAYERS);
    // `notes` takes one key; supplying two is rejected.
    let result = plan_layer_field_write(
        &book,
        &[SavedKey::Int(5)],
        "notes",
        &[SavedKey::Str("n1".into()), SavedKey::Str("n2".into())],
        "text",
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_LAYER_KEY_ARITY),
        "{result:?}"
    );
}

#[test]
fn a_group_entry_field_write_with_a_mismatched_value_type_is_rejected() {
    let book = schema(BOOK_LAYERS);
    // `notes.text` holds strings; an int does not satisfy the member type.
    let result = plan_layer_field_write(
        &book,
        &[SavedKey::Int(5)],
        "notes",
        &[SavedKey::Str("n1".into())],
        "text",
        &SavedValue::Int(7),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_TYPE_MISMATCH),
        "{result:?}"
    );
}

#[test]
fn a_group_entry_field_write_to_a_resource_without_a_saved_root_is_rejected() {
    let book = schema(LOCAL_BOOK_LAYERS);
    let result = plan_layer_field_write(
        &book,
        &[SavedKey::Int(5)],
        "notes",
        &[SavedKey::Str("n1".into())],
        "text",
        &SavedValue::Str("x".into()),
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_NO_SAVED_ROOT),
        "{result:?}"
    );
}

/// Plan and commit a keyed-layer merge of `^books(from).tags` into
/// `^books(to).tags`.
fn merge_tags(store: &mut MemStore, schema: &ResourceSchema, from: i64, to: i64) {
    plan_layer_merge(
        schema,
        &[SavedKey::Int(from)],
        &[SavedKey::Int(to)],
        "tags",
        store,
    )
    .expect("valid layer merge")
    .commit(store)
    .expect("commit succeeds");
}

#[test]
fn a_layer_merge_copies_entries_to_the_target_record() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write_tag(&mut store, &book, 1, 1, "favorite");
    write_tag(&mut store, &book, 1, 2, "gift");
    merge_tags(&mut store, &book, 1, 2);
    assert_eq!(
        decode_value(store.read(&tag_entry(2, 1)).expect("tag 1"), ValueType::Str),
        Some(SavedValue::Str("favorite".into()))
    );
    assert_eq!(
        decode_value(store.read(&tag_entry(2, 2)).expect("tag 2"), ValueType::Str),
        Some(SavedValue::Str("gift".into()))
    );
}

#[test]
fn a_layer_merge_overlays_and_keeps_uncovered_target_entries() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    // Source has pos 1; target has pos 1 (overwritten) and pos 2 (kept).
    write_tag(&mut store, &book, 1, 1, "from-source");
    write_tag(&mut store, &book, 2, 1, "old");
    write_tag(&mut store, &book, 2, 2, "kept");
    merge_tags(&mut store, &book, 1, 2);
    assert_eq!(
        decode_value(store.read(&tag_entry(2, 1)).expect("tag 1"), ValueType::Str),
        Some(SavedValue::Str("from-source".into())),
        "an overlapping key is overwritten by the source"
    );
    assert_eq!(
        decode_value(store.read(&tag_entry(2, 2)).expect("tag 2"), ValueType::Str),
        Some(SavedValue::Str("kept".into())),
        "a target entry the source does not cover is kept"
    );
}

#[test]
fn a_layer_merge_from_an_empty_source_is_a_no_op() {
    let book = schema(BOOK_LAYERS);
    let mut store = MemStore::new();
    write_tag(&mut store, &book, 2, 1, "kept"); // record 1 has no tags
    merge_tags(&mut store, &book, 1, 2);
    assert_eq!(
        decode_value(store.read(&tag_entry(2, 1)).expect("tag 1"), ValueType::Str),
        Some(SavedValue::Str("kept".into())),
        "merging an empty source changes nothing"
    );
}

#[test]
fn a_layer_merge_of_an_unknown_layer_is_rejected() {
    let book = schema(BOOK_LAYERS);
    let store = MemStore::new();
    let result = plan_layer_merge(
        &book,
        &[SavedKey::Int(1)],
        &[SavedKey::Int(2)],
        "chapters",
        &store,
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_UNKNOWN_LAYER),
        "{result:?}"
    );
}

#[test]
fn a_layer_merge_with_a_mismatched_target_identity_is_rejected() {
    let book = schema(BOOK_LAYERS);
    let store = MemStore::new();
    let result = plan_layer_merge(
        &book,
        &[SavedKey::Int(1)],
        &[SavedKey::Int(2), SavedKey::Int(3)],
        "tags",
        &store,
    );
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_IDENTITY_MISMATCH),
        "{result:?}"
    );
}
