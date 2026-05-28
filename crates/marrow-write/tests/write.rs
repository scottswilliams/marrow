//! Managed whole-resource writes: validate against the schema, lower the fields
//! into the store, and keep generated index entries coherent.

use marrow_schema::{ResourceSchema, compile_resource};
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, decode_key_value, encode_path};
use marrow_store::value::{SavedValue, ValueType, decode_value};
use marrow_syntax::{Declaration, parse_source};
use marrow_write::{
    FieldValue, ResourceValue, WRITE_REQUIRED_ABSENT, WRITE_TYPE_MISMATCH, WRITE_UNIQUE_CONFLICT,
    WRITE_UNKNOWN_FIELD, next_id, plan_field_write, plan_resource_delete, plan_resource_write,
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
        .commit(store);
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
        .commit(store);
}

/// The encoded path `^books(id).field`.
fn field_path(id: i64, field: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::Field(field.into()),
    ])
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
    plan.commit(&mut store);

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
    .commit(&mut store);
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
        .commit(&mut store);
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
