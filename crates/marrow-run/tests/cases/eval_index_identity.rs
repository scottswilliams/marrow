//! Identity values reconstructed from index traversal: unique-index lookups in
//! value position, and composite-identity index loops.

use crate::support;
use support::*;

use marrow_run::{RUN_TRAVERSAL, RUN_TYPE, RUN_UNSUPPORTED, Value};
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::TreeStore;

// --- Unique-index identity reads ---

/// A book with a unique index on `isbn`. `register` stores the book, and
/// `titleByIsbn` reads the identity back from the unique-index lookup path and
/// uses it to address the record.
const BOOK_ISBN: &str = "\
resource Book
    required title: string
    isbn: string
store ^books(id: int): Book

    index byIsbn(isbn) unique

pub fn register(id: int, t: string, isbn: string)
    ^books(id).title = t
    ^books(id).isbn = isbn

pub fn titleByIsbnKey(isbn: string, fallback: int): string
    for id in ^books.byIsbn(isbn)
        return ^books(id).title ?? \"\"
    return ^books(fallback).title ?? \"\"

pub fn hasIsbn(isbn: string): bool
    return exists(^books.byIsbn(isbn))

pub fn countIsbn(isbn: string): int
    return count(^books.byIsbn(isbn))

pub fn iterTitlesByIsbn(isbn: string)
    for id in ^books.byIsbn(isbn)
        print(^books(id).title ?? \"\")

pub fn changeIsbn(id: Id(^books))
    ^books(id).isbn = \"978-1\"

pub fn changeIsbnThroughHelper(isbn: string)
    for id in ^books.byIsbn(isbn)
        changeIsbn(id)
";

#[test]
fn reads_an_identity_from_a_unique_index() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titleByIsbnKey",
            Value::Str("978-0".into()),
            Value::Int(42)
        ),
    )
    .expect("titleByIsbn")
    .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
}

#[test]
fn a_unique_index_value_read_rejects_the_wrong_arity_at_runtime() {
    let resource = BOOK_ISBN_SCHEMA;
    checker_rejects(
        &format!("{resource}fn badIsbnMissing()\n    return ^books.byIsbn()\n"),
        "check.key_type",
    );
    checker_rejects(
        &format!("{resource}fn badIsbnExtra(isbn: string)\n    return ^books.byIsbn(isbn, 1)\n"),
        "check.key_type",
    );
}

#[test]
fn an_absent_unique_index_lookup_uses_the_fallback_identity() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(99),
            Value::Str("Fallback".into()),
            Value::Str("fallback-isbn".into()),
        ),
    )
    .expect("register fallback");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::titleByIsbnKey",
            Value::Str("missing".into()),
            Value::Int(99)
        ),
    )
    .expect("fallback")
    .value;
    assert_eq!(value, Some(Value::Str("Fallback".into())));
}

#[test]
fn unique_index_presence_and_count_follow_the_lookup_value() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");

    let call = |entry: &str, isbn: &str| {
        run_entry(
            &store,
            checked_entry!(&program, entry, Value::Str(isbn.into())),
        )
        .expect(entry)
        .value
    };
    assert_eq!(call("test::hasIsbn", "978-0"), Some(Value::Bool(true)));
    assert_eq!(call("test::hasIsbn", "missing"), Some(Value::Bool(false)));
    assert_eq!(call("test::countIsbn", "978-0"), Some(Value::Int(1)));
    assert_eq!(call("test::countIsbn", "missing"), Some(Value::Int(0)));
}

#[test]
fn unique_index_conflict_message_includes_index_name_and_key_preview() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register first book");

    let error = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(43),
            Value::Str("Pyramids".into()),
            Value::Str("978-0".into()),
        ),
    )
    .unwrap_err();
    assert_eq!(error.code, "write.unique_conflict");
    assert_eq!(
        error.message,
        "unique index `byIsbn` already holds key(s) (\"978-0\") for another identity"
    );
}

