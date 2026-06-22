//! Whole-resource values: reading, constructing (including qualified and nested
//! local-resource fields), saving, copying, and materializing unkeyed groups.

use crate::support;
use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};

/// A program that reads, copies, and reads back whole `Book` resources.
const BOOK_COPY: &str = "\
resource Book
    required title: string
    required shelf: string
store ^books(id: int): Book

pub fn read(id: int): Book
    var fallback: Book
    fallback.title = \"\"
    fallback.shelf = \"\"
    return ^books(id) ?? fallback

pub fn copy(from: int, to: int)
    var fallback: Book
    fallback.title = \"\"
    fallback.shelf = \"\"
    ^books(to) = ^books(from) ?? fallback

pub fn title_of(id: int): string
    return ^books(id).title ?? \"\"

pub fn shelf_of(id: int): string
    return ^books(id).shelf ?? \"\"
";

fn seed_field(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    field: &str,
    value: &str,
) {
    write_data_value(
        program,
        store,
        "books",
        &[SavedKey::Int(id)],
        &data_path(program, "books", &[field]),
        SavedValue::Str(value.into()),
    );
}

#[test]
fn reads_a_whole_resource() {
    let program = checked_program(BOOK_COPY);
    let store = TreeStore::memory();
    seed_field(&program, &store, 1, "title", "Mort");
    seed_field(&program, &store, 1, "shelf", "fiction");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::read", Value::Int(1)),
    )
    .expect("read");
    // Present fields, in schema order.
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("title".into(), Value::Str("Mort".into())),
            ("shelf".into(), Value::Str("fiction".into())),
        ]))
    );
}

#[test]
fn constructs_a_resource_value() {
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20shelf: string\nstore ^books(id: int): Book\n\n\
         pub fn draft(): Book\n\
         \x20\x20\x20\x20return Book(title: \"Mort\", shelf: \"fiction\")\n",
    );
    let store = TreeStore::memory();
    let outcome = run_entry(&store, checked_entry!(&program, "test::draft")).expect("draft");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("title".into(), Value::Str("Mort".into())),
            ("shelf".into(), Value::Str("fiction".into())),
        ]))
    );
}

#[test]
fn constructs_a_resource_value_with_a_local_resource_field() {
    let program = checked_program(
        "resource Address\n\
         \x20\x20\x20\x20city: string\n\n\
         resource Person\n\
         \x20\x20\x20\x20required name: string\n\
         \x20\x20\x20\x20address: Address\n\n\
         pub fn city(): string\n\
         \x20\x20\x20\x20const person = Person(name: \"Sam\", address: Address(city: \"Paris\"))\n\
         \x20\x20\x20\x20if const addr = person.address\n\
         \x20\x20\x20\x20\x20\x20\x20\x20return addr.city ?? \"\"\n\
         \x20\x20\x20\x20return \"\"\n",
    );
    let store = TreeStore::memory();
    let outcome = run_entry(&store, checked_entry!(&program, "test::city")).expect("city");
    assert_eq!(outcome.value, Some(Value::Str("Paris".into())));
}

#[test]
fn constructs_a_qualified_resource_value() {
    let program = checked_program_modules(&[
        "module library\n\
         resource Book\n\
         \x20\x20\x20\x20title: string\n",
        "module app\n\
         use library\n\
         pub fn draft(): unknown\n\
         \x20\x20\x20\x20return library::Book(title: \"Mort\")\n",
    ]);
    let store = TreeStore::memory();
    let outcome = run_entry(&store, checked_entry!(&program, "app::draft")).expect("draft");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![(
            "title".into(),
            Value::Str("Mort".into())
        )]))
    );
}

#[test]
fn constructor_field_with_qualified_resource_type_rejects_scalar() {
    checker_rejects_sources(
        &[
            "module library\n\
             resource Address\n\
             \x20\x20\x20\x20city: string\n",
            "module app\n\
             use library\n\
             resource Person\n\
             \x20\x20\x20\x20address: library::Address\n\
             fn make(): unknown\n\
             \x20\x20\x20\x20return Person(address: 1)\n",
        ],
        "check.call_argument",
    );
}

