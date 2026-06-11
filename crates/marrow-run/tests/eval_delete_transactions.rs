//! Record and field deletes, transaction commit and rollback, and unique-conflict
//! catch and propagation.

#[macro_use]
mod support;

use support::*;

use marrow_run::{RUN_DIVIDE_BY_ZERO, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

#[test]
fn delete_removes_a_record() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn set_title(id: int, t: string)\n    ^books(id).title = t\n\npub fn remove(id: int)\n    delete ^books(id)\n\npub fn has_book(id: int): bool\n    return exists(^books(id))\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_title",
            Value::Int(1),
            Value::Str("Mort".into())
        ),
    )
    .expect("write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(true))
    );
    run_entry(
        &store,
        checked_entry!(&program, "test::remove", Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(false)),
        "the record is gone after delete"
    );
}

#[test]
fn delete_removes_a_sparse_field_and_leaves_a_sibling() {
    // `delete ^books(id).subtitle` removes that field; a sibling field survives.
    let program = checked_program(
        "resource Book\n    required title: string\n    subtitle: string\nstore ^books(id: int): Book\n\npub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).subtitle = \"A Discworld Novel\"\n\npub fn drop_subtitle(id: int)\n    delete ^books(id).subtitle\n\npub fn has_subtitle(id: int): bool\n    return exists(^books(id).subtitle)\n\npub fn title_of(id: int): string\n    return ^books(id).title\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_subtitle", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(true))
    );
    run_entry(
        &store,
        checked_entry!(&program, "test::drop_subtitle", Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_subtitle", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(false)),
        "the field is gone after delete"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title_of", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Str("Mort".into())),
        "the sibling field survives"
    );
}

#[test]
fn deleting_an_indexed_field_removes_its_index_entry() {
    // `delete ^books(id).shelf` where `shelf` feeds `byShelf` tears down the entry,
    // so a later `keys(^books.byShelf(...))` no longer yields the record.
    let program = checked_program(&format!(
        "{BOOK_SHELF_INDEX_SCHEMA}pub fn add(id: int, t: string, s: string)\n    ^books(id).title = t\n    ^books(id).shelf = s\n\npub fn drop_shelf(id: int)\n    delete ^books(id).shelf\n\npub fn count_on(shelf: string): int\n    var c = 0\n    for id in keys(^books.byShelf(shelf))\n        c = c + 1\n    return c\n"
    ));
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
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::count_on", Value::Str("fiction".into()))
        )
        .expect("run")
        .value,
        Some(Value::Int(1))
    );
    run_entry(
        &store,
        checked_entry!(&program, "test::drop_shelf", Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::count_on", Value::Str("fiction".into()))
        )
        .expect("run")
        .value,
        Some(Value::Int(0)),
        "the index entry the deleted field fed is gone"
    );
}

#[test]
fn deleting_a_required_field_is_rejected() {
    // A required field can only go away when its entry/resource is deleted.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n\npub fn drop_title(id: int)\n    delete ^books(id).title\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::drop_title", Value::Int(1)),
    );
    assert_run_error(result, "write.required_field");
}

#[test]
fn deleting_a_layer_entry_leaves_other_entries() {
    // `delete ^books(id).versions(v)` removes one group-entry subtree; siblings
    // survive. Read each entry's `.title` to prove it: the deleted entry's title
    // falls back to the `??` default, the survivor's stays intact.
    let program = checked_program(
        "resource Book\n    required title: string\n    versions(version: int)\n        required title: string\nstore ^books(id: int): Book\n\npub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).versions(1).title = \"first\"\n    ^books(id).versions(2).title = \"second\"\n\npub fn drop_version(id: int, v: int)\n    delete ^books(id).versions(v)\n\npub fn version_title(id: int, v: int): string\n    return ^books(id).versions(v).title ?? \"<gone>\"\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(&program, "test::drop_version", Value::Int(1), Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::version_title",
                Value::Int(1),
                Value::Int(1)
            )
        )
        .expect("run")
        .value,
        Some(Value::Str("<gone>".into())),
        "the deleted version's subtree is gone"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::version_title",
                Value::Int(1),
                Value::Int(2)
            )
        )
        .expect("run")
        .value,
        Some(Value::Str("second".into())),
        "the other version survives"
    );
}

#[test]
fn deleting_a_keyed_leaf_entry_leaves_other_entries() {
    // `delete ^books(id).tags(pos)` removes one keyed-leaf entry; siblings survive.
    // `count(^books(id).tags)` counts the remaining entries; reading the deleted
    // one is an absent-element error while the survivor reads back.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).tags(1) = \"fiction\"\n    ^books(id).tags(2) = \"funny\"\n\npub fn drop_tag(id: int, pos: int)\n    delete ^books(id).tags(pos)\n\npub fn tag_count(id: int): int\n    return count(^books(id).tags)\n\npub fn tag_at(id: int, pos: int): string\n    return ^books(id).tags(pos) ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(&program, "test::drop_tag", Value::Int(1), Value::Int(1)),
    )
    .expect("delete");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_count", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Int(1)),
        "one tag remains after deleting one of two"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(1), Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str(String::new()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_at", Value::Int(1), Value::Int(2))
        )
        .expect("run")
        .value,
        Some(Value::Str("funny".into())),
        "the other tag survives"
    );
}

#[test]
fn a_transaction_commits_on_normal_exit() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn save(id: int)\n    transaction\n        ^books(id).title = \"kept\"\n\npub fn title_of(id: int): string\n    return ^books(id).title\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Int(1)),
    )
    .expect("commit");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::title_of", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Str("kept".into()))
    );
}