#[test]
fn unique_index_conflict_key_previews_are_bounded() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    let mut isbn = "978-".to_string();
    isbn.push_str(&"x".repeat(96));
    isbn.push_str("-tail-marker");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str(isbn.clone()),
        ),
    )
    .expect("register first book");

    let error = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(43),
            Value::Str("Pyramids".into()),
            Value::Str(isbn),
        ),
    )
    .unwrap_err();
    assert_eq!(error.code, "write.unique_conflict");
    assert!(error.message.contains("unique index `byIsbn`"));
    assert!(
        error.message.contains("(\"978-"),
        "message must include the bounded key prefix: {}",
        error.message
    );
    assert!(
        !error.message.contains("tail-marker"),
        "message must not contain the unbounded key tail: {}",
        error.message
    );
}

#[test]
fn unique_index_lookup_iteration_yields_the_stored_identity() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");

    let present = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::iterTitlesByIsbn",
            Value::Str("978-0".into())
        ),
    )
    .expect("present unique lookup iterates");
    assert_eq!(present.output, "Mort\n");

    let absent = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::iterTitlesByIsbn",
            Value::Str("missing".into())
        ),
    )
    .expect("absent unique lookup is an empty iteration");
    assert_eq!(absent.output, "");
}

#[test]
fn helper_call_mutating_a_traversed_unique_index_faults() {
    let program = checked_program(BOOK_ISBN);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::changeIsbnThroughHelper",
                Value::Str("978-0".into())
            ),
        ),
        RUN_TRAVERSAL,
    );
}

#[test]
fn keys_over_a_unique_index_lookup_is_not_a_collection() {
    let program = checked_program(&format!(
        "{BOOK_ISBN_SCHEMA}pub fn register(id: int, t: string, isbn: string)\n    ^books(id).title = t\n    ^books(id).isbn = isbn\n\npub fn countKeysByIsbn(isbn: string): int\n    var c = 0\n    for id in keys(^books.byIsbn(isbn))\n        c = c + 1\n    return c\n"
    ));
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::register",
            Value::Int(42),
            Value::Str("Mort".into()),
            Value::Str("978-0".into()),
        ),
    )
    .expect("register");

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::countKeysByIsbn",
                Value::Str("978-0".into())
            ),
        ),
        RUN_UNSUPPORTED,
    );
}

#[test]
fn unique_index_prefix_branch_presence_count_and_iteration_agree() {
    checker_rejects(
        "resource Item\n    required title: string\n    series: string\n    code: string\nstore ^items(id: int): Item\n\n    index bySeriesCode(series, code) unique\n\npub fn countSeries(series: string): int\n    return count(^items.bySeriesCode(series))\n",
        "check.key_type",
    );
}

#[test]
fn unique_index_prefix_branch_loops_are_rejected_by_the_checker() {
    let source = "resource Item\n    required title: string\n    series: string\n    code: string\nstore ^items(id: int): Item\n\n    index bySeriesCode(series, code) unique\n\npub fn titlesInSeries(series: string)\n    for id in ^items.bySeriesCode(series)\n        print(^items(id).title ?? \"\")\n";
    checker_rejects(source, "check.key_type");

    let source = "resource Item\n    required title: string\n    series: string\n    code: string\nstore ^items(id: int): Item\n\n    index bySeriesCode(series, code) unique\n\npub fn titlesInAnySeries()\n    for id in ^items.bySeriesCode\n        print(^items(id).title ?? \"\")\n";
    checker_rejects(source, "check.key_type");
}

/// A non-unique index in value position has no single identity to yield; the
/// runtime rejects it and points the reader at `keys(...)`.
const BOOK_SHELF_VALUE: &str = "\
resource Book
    required title: string
    shelf: string
store ^books(id: int): Book

    index byShelf(shelf, id)

pub fn firstOnShelf(shelf: string): Id(^books)
    return ^books.byShelf(shelf)
";

#[test]
fn a_non_unique_index_in_value_position_is_rejected() {
    checker_rejects(BOOK_SHELF_VALUE, "check.untyped_value");
}

const BOOKS_BY_AUTHOR: &str = "\
resource Author
    required name: string
store ^authors(id: int): Author