#[test]
fn resource_constructor_value_can_be_saved() {
    let program = checked_program(
        "resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20author: string\nstore ^books(id: int): Book\n\n\
         pub fn save(): int\n\
         \x20\x20\x20\x20const draft = Book(title: \"Small Gods\", author: \"Pratchett\")\n\
         \x20\x20\x20\x20^books(1) = draft\n\
         \x20\x20\x20\x20return count(^books)\n\n\
         pub fn title(): string\n\
         \x20\x20\x20\x20return ^books(1).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    let saved = run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(saved.value, Some(Value::Int(1)));
    let title = run_entry(&store, checked_entry!(&program, "test::title")).expect("title");
    assert_eq!(title.value, Some(Value::Str("Small Gods".into())));
}

#[test]
fn sparse_whole_resource_assignment_creates_record_presence() {
    let program = checked_program(
        "resource Note\n\
         \x20\x20\x20\x20body: string\nstore ^notes(id: int): Note\n\n\
         pub fn save(id: int)\n\
         \x20\x20\x20\x20var note: Note\n\
         \x20\x20\x20\x20^notes(id) = note\n\n\
         pub fn hasNote(id: int): bool\n\
         \x20\x20\x20\x20return exists(^notes(id))\n\n\
         pub fn hasBody(id: int): bool\n\
         \x20\x20\x20\x20return exists(^notes(id).body)\n\n\
         pub fn noteCount(): int\n\
         \x20\x20\x20\x20return count(^notes)\n\n\
         pub fn iterCount(): int\n\
         \x20\x20\x20\x20var n = 0\n\
         \x20\x20\x20\x20for id in keys(^notes)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20n = n + 1\n\
         \x20\x20\x20\x20return n\n",
    );
    let store = TreeStore::memory();

    run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Int(1)),
    )
    .expect("save");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::hasNote", Value::Int(1))
        )
        .expect("record presence")
        .value,
        Some(Value::Bool(true))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::noteCount"))
            .expect("root count")
            .value,
        Some(Value::Int(1))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::iterCount"))
            .expect("root iteration")
            .value,
        Some(Value::Int(1))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::hasBody", Value::Int(1))
        )
        .expect("field absence")
        .value,
        Some(Value::Bool(false))
    );
}

#[test]
fn overlong_record_presence_does_not_create_root_presence() {
    let program = checked_program(
        "resource Counter\n\
         \x20\x20\x20\x20value: int\nstore ^counter(id: int): Counter\n\n\
         pub fn hasRoot(): bool\n\
         \x20\x20\x20\x20return exists(^counter)\n\n\
         pub fn rootCount(): int\n\
         \x20\x20\x20\x20return count(^counter)\n",
    );
    let store = TreeStore::memory();
    write_record_presence(
        &program,
        &store,
        "counter",
        &[SavedKey::Int(1), SavedKey::Int(2)],
    );

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::hasRoot"))
            .expect("root presence")
            .value,
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::rootCount"))
            .expect("root count")
            .value,
        Some(Value::Int(0))
    );
}

#[test]
fn resource_constructor_optional_coalesce_is_checker_rejected() {
    checker_rejects(
        "resource Profile\n\
         \x20\x20\x20\x20email: string\n\n\
         pub fn email(): string\n\
         \x20\x20\x20\x20return Profile()?.email ?? \"none\"\n",
        "check.operator_type",
    );
}

#[test]
fn copies_a_whole_resource() {
    let program = checked_program(BOOK_COPY);
    let store = TreeStore::memory();
    seed_field(&program, &store, 1, "title", "Mort");
    seed_field(&program, &store, 1, "shelf", "fiction");
    run_entry(
        &store,
        checked_entry!(&program, "test::copy", Value::Int(1), Value::Int(2)),
    )
    .expect("copy");
    let read = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(2)))
            .expect("run")
            .value
    };
    assert_eq!(read("test::title_of"), Some(Value::Str("Mort".into())));
    assert_eq!(read("test::shelf_of"), Some(Value::Str("fiction".into())));
}

