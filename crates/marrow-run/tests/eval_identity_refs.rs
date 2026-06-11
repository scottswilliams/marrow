//! Typed identity reference fields (`Id(^store)`): canonical key-encoded storage,
//! round-trip through field and whole-record writes, self-reference, and value
//! equality across identity origins.

#[macro_use]
mod support;

use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::Value;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::TreeStore;

/// A program with an `Author` resource and a `Book` whose `authorId` is a typed
/// reference to `Author`. `seed` writes a reference; `read` reads it back.
fn typed_ref_program() -> CheckedRuntimeProgram {
    checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   ^books(1).authorId = author\n\
         \n\
         pub fn read(): bool\n\
         \x20   for author in keys(^authors)\n\
         \x20       const stored: Id(^authors) = ^books(1).authorId ?? author\n\
         \x20       return stored == author\n\
         \x20   return false\n",
    )
}

#[test]
fn an_identity_field_round_trips_through_saved_data() {
    // A `Book.authorId: Id(^authors)` field stores an identity and reads it back as
    // the same identity value produced by the author store.
    let program = typed_ref_program();
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn a_stored_identity_field_reads_back_the_identity_value() {
    // The stored leaf carries the referenced identity's key segments, not a plain
    // scalar field encoding.
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   ^books(1).authorId = author\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    // The stored leaf is the canonical identity encoding — the same
    // order-preserving key bytes a unique index entry stores — not a scalar
    // `encode_value`.
    let stored = read_data_bytes(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["authorId"]),
    );
    assert_eq!(
        stored,
        Some(encode_identity_payload(&[SavedKey::Int(1)])),
        "identity field stored as its canonical key encoding"
    );
}

#[test]
fn a_type_wrong_identity_field_does_not_decode_as_an_identity_value() {
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn read(): bool\n\
         \x20   const fallback = Id(^authors, 7)\n\
         \x20   const stored: Id(^authors) = ^books(1).authorId ?? fallback\n\
         \x20   return stored == fallback\n",
    );
    let store = TreeStore::memory();
    let path = data_path(&program, "books", &["authorId"]);
    store
        .write_node(&store_catalog_id(&program, "books"), &[SavedKey::Int(1)])
        .expect("write book node");
    store
        .write_data_value(
            &store_catalog_id(&program, "books"),
            &[SavedKey::Int(1)],
            &path,
            encode_identity_payload(&[SavedKey::Str("not-an-int".to_string())]),
        )
        .expect("write corrupt identity leaf");

    assert!(
        run_entry(&store, checked_entry!(&program, "test::read")).is_err(),
        "type-wrong identity bytes must not construct a runtime identity"
    );
}

#[test]
fn an_identity_field_assigned_via_next_id_round_trips() {
    // Constructing the reference from `nextId(^authors)` (the first allocated id is
    // `1` on an empty root) round-trips through the saved identity field.
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   const a = nextId(^authors)\n\
         \x20   ^authors(a).name = \"Ada\"\n\
         \x20   ^books(1).authorId = a\n\
         \n\
         pub fn read(): bool\n\
         \x20   for author in keys(^authors)\n\
         \x20       const stored: Id(^authors) = ^books(1).authorId ?? author\n\
         \x20       return stored == author\n\
         \x20   return false\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn a_self_referencing_identity_field_round_trips() {
    // A field of the same resource (`managerId: Id(^people)` on `Person`) is a valid
    // self-reference that stores and reads back like any other typed reference.
    let program = checked_program(
        "resource Person\n\
         \x20   managerId: Id(^people)\nstore ^people(id: int): Person\n\
         \n\
         pub fn seed(): bool\n\
         \x20   const person = nextId(^people)\n\
         \x20   ^people(person).managerId = person\n\
         \x20   const manager = nextId(^people)\n\
         \x20   ^people(person).managerId = manager\n\
         \x20   return read(manager)\n\
         \n\
         pub fn read(expected: Id(^people)): bool\n\
         \x20   const stored: Id(^people) = ^people(1).managerId ?? expected\n\
         \x20   return stored == expected\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::seed"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn equality_on_two_identities_of_the_same_store_evaluates() {
    // `==` on two identities of the same store is value equality of their keys:
    // equal keys are `true`, differing keys are `false`.
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         pub fn same(): bool\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   return author == author\n\
         \n\
         pub fn different(): bool\n\
         \x20   const ada = nextId(^authors)\n\
         \x20   ^authors(ada).name = \"Ada\"\n\
         \x20   const grace = nextId(^authors)\n\
         \x20   ^authors(grace).name = \"Grace\"\n\
         \x20   return ada == grace\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::same")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::different")).unwrap(),
        Some(Value::Bool(false))
    );
}