resource Book
    required title: string
    authorId: Id(^authors)
store ^books(id: int): Book

    index byAuthor(authorId, id)

pub fn seed()
    const ann = Id(^authors, 1)
    const bob = Id(^authors, 2)
    ^authors(ann).name = \"Ann\"
    ^authors(bob).name = \"Bob\"
    ^books(1).title = \"A\"
    ^books(1).authorId = ann
    ^books(2).title = \"B\"
    ^books(2).authorId = bob
    ^books(3).title = \"C\"
    ^books(3).authorId = ann

fn titlesByAuthor(author: Id(^authors))
    for id in ^books.byAuthor(author)
        print(^books(id).title ?? \"\")

pub fn titlesByAnn()
    titlesByAuthor(Id(^authors, 1))

pub fn titlesByBob()
    titlesByAuthor(Id(^authors, 2))
";

#[test]
fn index_over_identity_field_streams_matching_records() {
    let program = checked_program(BOOKS_BY_AUTHOR);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let ann =
        run_entry(&store, checked_entry!(&program, "test::titlesByAnn")).expect("titles by ann");
    assert_eq!(ann.output, "A\nC\n");

    let bob =
        run_entry(&store, checked_entry!(&program, "test::titlesByBob")).expect("titles by bob");
    assert_eq!(bob.output, "B\n");
}

#[test]
fn partial_non_unique_identity_component_traversal_yields_identity_values() {
    let program = checked_program(
        "resource Author\n\
         \x20   required name: string\n\
         store ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   required author: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \x20   index byAuthor(author, id)\n\
         \n\
         pub fn seed()\n\
         \x20   const ann = Id(^authors, 1)\n\
         \x20   ^authors(ann).name = \"Ann\"\n\
         \x20   var book: Book\n\
         \x20   book.title = \"Book\"\n\
         \x20   book.author = ann\n\
         \x20   ^books(7) = book\n\
         \n\
         pub fn authorNamesByBranch()\n\
         \x20   for author in ^books.byAuthor\n\
         \x20       print(^authors(author).name ?? \"\")\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::authorNamesByBranch"),
    )
    .expect("run");
    assert_eq!(outcome.output, "Ann\n");
}

#[test]
fn partial_non_unique_enum_component_traversal_yields_enum_values() {
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
         pub fn seed()\n\
         \x20   var book: Book\n\
         \x20   book.title = \"Published\"\n\
         \x20   book.status = Status::published\n\
         \x20   ^books(1) = book\n\
         \n\
         pub fn publishedStatusesByBranch()\n\
         \x20   for status in ^books.byStatus\n\
         \x20       if status is Status::published\n\
         \x20           print(\"published\")\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::publishedStatusesByBranch"),
    )
    .expect("run");
    assert_eq!(outcome.output, "published\n");
}

#[test]
fn malformed_unique_index_identity_payload_cannot_feed_identity_index_argument() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   isbn: string\n\
         store ^books(id: int): Book\n\
         \x20   index byIsbn(isbn) unique\n\
         \n\
         resource Loan\n\
         \x20   required book: Id(^books)\n\
         store ^loans(id: int): Loan\n\
         \x20   index byBook(book, id)\n\
         \n\
         pub fn seed()\n\
         \x20   ^books(1).title = \"Bad\"\n\
         \x20   ^books(1).isbn = \"bad\"\n\
         \n\
         pub fn countLoansThroughBadIsbn(): int\n\
         \x20   for book in ^books.byIsbn(\"bad\")\n\
         \x20       return count(^loans.byBook(book))\n\
         \x20   return -1\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byIsbn"),
            &[SavedKey::Str("bad".into())],
            &[SavedKey::Int(1)],
            encode_identity_payload(&[SavedKey::Str("not-an-int".into())]),
        )
        .expect("corrupt unique index value");

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::countLoansThroughBadIsbn"),
        ),
        RUN_TYPE,
    );
}