/// A resource declaring an unkeyed nested group (`name`). Whole-resource reads
/// and writes materialize the structural group as a nested resource value.
const PATIENT_WITH_GROUP: &str = "\
resource Patient
    mrn: string
    name
        required first: string
        last: string
store ^patients(id: int): Patient

pub fn read(id: int): Patient
    var fallback: Patient
    fallback.mrn = \"\"
    return ^patients(id) ?? fallback

pub fn copy(from: int, to: int)
    var fallback: Patient
    fallback.mrn = \"\"
    ^patients(to) = ^patients(from) ?? fallback

pub fn first_of(id: int): string
    return ^patients(id)?.name?.first ?? \"\"
";

#[test]
fn whole_resource_read_materializes_unkeyed_groups() {
    let program = checked_program(PATIENT_WITH_GROUP);
    let store = TreeStore::memory();
    seed_patient_field(&program, &store, 1, "mrn", "A1");
    seed_patient_name_field(&program, &store, 1, "first", "Sam");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::read", Value::Int(1)),
    )
    .expect("read");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("mrn".into(), Value::Str("A1".into())),
            (
                "name".into(),
                Value::Resource(vec![("first".into(), Value::Str("Sam".into()))])
            ),
        ]))
    );
}

#[test]
fn whole_resource_write_copies_unkeyed_group_fields() {
    let program = checked_program(PATIENT_WITH_GROUP);
    let store = TreeStore::memory();
    seed_patient_field(&program, &store, 1, "mrn", "A1");
    seed_patient_name_field(&program, &store, 1, "first", "Sam");
    run_entry(
        &store,
        checked_entry!(&program, "test::copy", Value::Int(1), Value::Int(2)),
    )
    .expect("copy");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::first_of", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("Sam".into()))
    );
}

#[test]
fn whole_resource_write_from_local_value_accepts_resources_with_unkeyed_groups() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   binding\n\
         \x20       cover: string\n\
         \x20       spine: string\nstore ^books(id: int): Book\n\n\
         pub fn save(id: int)\n\
         \x20   var book: Book\n\
         \x20   book.title = \"Small Gods\"\n\
         \x20   ^books(id) = book\n\n\
         pub fn title_of(id: int): string\n\
         \x20   return ^books(id).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Int(1)),
    )
    .expect("write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Small Gods".into()))
    );
}

#[test]
fn whole_resource_assignment_clears_omitted_saved_children() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   note: string\n\
         \x20   details\n\
         \x20       edition: string\n\
         \x20   tags(pos: int): string\n\
         \x20   versions(version: int)\n\
         \x20       required title: string\n\
         store ^books(id: int): Book\n\n\
         pub fn replace(id: int)\n\
         \x20   var book: Book\n\
         \x20   book.title = \"replacement\"\n\
         \x20   ^books(id) = book\n\n\
         pub fn title_of(id: int): string\n\
         \x20   return ^books(id).title ?? \"\"\n\n\
         pub fn note_of(id: int): string\n\
         \x20   return ^books(id).note ?? \"missing\"\n\n\
         pub fn edition_of(id: int): string\n\
         \x20   return ^books(id)?.details?.edition ?? \"missing\"\n\n\
         pub fn tag_at(id: int, pos: int): string\n\
         \x20   return ^books(id).tags(pos) ?? \"missing\"\n\n\
         pub fn version_title(id: int, version: int): string\n\
         \x20   return ^books(id).versions(version).title ?? \"missing\"\n",
    );
    let store = TreeStore::memory();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["title"]),
        SavedValue::Str("original".into()),
    );
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["note"]),
        SavedValue::Str("keep?".into()),
    );
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["details", "edition"]),
        SavedValue::Str("first".into()),
    );
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &keyed_data_path(&program, "books", &[("tags", vec![SavedKey::Int(1)])], &[]),
        SavedValue::Str("tag".into()),
    );
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &keyed_data_path(
            &program,
            "books",
            &[("versions", vec![SavedKey::Int(1)])],
            &["title"],
        ),
        SavedValue::Str("v1".into()),
    );

    run_entry(
        &store,
        checked_entry!(&program, "test::replace", Value::Int(1)),
    )
    .expect("replace");

    let read = |entry: &str, args: Vec<Value>| {
        run_entry(&store, checked_entry_call(&program, entry, args))
            .expect("read")
            .value
    };
    assert_eq!(
        read("test::title_of", vec![Value::Int(1)]),
        Some(Value::Str("replacement".into()))
    );
    assert_eq!(
        read("test::note_of", vec![Value::Int(1)]),
        Some(Value::Str("missing".into()))
    );
    assert_eq!(
        read("test::edition_of", vec![Value::Int(1)]),
        Some(Value::Str("missing".into()))
    );
    assert_eq!(
        read("test::tag_at", vec![Value::Int(1), Value::Int(1)]),
        Some(Value::Str("missing".into()))
    );
    assert_eq!(
        read("test::version_title", vec![Value::Int(1), Value::Int(1)]),
        Some(Value::Str("missing".into()))
    );

    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["note"]),
            ScalarType::Str,
        ),
        None
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["details", "edition"]),
            ScalarType::Str,
        ),
        None
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &keyed_data_path(&program, "books", &[("tags", vec![SavedKey::Int(1)])], &[],),
            ScalarType::Str,
        ),
        None
    );
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &keyed_data_path(
                &program,
                "books",
                &[("versions", vec![SavedKey::Int(1)])],
                &["title"],
            ),
            ScalarType::Str,
        ),
        None
    );
}

