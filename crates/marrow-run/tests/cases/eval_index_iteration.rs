//! Index-key iteration: counting and printing in key order, bare first-level key
//! iteration, and the fault raised when an indexed field is mutated mid-traversal.

use crate::support;
use support::*;

use marrow_run::{RUN_TRAVERSAL, RUN_TYPE, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

/// A program that indexes books by shelf and traverses the index with `keys`.
const BOOK_SHELF: &str = "\
resource Book
    required title: string
    shelf: string
store ^books(id: int): Book

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
    for id, book in ^books.byShelf(shelf)
        print(book.title)
";

const POST_DATES: &str = "\
resource Post
    required title: string
    published: int
store ^posts(id: int): Post

    index byDate(published, id)

pub fn add(id: int, title: string, published: int)
    ^posts(id).title = title
    ^posts(id).published = published

pub fn titlesBetween(start: int, end: int)
    for id in ^posts.byDate(start..end)
        print(^posts(id).title ?? \"\")

pub fn titlePairsBetween(start: int, end: int)
    for id, post in ^posts.byDate(start..end)
        print($\"{id}: {post.title}\")

pub fn countBetween(start: int, end: int): int
    return count(^posts.byDate(start..end))

pub fn existsBetween(start: int, end: int): bool
    return exists(^posts.byDate(start..end))

pub fn titlesFrom(start: int)
    for id in ^posts.byDate(start..)
        print(^posts(id).title ?? \"\")

pub fn titlesBefore(end: int)
    for id in ^posts.byDate(..end)
        print(^posts(id).title ?? \"\")

pub fn titlesThrough(end: int)
    for id in ^posts.byDate(..=end)
        print(^posts(id).title ?? \"\")

pub fn titlesAfter(lastSeen: int, end: int)
    for id in ^posts.byDate(lastSeen..end)
        if (^posts(id).published ?? 0) != lastSeen
            print(^posts(id).title ?? \"\")

pub fn titlesBetweenInverted(start: int, end: int)
    for id in ^posts.byDate(start..end)
        print(^posts(id).title ?? \"\")
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

#[test]
fn bounded_index_range_streams_matching_records() {
    let program = checked_program(POST_DATES);
    let store = TreeStore::memory();
    for (id, title, published) in [(1, "A", 10), (2, "B", 20), (3, "C", 30), (4, "D", 40)] {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::add",
                Value::Int(id),
                Value::Str(title.into()),
                Value::Int(published),
            ),
        )
        .expect("add");
    }

    let between = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titlesBetween",
            Value::Int(20),
            Value::Int(40)
        ),
    )
    .expect("between");
    assert_eq!(between.output, "B\nC\n");

    let pairs = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titlePairsBetween",
            Value::Int(20),
            Value::Int(40)
        ),
    )
    .expect("pairs");
    assert_eq!(pairs.output, "2: B\n3: C\n");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::countBetween",
                Value::Int(100),
                Value::Int(200)
            )
        )
        .expect("count empty range")
        .value,
        Some(Value::Int(0))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::existsBetween",
                Value::Int(100),
                Value::Int(200)
            )
        )
        .expect("exists empty range")
        .value,
        Some(Value::Bool(false))
    );

    let from = run_entry(
        &store,
        checked_entry!(&program, "test::titlesFrom", Value::Int(30)),
    )
    .expect("from");
    assert_eq!(from.output, "C\nD\n");

    let before = run_entry(
        &store,
        checked_entry!(&program, "test::titlesBefore", Value::Int(30)),
    )
    .expect("before");
    assert_eq!(before.output, "A\nB\n");

    let through = run_entry(
        &store,
        checked_entry!(&program, "test::titlesThrough", Value::Int(30)),
    )
    .expect("through");
    assert_eq!(through.output, "A\nB\nC\n");

    let page = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titlesAfter",
            Value::Int(20),
            Value::Int(50)
        ),
    )
    .expect("page after");
    assert_eq!(page.output, "C\nD\n");

    let inverted = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titlesBetweenInverted",
            Value::Int(40),
            Value::Int(20)
        ),
    )
    .expect("inverted");
    assert_eq!(inverted.output, "");
}

#[test]
fn bounded_enum_range_rejects_corrupt_ranged_component() {
    let program = checked_program(
        "enum Status\n\
         \x20   draft\n\
         \x20   published\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required status: Status\n\
         store ^books(id: int): Book\n\
         \x20   index byStatus(status, id)\n\
         \n\
         pub fn countDraftOrLater(): int\n\
         \x20   return count(^books.byStatus(Status::draft..))\n\
         \n\
         pub fn hasDraftOrLater(): bool\n\
         \x20   return exists(^books.byStatus(Status::draft..))\n\
         \n\
         pub fn printDraftOrLater()\n\
         \x20   for id in ^books.byStatus(Status::draft..)\n\
         \x20       print(id)\n",
    );
    let store = TreeStore::memory();
    let corrupt_status = format!(
        "{}~",
        enum_member_catalog_id(&program, "Status", "draft").as_str()
    );
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byStatus"),
            &[SavedKey::Str(corrupt_status), SavedKey::Int(1)],
            &[SavedKey::Int(1)],
            Vec::new(),
        )
        .expect("corrupt enum range entry");

    for entry in [
        "test::countDraftOrLater",
        "test::hasDraftOrLater",
        "test::printDraftOrLater",
    ] {
        assert_run_error(run_entry(&store, checked_entry!(&program, entry)), RUN_TYPE);
    }
}