#[test]
fn single_key_store_identity_behaves_like_other_identity_origins() {
    let program = checked_program(
        "resource Doc\n\
         \x20   title: string\nstore ^docs(id: int): Doc\n\
         \n\
         pub fn idValue(): Id(^docs)\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   for doc in keys(^docs)\n\
         \x20       return doc\n\
         \x20   return id\n\
         \n\
         pub fn mixedEq(): bool\n\
         \x20   const id = nextId(^docs)\n\
         \x20   ^docs(id).title = \"T\"\n\
         \x20   for doc in keys(^docs)\n\
         \x20       return id == doc\n\
         \x20   return false\n",
    );
    assert_identity_value(
        run(checked_entry!(&program, "test::idValue")).unwrap(),
        "docs",
        &[SavedKey::Int(1)],
    );
    assert_eq!(
        run(checked_entry!(&program, "test::mixedEq")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn explicit_single_key_identity_constructor_reads_and_writes_records() {
    let program = checked_program(
        "resource Book\n\
         \x20   title: string\n\
         store ^books(id: string): Book\n\
         \n\
         pub fn make(): Id(^books)\n\
         \x20   return Id(^books, \"book-17\")\n\
         \n\
         pub fn seed()\n\
         \x20   const id = Id(^books, \"book-17\")\n\
         \x20   ^books(id).title = \"Mort\"\n\
         \n\
         pub fn read(): string\n\
         \x20   return ^books(Id(^books, \"book-17\")).title ?? \"missing\"\n",
    );
    let store = TreeStore::memory();
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::make"))
            .expect("make runs")
            .value,
        "books",
        &[SavedKey::Str("book-17".to_string())],
    );
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read runs")
            .value,
        Some(Value::Str("Mort".to_string()))
    );
}

#[test]
fn explicit_composite_identity_constructor_addresses_records() {
    let program = checked_program(
        "resource Enrollment\n\
         \x20   grade: string\n\
         store ^enrollments(student: string, course: string): Enrollment\n\
         \n\
         pub fn make(): Id(^enrollments)\n\
         \x20   return Id(^enrollments, \"student-1\", \"course-9\")\n\
         \n\
         pub fn seed()\n\
         \x20   const id = Id(^enrollments, \"student-1\", \"course-9\")\n\
         \x20   ^enrollments(id).grade = \"A\"\n\
         \n\
         pub fn read(): string\n\
         \x20   return ^enrollments(Id(^enrollments, \"student-1\", \"course-9\")).grade ?? \"missing\"\n",
    );
    let store = TreeStore::memory();
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::make"))
            .expect("make runs")
            .value,
        "enrollments",
        &[
            SavedKey::Str("student-1".to_string()),
            SavedKey::Str("course-9".to_string()),
        ],
    );
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read runs")
            .value,
        Some(Value::Str("A".to_string()))
    );
}

#[test]
fn unique_index_identity_compares_with_the_allocated_identity() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   required isbn: string\nstore ^books(id: int): Book\n\
         \x20   index byIsbn(isbn) unique\n\
         \n\
         pub fn seed(): bool\n\
         \x20   var b: Book\n\
         \x20   b.title = \"T\"\n\
         \x20   b.isbn = \"I-1\"\n\
         \x20   const id = nextId(^books)\n\
         \x20   ^books(id) = b\n\
         \x20   const found: Id(^books) = ^books.byIsbn(\"I-1\") ?? id\n\
         \x20   return id == found\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seed")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn a_whole_resource_write_with_an_identity_field_round_trips() {
    // A whole-record write `^books(1) = b` carrying an identity-typed field stores
    // the reference, and a whole-record read reads it back.
    let program = checked_program(
        "resource Author\n\
         \x20   name: string\nstore ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   const author = nextId(^authors)\n\
         \x20   ^authors(author).name = \"Ada\"\n\
         \x20   var b: Book\n\
         \x20   b.title = \"Mort\"\n\
         \x20   b.authorId = author\n\
         \x20   ^books(1) = b\n\
         \n\
         pub fn read(): bool\n\
         \x20   if exists(^books(1))\n\
         \x20       const b = ^books(1)\n\
         \x20       for author in keys(^authors)\n\
         \x20           return b.authorId == author\n\
         \x20   return false\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}