#[test]
fn whole_resource_assignment_clears_omitted_index_field() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   shelf: string\n\
         store ^books(id: int): Book\n\
         \x20   index byShelf(shelf, id)\n\n\
         pub fn add(id: int, title: string, shelf: string)\n\
         \x20   ^books(id).title = title\n\
         \x20   ^books(id).shelf = shelf\n\n\
         pub fn replace(id: int)\n\
         \x20   var book: Book\n\
         \x20   book.title = \"replacement\"\n\
         \x20   ^books(id) = book\n\n\
         pub fn title_of(id: int): string\n\
         \x20   return ^books(id).title ?? \"\"\n\n\
         pub fn count_by_shelf(shelf: string): int\n\
         \x20   return count(^books.byShelf(shelf))\n",
    );
    let store = TreeStore::memory();
    let call = |entry: &str, args: Vec<Value>| {
        run_entry(&store, checked_entry_call(&program, entry, args))
            .expect("run")
            .value
    };

    call(
        "test::add",
        vec![
            Value::Int(1),
            Value::Str("original".into()),
            Value::Str("fiction".into()),
        ],
    );
    assert_eq!(
        call("test::count_by_shelf", vec![Value::Str("fiction".into())]),
        Some(Value::Int(1))
    );
    let by_shelf = index_catalog_id(&program, "books", "byShelf");
    let old_tuple = [SavedKey::Str("fiction".into()), SavedKey::Int(1)];
    let initial_entries = store
        .scan_index_tuple(&by_shelf, &old_tuple, 2)
        .expect("scan initial index");
    assert!(!initial_entries.entries.is_empty());

    call("test::replace", vec![Value::Int(1)]);

    assert_eq!(
        call("test::count_by_shelf", vec![Value::Str("fiction".into())]),
        Some(Value::Int(0))
    );
    assert_eq!(
        call("test::title_of", vec![Value::Int(1)]),
        Some(Value::Str("replacement".into()))
    );

    let old_entries = store
        .scan_index_tuple(&by_shelf, &old_tuple, 2)
        .expect("scan index");
    assert!(old_entries.entries.is_empty());
}

fn seed_patient_field(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    field: &str,
    value: &str,
) {
    write_data_value(
        program,
        store,
        "patients",
        &[SavedKey::Int(id)],
        &data_path(program, "patients", &[field]),
        SavedValue::Str(value.into()),
    );
}

fn seed_patient_name_field(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    field: &str,
    value: &str,
) {
    write_data_value(
        program,
        store,
        "patients",
        &[SavedKey::Int(id)],
        &data_path(program, "patients", &["name", field]),
        SavedValue::Str(value.into()),
    );
}