#[test]
fn a_transaction_rolls_back_on_an_escaping_error() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn risky(id: int)\n    transaction\n        ^books(id).title = \"staged\"\n        const x = 1 / 0\n\npub fn has_book(id: int): bool\n    return exists(^books(id))\n"
    ));
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::risky", Value::Int(1)),
    );
    assert_run_error(result, RUN_DIVIDE_BY_ZERO);
    // The write staged before the error was rolled back.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .expect("run")
        .value,
        Some(Value::Bool(false)),
        "the staged write rolled back with the transaction"
    );
}

/// A `Book` with a unique `isbn` index plus helpers that seed a record, attempt
/// a conflicting write under `try`/`catch`, and read a field back. Used by the
/// recoverable-write-fault tests.
const UNIQUE_RECOVERY: &str = "\
resource Book
    required title: string
    isbn: string
store ^books(id: int): Book

    index byIsbn(isbn) unique

pub fn seed(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

pub fn claimOrCode(id: int, isbn: string): string
    try
        ^books(id).isbn = isbn
    catch err: Error
        return err.code
    return \"written\"

pub fn claim(id: int, isbn: string)
    ^books(id).isbn = isbn

pub fn recover(id: int, isbn: string, fallback: string): string
    try
        ^books(id).isbn = isbn
    catch err: Error
        ^books(id).title = fallback
    return ^books(id).title ?? \"\"

pub fn titleOf(id: int): string
    return ^books(id).title ?? \"\"

pub fn isbnOf(id: int): string
    return ^books(id).isbn ?? \"\"

pub fn ownerOf(isbn: string): Id(^books)
    for id in ^books.byIsbn(isbn)
        return id
    throw Error(code: \"test.missing_isbn\", message: \"missing isbn\")
";

#[test]
fn a_unique_conflict_is_catchable_and_binds_the_dotted_code() {
    // A unique-index conflict surfaces as a catchable Error, so a `try`/`catch`
    // inside the writing function binds it by its `write.unique_conflict` code
    // and the function continues normally.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    // Book 2 tries to claim book 1's isbn: a unique conflict the catch binds.
    let caught = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::claimOrCode",
            Value::Int(2),
            Value::Str("978-0".into())
        ),
    )
    .expect("caught")
    .value;
    assert_eq!(caught, Some(Value::Str("write.unique_conflict".into())));
}

#[test]
fn a_caught_unique_conflict_lets_following_code_run_and_did_not_write() {
    // After catching the conflict, code keeps running (writes a fallback) and the
    // rejected write left no effect: book 2 still owns its original isbn.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    let title = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::recover",
            Value::Int(2),
            Value::Str("978-0".into()),
            Value::Str("fallback".into()),
        ),
    )
    .expect("recovered")
    .value;
    assert_eq!(title, Some(Value::Str("fallback".into())), "catch body ran");
    // The rejected write left no effect: book 2 still has its original isbn and the
    // unique index still maps the conflicting isbn to book 1, not book 2.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::isbnOf", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("978-9".into())),
        "book 2's isbn was not overwritten",
    );
    assert_identity_value(
        run_entry(
            &store,
            checked_entry!(&program, "test::ownerOf", Value::Str("978-0".into())),
        )
        .expect("read")
        .value,
        "books",
        &[SavedKey::Int(1)],
    );
}

#[test]
fn an_uncaught_unique_conflict_keeps_its_dotted_code() {
    // A unique conflict that escapes the entry surfaces with its own
    // `write.unique_conflict` code rather than a generic uncaught-error code.
    let program = checked_program(UNIQUE_RECOVERY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::claim",
                Value::Int(2),
                Value::Str("978-0".into())
            ),
        ),
        "write.unique_conflict",
    );
}

#[test]
fn a_unique_conflict_inside_a_transaction_can_be_caught_and_continue() {
    // A conflict caught inside a transaction has no effect, and the transaction
    // continues and commits its other writes.
    let program = checked_program(&format!(
        "{BOOK_ISBN_SCHEMA}pub fn seed(id: int, t: string, isbn: string)\n    ^books(id).title = t\n    ^books(id).isbn = isbn\n\npub fn run_it(id: int, isbn: string, t: string)\n    transaction\n        try\n            ^books(id).isbn = isbn\n        catch err: Error\n            ^books(id).title = t\n\npub fn titleOf(id: int): string\n    return ^books(id).title ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::run_it",
            Value::Int(2),
            Value::Str("978-0".into()),
            Value::Str("after".into()),
        ),
    )
    .expect("transaction commits after catching");
    // The transaction's other write (the title) committed.
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::titleOf", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("after".into())),
    );
}

#[test]
fn a_caught_write_fault_does_not_leak_into_a_later_fault() {
    // After a `try` catches a write fault, the stashed Error is cleared, so a later
    // genuine fault (divide-by-zero) still faults rather than being miscaught.
    let program = checked_program(&format!(
        "{BOOK_ISBN_SCHEMA}pub fn seed(id: int, t: string, isbn: string)\n    ^books(id).title = t\n    ^books(id).isbn = isbn\n\npub fn run_it(): int\n    try\n        ^books(2).isbn = \"978-0\"\n    catch err: Error\n        write(\"caught\")\n    const boom = 1 / 0\n    return 0\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(2),
            Value::Str("Pyramids".into()),
            Value::Str("978-9".into()),
        ),
    )
    .expect("seed");
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::run_it")),
        RUN_DIVIDE_BY_ZERO,
    );
}
