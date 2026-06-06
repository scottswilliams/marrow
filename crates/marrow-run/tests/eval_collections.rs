//! Append and keyed-leaf writes, index-key iteration, whole-resource read, write,
//! construction and copy.

#[macro_use]
mod support;

use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{RUN_TRAVERSAL, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};

#[test]
fn an_unguarded_absent_element_read_is_rejected() {
    checker_rejects(
        "resource Book at ^books(id: int)\n    title: string\n\npub fn titleOrCode(id: int): string\n    try\n        return ^books(id).title\n    catch err: Error\n        return err.code\n",
        "check.bare_maybe_present_read",
    );
}

#[test]
fn reads_inside_a_transaction_see_earlier_writes() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn rww(id: int): string\n    transaction\n        ^books(id).title = \"fresh\"\n        return ^books(id).title\n"
    ));
    let store = TreeStore::memory();
    let outcome =
        run_entry(&store, checked_entry!(&program, "test::rww", Value::Int(1))).expect("run");
    assert_eq!(outcome.value, Some(Value::Str("fresh".into())));
}

#[test]
fn append_writes_at_the_next_position() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n"
    ));
    let store = TreeStore::memory();
    let appended = |t: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add_tag",
                Value::Int(5),
                Value::Str(t.into())
            ),
        )
        .expect("run")
        .value
    };
    // Successive appends take positions 1 then 2 (no hole-filling).
    assert_eq!(appended("a"), Some(Value::Int(1)));
    assert_eq!(appended("b"), Some(Value::Int(2)));
    // The values landed at `^books(5).tags(1)` and `tags(2)`.
    let tag = |pos: i64| -> Option<SavedValue> {
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(5)],
            &keyed_data_path(
                &program,
                "books",
                &[("tags", vec![SavedKey::Int(pos)])],
                &[],
            ),
            ScalarType::Str,
        )
    };
    assert_eq!(tag(1), Some(SavedValue::Str("a".into())));
    assert_eq!(tag(2), Some(SavedValue::Str("b".into())));
}

#[test]
fn appends_then_reads_back_keyed_leaf_entries() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n\npub fn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_tag",
            Value::Int(5),
            Value::Str("a".into())
        ),
    )
    .expect("append");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_tag",
            Value::Int(5),
            Value::Str("b".into())
        ),
    )
    .expect("append");
    let tag = |pos: i64| {
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(5), Value::Int(pos)),
        )
        .expect("read")
        .value
    };
    assert_eq!(tag(1), Some(Value::Str("a".into())));
    assert_eq!(tag(2), Some(Value::Str("b".into())));
    assert_eq!(tag(3), Some(Value::Str(String::new())));
}

#[test]
fn explicit_keyed_leaf_write_then_reads_back() {
    // `^books(id).tags(pos) = value` writes one keyed-leaf entry directly, and a
    // string-keyed leaf `scores(key) = value` writes through the same path.
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    tags(pos: int): string\n    scores(key: string): int\n\npub fn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n\npub fn set_score(id: int, key: string, n: int)\n    ^books(id).scores(key) = n\n\npub fn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n\npub fn score_at(id: int, key: string): int\n    return ^books(id).scores(key) ?? 0\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_tag",
            Value::Int(5),
            Value::Int(3),
            Value::Str("fiction".into())
        ),
    )
    .expect("explicit keyed-leaf write");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_score",
            Value::Int(5),
            Value::Str("alice".into()),
            Value::Int(7)
        ),
    )
    .expect("string-keyed leaf write");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(5), Value::Int(3))
        )
        .expect("read")
        .value,
        Some(Value::Str("fiction".into()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::score_at",
                Value::Int(5),
                Value::Str("alice".into())
            )
        )
        .expect("read")
        .value,
        Some(Value::Int(7))
    );
}