#[test]
fn malformed_non_unique_identity_suffix_cannot_yield_identity() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   tag: string\n\
         store ^books(id: int): Book\n\
         \x20   index byTag(tag, id)\n\
         \n\
         pub fn printBooksThroughTag()\n\
         \x20   for book in ^books.byTag(\"x\")\n\
         \x20       print(book)\n",
    );
    let store = TreeStore::memory();
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byTag"),
            &[SavedKey::Str("x".into()), SavedKey::Str("bad".into())],
            &[SavedKey::Str("bad".into())],
            Vec::new(),
        )
        .expect("corrupt non-unique index suffix");

    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::printBooksThroughTag"),
        ),
        RUN_TYPE,
    );
}

#[test]
fn count_over_partial_enum_index_branch_rejects_corrupt_component() {
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
         pub fn countStatuses(): int\n\
         \x20   return count(^books.byStatus)\n",
    );
    let store = TreeStore::memory();
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byStatus"),
            &[SavedKey::Str("not-a-member".into()), SavedKey::Int(1)],
            &[SavedKey::Int(1)],
            Vec::new(),
        )
        .expect("corrupt enum index branch");

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::countStatuses")),
        RUN_TYPE,
    );
}

#[test]
fn exact_non_unique_presence_rejects_corrupt_identity_suffix() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   required tag: string\n\
         store ^books(id: int): Book\n\
         \x20   index byTag(tag, id)\n\
         \n\
         pub fn hasExactTag(): bool\n\
         \x20   return exists(^books.byTag(\"x\", 7))\n",
    );
    let store = TreeStore::memory();
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byTag"),
            &[SavedKey::Str("x".into()), SavedKey::Int(7)],
            &[SavedKey::Str("bad".into())],
            Vec::new(),
        )
        .expect("corrupt exact non-unique index suffix");

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::hasExactTag")),
        RUN_TYPE,
    );
}

#[test]
fn exact_non_unique_presence_rejects_tuple_identity_mismatch() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   required tag: string\n\
         store ^books(account: int, id: int): Book\n\
         \x20   index byTag(tag, account, id)\n\
         \n\
         pub fn hasExactTaggedBook(): bool\n\
         \x20   return exists(^books.byTag(\"x\", 3, 7))\n",
    );
    let store = TreeStore::memory();
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byTag"),
            &[
                SavedKey::Str("x".into()),
                SavedKey::Int(3),
                SavedKey::Int(7),
            ],
            &[SavedKey::Int(3), SavedKey::Int(8)],
            Vec::new(),
        )
        .expect("corrupt exact non-unique index identity");

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::hasExactTaggedBook")),
        RUN_TYPE,
    );
}

#[test]
fn partial_composite_identity_presence_accepts_valid_branch() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   required tag: string\n\
         store ^books(account: int, id: int): Book\n\
         \x20   index byTag(tag, account, id)\n\
         \n\
         pub fn seed()\n\
         \x20   var book: Book\n\
         \x20   book.title = \"Book\"\n\
         \x20   book.tag = \"x\"\n\
         \x20   ^books(3, 7) = book\n\
         \n\
         pub fn hasTaggedBook(): bool\n\
         \x20   return exists(^books.byTag(\"x\"))\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let value = run_entry(&store, checked_entry!(&program, "test::hasTaggedBook"))
        .expect("presence")
        .value;
    assert_eq!(value, Some(Value::Bool(true)));
}

#[test]
fn walked_composite_identity_count_exhausts_exact_tuple_entries() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   required tag: string\n\
         store ^books(account: int, id: int): Book\n\
         \x20   index byTag(tag, account, id)\n\
         \n\
         pub fn countTaggedBooks(): int\n\
         \x20   return count(^books.byTag(\"x\"))\n",
    );
    let store = TreeStore::memory();
    let by_tag = index_catalog_id(&program, "books", "byTag");
    store
        .write_index_entry(
            &by_tag,
            &[
                SavedKey::Str("x".into()),
                SavedKey::Int(3),
                SavedKey::Int(7),
            ],
            &[SavedKey::Int(3), SavedKey::Int(7)],
            Vec::new(),
        )
        .expect("valid non-unique index entry");
    store
        .write_index_entry(
            &by_tag,
            &[
                SavedKey::Str("x".into()),
                SavedKey::Int(3),
                SavedKey::Int(7),
            ],
            &[SavedKey::Int(3), SavedKey::Int(8)],
            Vec::new(),
        )
        .expect("corrupt non-unique index sibling");

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::countTaggedBooks")),
        RUN_TYPE,
    );
}

