//! Managed whole-resource writes: validate against the schema, then lower the
//! fields into the store.

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

#[test]
fn a_whole_resource_write_lowers_fields_into_the_store() {
    let book = schema(BOOK);
    let value = ResourceValue {
        fields: vec![
            ("title".into(), saved("Small Gods")),
            ("shelf".into(), saved("fiction")),
        ],
    };

    let plan = plan_resource_write(&book, &[SavedKey::Int(42)], &value).expect("valid write");
    let mut store = MemStore::new();
    plan.commit(&mut store);

    for (field, expected) in [("title", "Small Gods"), ("shelf", "fiction")] {
        let path = encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(42)),
            PathSegment::Field(field.into()),
        ]);
        let bytes = store
            .read(&path)
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
    let value = ResourceValue {
        fields: vec![("title".into(), saved("Mort"))],
    };
    let plan = plan_resource_write(&book, &[SavedKey::Int(7)], &value).expect("valid write");
    let mut store = MemStore::new();
    plan.commit(&mut store);

    let shelf = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(7)),
        PathSegment::Field("shelf".into()),
    ]);
    assert_eq!(
        store.read(&shelf),
        None,
        "omitted sparse field is not stored"
    );
}

#[test]
fn a_missing_required_field_is_rejected_with_no_write() {
    let book = schema(BOOK);
    let value = ResourceValue {
        fields: vec![("shelf".into(), saved("fiction"))],
    };
    let result = plan_resource_write(&book, &[SavedKey::Int(42)], &value);
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
    let result = plan_resource_write(&book, &[SavedKey::Int(42)], &value);
    assert!(
        matches!(result, Err(ref error) if error.code == WRITE_TYPE_MISMATCH),
        "{result:?}"
    );
}

#[test]
fn a_whole_resource_write_replaces_the_previous_value() {
    let book = schema(BOOK);
    let mut store = MemStore::new();

    // First write has both fields.
    plan_resource_write(
        &book,
        &[SavedKey::Int(1)],
        &ResourceValue {
            fields: vec![
                ("title".into(), saved("draft")),
                ("shelf".into(), saved("new")),
            ],
        },
    )
    .expect("first write")
    .commit(&mut store);

    // Second write omits the sparse field; replace semantics remove it.
    plan_resource_write(
        &book,
        &[SavedKey::Int(1)],
        &ResourceValue {
            fields: vec![("title".into(), saved("final"))],
        },
    )
    .expect("second write")
    .commit(&mut store);

    let path = |field: &str| {
        encode_path(&[
            PathSegment::Root("books".into()),
            PathSegment::RecordKey(SavedKey::Int(1)),
            PathSegment::Field(field.into()),
        ])
    };
    assert_eq!(
        decode_value(store.read(&path("title")).expect("title"), ValueType::Str),
        Some(SavedValue::Str("final".into()))
    );
    assert_eq!(store.read(&path("shelf")), None, "replaced away");
}

#[test]
fn next_id_allocates_after_the_highest_record() {
    let book = schema(BOOK);
    let mut store = MemStore::new();
    // An empty root allocates 1.
    assert_eq!(next_id("books", &store), Ok(1));

    // Write records 5 and 1 (out of order); the next id is one past the highest.
    for id in [5, 1] {
        plan_resource_write(
            &book,
            &[SavedKey::Int(id)],
            &ResourceValue {
                fields: vec![("title".into(), saved("t"))],
            },
        )
        .expect("write")
        .commit(&mut store);
    }
    assert_eq!(next_id("books", &store), Ok(6));
}

#[test]
fn a_write_emits_non_unique_index_entries() {
    let book = schema(BOOK_INDEXED);
    let value = ResourceValue {
        fields: vec![
            ("title".into(), saved("Mort")),
            ("shelf".into(), saved("fiction")),
        ],
    };
    let mut store = MemStore::new();
    plan_resource_write(&book, &[SavedKey::Int(42)], &value)
        .expect("write")
        .commit(&mut store);

    // ^books.byShelf("fiction", 42) marks the entry.
    let entry = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Index("byShelf".into()),
        PathSegment::IndexKey(SavedKey::Str("fiction".into())),
        PathSegment::IndexKey(SavedKey::Int(42)),
    ]);
    assert_eq!(store.read(&entry), Some(&b"1"[..]), "index entry present");
}

#[test]
fn no_index_entry_when_an_indexed_field_is_absent() {
    let book = schema(BOOK_INDEXED);
    // `shelf` (an index argument) is omitted, so the byShelf entry is not written.
    let value = ResourceValue {
        fields: vec![("title".into(), saved("Mort"))],
    };
    let mut store = MemStore::new();
    plan_resource_write(&book, &[SavedKey::Int(42)], &value)
        .expect("write")
        .commit(&mut store);

    let by_shelf = encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::Index("byShelf".into()),
    ]);
    assert_eq!(store.scan(&by_shelf, usize::MAX).entries.len(), 0);
}
