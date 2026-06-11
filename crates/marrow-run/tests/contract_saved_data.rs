//! Saved-data fail-closed contracts: a required field absent in saved data faults
//! rather than defaulting, and an absent unique-indexed field is not a duplicate
//! unique key. Both pin the boundary where saved data is incomplete: a fault, never
//! a silent value, and absence is its own state, not a collision.

#[macro_use]
mod support;

use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{RUN_ABSENT, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

/// A record with two required fields. The store seed writes `title` but not
/// `pages`, modeling saved data missing a required field — the exact shape a
/// botched write or an un-applied required-field evolution would leave behind.
const REQUIRED_PAGES: &str = "\
resource Book
    required title: string
    required pages: int
store ^books(id: int): Book

pub fn pages_of(id: int): int
    if const book = ^books(id)
        return book.pages
    throw Error(code: \"test.missing_book\", message: \"missing book\")

pub fn whole(id: int): Book
    var fallback: Book
    fallback.title = \"\"
    fallback.pages = 0
    if const book = ^books(id)
        return book
    return fallback

pub fn pages_or_caught(id: int): string
    try
        if const book = ^books(id)
            return $\"{book.pages}\"
        return \"missing book\"
    catch err: Error
        return err.code
";

fn store_missing_required_pages(program: &CheckedRuntimeProgram, id: i64) -> TreeStore {
    let store = TreeStore::memory();
    write_data_value(
        program,
        &store,
        "books",
        &[SavedKey::Int(id)],
        &data_path(program, "books", &["title"]),
        SavedValue::Str("Mort".into()),
    );
    store
}

#[test]
fn a_required_field_absent_in_saved_data_faults_rather_than_defaulting() {
    // Reading a required field that saved data does not carry raises an absent-element
    // fault. The read is fail-closed: there is no `??` on the read site, so nothing
    // substitutes a default for the missing required value.
    let program = checked_program(REQUIRED_PAGES);
    let store = store_missing_required_pages(&program, 1);
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::pages_of", Value::Int(1)),
    );
    assert_run_error(result, RUN_ABSENT);
}

#[test]
fn a_whole_resource_read_over_a_missing_required_field_faults() {
    // Materializing the whole record when a required member is absent in saved data
    // faults rather than yielding a partial resource or the `??` fallback. The fallback
    // applies only to a wholly-absent record, not to one missing a required field.
    let program = checked_program(REQUIRED_PAGES);
    let store = store_missing_required_pages(&program, 1);
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::whole", Value::Int(1)),
    );
    assert_run_error(result, RUN_ABSENT);
}

#[test]
fn a_missing_required_field_fault_during_materialization_is_not_caught() {
    // The required-field-absent fault during whole-resource materialization carries
    // the absent-element code, but is fatal rather than catchable. The surrounding
    // catch must not convert corrupted saved data into an ordinary fallback path.
    let program = checked_program(REQUIRED_PAGES);
    let store = store_missing_required_pages(&program, 1);
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::pages_or_caught", Value::Int(1)),
    );
    assert_run_error(result, RUN_ABSENT);
}

/// A keyed root with a unique index over a sparse `isbn` field. Records can be
/// seeded with or without `isbn`; lookups read records back through the root and
/// the unique index so absence and presence are both observable.
const UNIQUE_ISBN: &str = "\
resource Book
    required title: string
    isbn: string
store ^books(id: int): Book

    index byIsbn(isbn) unique

pub fn seed_without_isbn(id: int, t: string)
    ^books(id).title = t

pub fn seed_with_isbn(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

pub fn record_count(): int
    var c = 0
    for book in ^books
        c = c + 1
    return c

pub fn isbn_owner(isbn: string): Id(^books)
    for id in ^books.byIsbn(isbn)
        return id
    throw Error(code: \"test.no_owner\", message: \"no owner\")
";

#[test]
fn two_records_with_an_absent_unique_field_coexist() {
    // A unique index keys on present values only. Two records that both leave the
    // unique-indexed `isbn` absent are not a duplicate-key conflict: absence is not a
    // value, so neither write collides and both records persist.
    let program = checked_program(UNIQUE_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed_without_isbn",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("first record without isbn");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed_without_isbn",
            Value::Int(2),
            Value::Str("Reaper".into())
        ),
    )
    .expect("second record without isbn is not a unique conflict");

    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::record_count"))
            .expect("count")
            .value,
        Some(Value::Int(2)),
        "both records with an absent unique key coexist"
    );
}

#[test]
fn a_present_unique_key_still_conflicts_after_an_absent_one() {
    // Absence does not occupy the unique key, so a later present value is still free to
    // claim it and a second claim of that same present value still conflicts. This pins
    // that the absent records did not silently reserve the key.
    let program = checked_program(UNIQUE_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed_without_isbn",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("absent-isbn record");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed_with_isbn",
            Value::Int(2),
            Value::Str("Reaper".into()),
            Value::Str("978-2".into())
        ),
    )
    .expect("a present isbn claims the key freely after an absent one");

    // The unique index resolves the present value to its record.
    assert_identity_value(
        run_entry(
            &store,
            checked_entry!(&program, "test::isbn_owner", Value::Str("978-2".into())),
        )
        .expect("lookup")
        .value,
        "books",
        &[SavedKey::Int(2)],
    );

    // A third record claiming the same present value is a genuine unique conflict.
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed_with_isbn",
            Value::Int(3),
            Value::Str("Sourcery".into()),
            Value::Str("978-2".into())
        ),
    );
    assert_run_error(result, "write.unique_conflict");
}