#[test]
fn partial_enum_presence_rejects_corrupt_sibling_after_valid_branch() {
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
         pub fn seed()\n\
         \x20   var book: Book\n\
         \x20   book.title = \"Published\"\n\
         \x20   book.status = Status::published\n\
         \x20   ^books(1) = book\n\
         \n\
         pub fn hasStatusBranch(): bool\n\
         \x20   return exists(^books.byStatus)\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let corrupt_status = format!(
        "{}~",
        enum_member_catalog_id(&program, "Status", "published").as_str()
    );
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byStatus"),
            &[SavedKey::Str(corrupt_status), SavedKey::Int(2)],
            &[SavedKey::Int(2)],
            Vec::new(),
        )
        .expect("corrupt enum index sibling");

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::hasStatusBranch")),
        RUN_TYPE,
    );
}

#[test]
fn unique_index_rejects_physical_identity_payload_mismatch() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   isbn: string\n\
         store ^books(id: int): Book\n\
         \x20   index byIsbn(isbn) unique\n\
         \n\
         pub fn idByIsbn(isbn: string): Id(^books)\n\
         \x20   return ^books.byIsbn(isbn) ?? Id(^books, 0)\n\
         \n\
         pub fn hasIsbn(isbn: string): bool\n\
         \x20   return exists(^books.byIsbn(isbn))\n\
         \n\
         pub fn countIsbn(isbn: string): int\n\
         \x20   return count(^books.byIsbn(isbn))\n\
         \n\
         pub fn printByIsbn(isbn: string)\n\
         \x20   for id in ^books.byIsbn(isbn)\n\
         \x20       print(id)\n",
    );
    let store = TreeStore::memory();
    store
        .write_index_entry(
            &index_catalog_id(&program, "books", "byIsbn"),
            &[SavedKey::Str("978-0".into())],
            &[SavedKey::Int(2)],
            encode_identity_payload(&[SavedKey::Int(1)]),
        )
        .expect("corrupt unique index identity");

    for entry in [
        "test::idByIsbn",
        "test::hasIsbn",
        "test::countIsbn",
        "test::printByIsbn",
    ] {
        assert_run_error(
            run_entry(
                &store,
                checked_entry!(&program, entry, Value::Str("978-0".into())),
            ),
            RUN_TYPE,
        );
    }
}

#[test]
fn unique_index_rejects_duplicate_physical_entries_for_one_tuple() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   isbn: string\n\
         store ^books(id: int): Book\n\
         \x20   index byIsbn(isbn) unique\n\
         \n\
         pub fn idByIsbn(isbn: string): Id(^books)\n\
         \x20   return ^books.byIsbn(isbn) ?? Id(^books, 0)\n\
         \n\
         pub fn hasIsbn(isbn: string): bool\n\
         \x20   return exists(^books.byIsbn(isbn))\n",
    );
    let store = TreeStore::memory();
    let by_isbn = index_catalog_id(&program, "books", "byIsbn");
    store
        .write_index_entry(
            &by_isbn,
            &[SavedKey::Str("978-0".into())],
            &[SavedKey::Int(1)],
            encode_identity_payload(&[SavedKey::Int(1)]),
        )
        .expect("first unique index entry");
    store
        .write_index_entry(
            &by_isbn,
            &[SavedKey::Str("978-0".into())],
            &[SavedKey::Int(2)],
            encode_identity_payload(&[SavedKey::Int(2)]),
        )
        .expect("duplicate unique index entry");

    for entry in ["test::idByIsbn", "test::hasIsbn"] {
        assert_run_error(
            run_entry(
                &store,
                checked_entry!(&program, entry, Value::Str("978-0".into())),
            ),
            RUN_TYPE,
        );
    }
}

