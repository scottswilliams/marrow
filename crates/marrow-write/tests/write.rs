//! Managed whole-resource writes: validate against the schema, lower the fields
//! into the store, and keep generated index entries coherent.

use marrow_schema::{ResourceSchema, compile_resource};
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, encode_path};
use marrow_store::value::{SavedValue, ValueType, decode_value};
use marrow_syntax::{Declaration, parse_source};
use marrow_write::{
    FieldValue, ResourceValue, WRITE_REQUIRED_ABSENT, WRITE_TYPE_MISMATCH, next_id,
    plan_resource_write,
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
