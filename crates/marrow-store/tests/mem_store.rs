//! In-memory store behavior: write/read round-trips, the four presence states,
//! and subtree delete. These are the first store-conformance laws.

use marrow_store::mem::{MemStore, Presence};
use marrow_store::path::{PathSegment, SavedKey};

/// The path `^books(id)`.
fn book(id: i64) -> Vec<PathSegment> {
    vec![
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
    ]
}

/// The path `^books(id).<field>`.
fn book_field(id: i64, field: &str) -> Vec<PathSegment> {
    let mut path = book(id);
    path.push(PathSegment::Field(field.into()));
    path
}

#[test]
fn write_then_read_returns_the_value() {
    let mut store = MemStore::new();
    store.write(&book_field(1, "title"), b"Dune".to_vec());
    assert_eq!(store.read(&book_field(1, "title")), Some(&b"Dune"[..]));
}

#[test]
fn reading_an_absent_path_returns_none() {
    let store = MemStore::new();
    assert_eq!(store.read(&book_field(1, "title")), None);
}

#[test]
fn writing_replaces_the_existing_value() {
    let mut store = MemStore::new();
    store.write(&book_field(1, "title"), b"draft".to_vec());
    store.write(&book_field(1, "title"), b"final".to_vec());
    assert_eq!(store.read(&book_field(1, "title")), Some(&b"final"[..]));
}

#[test]
fn presence_reports_all_four_states() {
    let mut store = MemStore::new();
    assert_eq!(store.presence(&book(1)), Presence::Absent);

    // A whole-resource value at the record, no children yet.
    store.write(&book(1), b"whole".to_vec());
    assert_eq!(store.presence(&book(1)), Presence::ValueOnly);

    // A field below the record adds children.
    store.write(&book_field(1, "title"), b"Dune".to_vec());
    assert_eq!(store.presence(&book(1)), Presence::ValueAndChildren);

    // A different record with only a field below it, no whole value.
    store.write(&book_field(2, "title"), b"Sand".to_vec());
    assert_eq!(store.presence(&book(2)), Presence::ChildrenOnly);
}

#[test]
fn delete_removes_the_value_and_its_subtree() {
    let mut store = MemStore::new();
    store.write(&book(1), b"whole".to_vec());
    store.write(&book_field(1, "title"), b"Dune".to_vec());
    store.write(&book_field(1, "author"), b"Herbert".to_vec());
    store.write(&book_field(2, "title"), b"Other".to_vec());

    store.delete(&book(1));

    assert_eq!(store.presence(&book(1)), Presence::Absent);
    assert_eq!(store.read(&book_field(1, "title")), None);
    assert_eq!(store.read(&book_field(1, "author")), None);
    // A sibling record is untouched.
    assert_eq!(store.read(&book_field(2, "title")), Some(&b"Other"[..]));
}

#[test]
fn deleting_an_absent_path_is_a_no_op() {
    let mut store = MemStore::new();
    store.write(&book_field(2, "title"), b"Other".to_vec());
    store.delete(&book(1));
    assert_eq!(store.read(&book_field(2, "title")), Some(&b"Other"[..]));
}
