//! The reference sample end to end, on native storage, its update functions, and
//! group-entry field reads.

#[macro_use]
mod support;

use support::*;

use marrow_run::{Host, RUN_ABSENT, Value};
use marrow_store::tree::TreeStore;

#[test]
fn the_reference_sample_runs_end_to_end() {
    // The canonical sample must run on the in-memory store: add a book in a
    // transaction (whole-resource + history group writes),
    // tag it, and print the fiction shelf via index traversal.
    let program = checked_program(&sample_source());
    let store = TreeStore::memory();
    let host = Host::new().with_clock(1_700_000_000_000_000_000); // 2023-11-14T22:13:20Z
    let outcome = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "shelf::sample::main"),
    )
    .expect("the sample's main runs end-to-end");
    // `main` returns nothing and prints the one fiction book it added.
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "1: Small Gods\n");
}

#[test]
fn the_reference_sample_runs_on_native_storage() {
    // The reference sample must run unchanged on the native redb backend, with
    // output identical to the in-memory run.
    let program = checked_program(&sample_source());
    let dir = tempfile::tempdir().expect("create a temp dir");
    let store = TreeStore::open(&dir.path().join("sample.redb")).expect("open redb");
    let host = Host::new().with_clock(1_700_000_000_000_000_000);
    let outcome = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "shelf::sample::main"),
    )
    .expect("the sample's main runs on native storage");
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "1: Small Gods\n");
}

const BOOK_VERSIONS: &str = "\
resource Book
    required title: string

    versions(version: int)
        required title: string
store ^books(id: int): Book

pub fn seed(id: int, t: string)
    ^books(id).title = t

pub fn set_version_title(id: int, v: int, t: string)
    ^books(id).versions(v).title = t

pub fn version_title(id: int, v: int): string
    return ^books(id).versions(v).title
";

#[test]
fn reads_a_field_from_a_group_entry() {
    let program = checked_program(BOOK_VERSIONS);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("root".into())
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_version_title",
            Value::Int(1),
            Value::Int(2),
            Value::Str("Mort".into())
        ),
    )
    .expect("write");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::version_title",
            Value::Int(1),
            Value::Int(2)
        ),
    )
    .expect("read")
    .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

#[test]
fn reading_an_absent_group_field_is_an_error() {
    let program = checked_program(BOOK_VERSIONS);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::version_title",
            Value::Int(1),
            Value::Int(2)
        ),
    );
    assert_run_error(result, RUN_ABSENT);
}

#[test]
fn the_sample_update_functions_run() {
    // Drive the reference sample's mutating API beyond `main`: add a book, add a
    // note (group write guarded by `exists`), and move it between shelves (a
    // field write that also moves its generated index entry).
    let source = format!(
        "{}\n\n\
         pub fn exerciseUpdates(changedAt: instant): bool\n\
         \x20   const id = add(\n\
         \x20       title: \"Small Gods\",\n\
         \x20       author: \"Terry Pratchett\",\n\
         \x20       shelf: \"fiction\",\n\
         \x20       changedAt: changedAt,\n\
         \x20   )\n\
         \x20   const missing = nextId(^books)\n\
         \x20   const note = addNote(id, \"n1\", \"first\")\n\
         \x20   const missingNote = addNote(missing, \"n2\", \"missing\")\n\
         \x20   moveToShelf(id, \"history\", changedAt)\n\
         \x20   return note and not missingNote\n",
        sample_source()
    );
    let program = checked_program(&source);
    let store = TreeStore::memory();
    let when = Value::Instant(1_700_000_000_000_000_000);
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "shelf::sample::exerciseUpdates", when)
        )
        .expect("exercise sample updates")
        .value,
        Some(Value::Bool(true))
    );
    let shelf = |name: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "shelf::sample::printShelf",
                Value::Str(name.into())
            ),
        )
        .expect("printShelf")
        .output
    };
    assert_eq!(shelf("history"), "1: Small Gods\n", "moved to history");
    assert_eq!(shelf("fiction"), "", "and left fiction");
}