#[test]
fn explicit_keyed_leaf_write_creates_a_hole_that_append_skips() {
    // An explicit write past the dense range leaves a hole; append chooses one
    // past the highest positive key, not the first gap.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn set_tag(id: int, pos: int, t: string)\n    ^books(id).tags(pos) = t\n\npub fn add_tag(id: int, t: string): int\n    return append(^books(id).tags, t)\n\npub fn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    // Write position 5 directly, leaving 1..=4 as holes.
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_tag",
            Value::Int(9),
            Value::Int(5),
            Value::Str("hi".into())
        ),
    )
    .expect("explicit write");
    // Append lands at 6 (one past the highest positive key), skipping the holes.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add_tag",
                Value::Int(9),
                Value::Str("next".into())
            )
        )
        .expect("append")
        .value,
        Some(Value::Int(6))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(9), Value::Int(6))
        )
        .expect("read")
        .value,
        Some(Value::Str("next".into()))
    );
}

/// A program that indexes books by shelf and traverses the index with `keys`.
const BOOK_SHELF: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

pub fn add(id: int, t: string, s: string)
    ^books(id).title = t
    ^books(id).shelf = s

pub fn count_on(shelf: string): int
    var c = 0
    for id in keys(^books.byShelf(shelf))
        c = c + 1
    return c

pub fn count_via_bare_index(): int
    var c = 0
    for shelf in ^books.byShelf
        for id in ^books.byShelf(shelf)
            c = c + 1
    return c

pub fn reshelve_while_iterating()
    for id in keys(^books.byShelf(\"fiction\"))
        ^books(id).shelf = \"history\"

pub fn reshelve_while_iterating_direct()
    for id in ^books.byShelf(\"fiction\")
        ^books(id).shelf = \"history\"

pub fn titles_on(shelf: string)
    for id in ^books.byShelf(shelf)
        print(^books(id).title)
";

#[test]
fn iterates_index_keys() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ),
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Sourcery", "fiction");
    add(3, "Guards", "history");

    let count = |shelf: &str| {
        run_entry(
            &store,
            checked_entry!(&program, "test::count_on", Value::Str(shelf.into())),
        )
        .expect("count")
        .value
    };
    assert_eq!(count("fiction"), Some(Value::Int(2)));
    assert_eq!(count("history"), Some(Value::Int(1)));
    assert_eq!(count("romance"), Some(Value::Int(0)));
}

#[test]
fn bare_index_iteration_yields_first_level_keys() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ),
        )
        .expect("add");
    };
    add(1, "Mort", "fiction");
    add(2, "Sourcery", "fiction");
    add(3, "Guards", "history");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::count_via_bare_index"),
    )
    .expect("run");
    assert_eq!(outcome.value, Some(Value::Int(3)));
}

#[test]
fn updating_an_indexed_field_while_iterating_that_index_faults() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    for (id, title) in [(1, "Mort"), (2, "Sourcery")] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str("fiction".into()),
            ),
        )
        .expect("add");
    }

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::reshelve_while_iterating"),
        ),
        RUN_TRAVERSAL,
    );
    let remaining = run_entry(
        &store,
        checked_entry!(&program, "test::count_on", Value::Str("fiction".into())),
    )
    .expect("count")
    .value;
    assert_eq!(remaining, Some(Value::Int(2)));
}

#[test]
fn updating_an_indexed_field_while_directly_iterating_that_index_faults() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("fiction".into()),
        ),
    )
    .expect("add");

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::reshelve_while_iterating_direct"),
        ),
        RUN_TRAVERSAL,
    );
}

#[test]
fn prints_titles_in_index_key_order() {
    let program = checked_program(BOOK_SHELF);
    let store = TreeStore::memory();
    let add = |id: i64, title: &str, shelf: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Str(shelf.into()),
            ),
        )
        .expect("add");
    };
    add(2, "Sourcery", "fiction");
    add(1, "Mort", "fiction");

    // The index yields ids in key order (1 then 2), regardless of insert order.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::titles_on", Value::Str("fiction".into())),
    )
    .expect("run");
    assert_eq!(outcome.output, "Mort\nSourcery\n");
}

/// A program that reads, copies, and reads back whole `Book` resources.
const BOOK_COPY: &str = "\
resource Book at ^books(id: int)
    required title: string
    required shelf: string

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
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20shelf: string\n\n\
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
        "resource Book at ^books(id: int)\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20author: string\n\n\
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
resource Patient at ^patients(id: int)
    mrn: string
    name
        required first: string
        last: string

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
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   binding\n\
         \x20       cover: string\n\
         \x20       spine: string\n\n\
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
