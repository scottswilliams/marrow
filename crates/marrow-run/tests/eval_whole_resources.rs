//! Whole-resource values: reading, constructing (including qualified and nested
//! local-resource fields), saving, copying, and materializing unkeyed groups.

#[macro_use]
mod support;

use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

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
         \x20\x20\x20\x20return person.address.city\n",
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
         \x20\x20\x20\x20return ^books(1).title\n",
    );
    let store = TreeStore::memory();
    let saved = run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    assert_eq!(saved.value, Some(Value::Int(1)));
    let title = run_entry(&store, checked_entry!(&program, "test::title")).expect("title");
    assert_eq!(title.value, Some(Value::Str("Small Gods".into())));
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
