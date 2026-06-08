//! Index-key iteration: counting and printing in key order, bare first-level key
//! iteration, and the fault raised when an indexed field is mutated mid-traversal.

#[macro_use]
mod support;

use support::*;

use marrow_run::{RUN_TRAVERSAL, Value};
use marrow_store::tree::TreeStore;

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