#[test]
fn unique_index_over_identity_field_rejects_conflicts() {
    let program = checked_program(
        "resource Author\n\
         \x20   required name: string\n\
         store ^authors(id: int): Author\n\
         \n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\
         \x20   index oneBookByAuthor(authorId) unique\n\
         \n\
         pub fn conflict()\n\
         \x20   const ann = Id(^authors, 1)\n\
         \x20   ^authors(ann).name = \"Ann\"\n\
         \x20   ^books(1).title = \"A\"\n\
         \x20   ^books(1).authorId = ann\n\
         \x20   ^books(2).title = \"B\"\n\
         \x20   ^books(2).authorId = ann\n",
    );
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::conflict")),
        "write.unique_conflict",
    );
}

// --- Composite-identity index traversal ---

#[test]
fn traverses_a_composite_identity_index() {
    let program = checked_program(ENROLLMENT_STATUS);
    let store = TreeStore::memory();
    let enroll = |s: &str, c: &str, st: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::enroll",
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ),
        )
        .expect("enroll");
    };
    enroll("student-1", "course-8", "active");
    enroll("student-1", "course-9", "active");
    enroll("student-1", "course-7", "dropped");

    // Each reconstructed identity addresses its record: every active enrollment
    // reads back `active`. Two such entries exist, in (studentId, courseId) order.
    let outcome = run_entry(&store, checked_entry!(&program, "test::activeStatuses")).expect("run");
    assert_eq!(outcome.output, "active\nactive\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCoursesForStudent",
            Value::Str("student-1".into())
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "course-8\ncourse-9\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExact",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "course-8\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExactPair",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "course-8:course-8\n");

    let outcome = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExactKeys",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .expect("run");
    assert_eq!(outcome.output, "course-8\n");

    let exact_count = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExactCount",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
        ),
    )
    .expect("count")
    .value;
    assert_eq!(exact_count, Some(Value::Int(1)));

    let inactive_count = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::activeCourseExactCount",
            Value::Str("student-1".into()),
            Value::Str("course-7".into()),
        ),
    )
    .expect("count")
    .value;
    assert_eq!(inactive_count, Some(Value::Int(0)));
}

#[test]
fn helper_mutating_a_traversed_composite_index_faults_at_runtime() {
    let program = checked_program(
        "resource Enrollment\n    required status: string\n    required student: string\n    required course: string\nstore ^enrollments(studentId: string, courseId: string): Enrollment\n\n    index byStatus(status, studentId, courseId)\n\npub fn enroll(s: string, c: string, st: string)\n    var enrollment: Enrollment\n    enrollment.status = st\n    enrollment.student = s\n    enrollment.course = c\n    ^enrollments(s, c) = enrollment\n\npub fn markInactive(id: Id(^enrollments))\n    ^enrollments(id).status = \"inactive\"\n\npub fn deactivateExact(student: string, course: string)\n    for id in ^enrollments.byStatus(\"active\", student, course)\n        markInactive(id)\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::enroll",
            Value::Str("student-1".into()),
            Value::Str("course-8".into()),
            Value::Str("active".into()),
        ),
    )
    .expect("enroll");
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::deactivateExact",
                Value::Str("student-1".into()),
                Value::Str("course-8".into()),
            ),
        ),
        RUN_TRAVERSAL,
    );
}

#[test]
fn direct_composite_identity_index_loop_yields_identities() {
    let program = checked_program(ENROLLMENT_STATUS);
    let store = TreeStore::memory();
    let enroll = |s: &str, c: &str, st: &str| {
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::enroll",
                Value::Str(s.into()),
                Value::Str(c.into()),
                Value::Str(st.into()),
            ),
        )
        .expect("enroll");
    };
    enroll("student-1", "course-8", "active");
    enroll("student-1", "course-9", "active");
    enroll("student-1", "course-7", "dropped");

    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::activeEnrollmentsDirect"),
    )
    .expect("run");
    assert_eq!(outcome.output, "student-1:course-8\nstudent-1:course-9\n");
}
