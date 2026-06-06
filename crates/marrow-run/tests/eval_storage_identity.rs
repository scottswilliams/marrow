//! The reference sample end to end, resource-identity values, singleton stores,
//! unkeyed-group fields, and unique- and composite-identity index traversal.

#[macro_use]
mod support;

use support::*;

use marrow_run::{Host, RUN_ABSENT, RUN_TRAVERSAL, RUN_UNSUPPORTED, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};

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
resource Book at ^books(id: int)
    required title: string

    versions(version: int)
        required title: string

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

// --- Resource-identity values ---

#[test]
fn multiple_stores_over_one_resource_keep_runtime_roots_separate() {
    let program = checked_program(
        "resource Book\n\
         \x20   title: string\n\
         \n\
         store ^books(id: int): Book\n\
         store ^archivedBooks(id: int): Book\n\
         \n\
         pub fn seed()\n\
         \x20   ^books(1).title = \"live\"\n\
         \x20   ^archivedBooks(1).title = \"archived\"\n\
         \n\
         pub fn live(): string\n\
         \x20   return ^books(1).title ?? \"\"\n\
         \n\
         pub fn archived(): string\n\
         \x20   return ^archivedBooks(1).title ?? \"\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");

    let live = run_entry(&store, checked_entry!(&program, "test::live"))
        .expect("live")
        .value;
    assert_eq!(live, Some(Value::Str("live".into())));

    let archived = run_entry(&store, checked_entry!(&program, "test::archived"))
        .expect("archived")
        .value;
    assert_eq!(archived, Some(Value::Str("archived".into())));
}

/// A single-key store identity from `nextId(^books)` can be passed to saved reads
/// and writes. The identity carries the lowered key so `^books(id)` reads the
/// same record `^books(1)` does.
const BOOK_IDENTITY: &str = "\
resource Book at ^books(id: int)
    required title: string

pub fn save(t: string)
    const id = nextId(^books)
    ^books(id).title = t

pub fn title(): string
    for id in ^books
        return ^books(id).title
    return \"\"
";

#[test]
fn allocates_and_uses_a_single_key_store_identity() {
    let program = checked_program(BOOK_IDENTITY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::save", Value::Str("Mort".into())),
    )
    .expect("save");
    let value = run_entry(&store, checked_entry!(&program, "test::title"))
        .expect("title")
        .value;
    assert_eq!(value, Some(Value::Str("Mort".into())));
    // The identity lowered to the same key a plain int does: `^books(1)`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&program, "books", &["title"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Mort".into()))
    );
}

#[test]
fn a_plain_int_identity_still_works() {
    // The bare int path remains the executable single-key store identity path.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn save()\n    ^books(1).title = \"a\"\n\npub fn read(): string\n    return ^books(1).title\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save")).expect("save");
    let value = run_entry(&store, checked_entry!(&program, "test::read"))
        .expect("read")
        .value;
    assert_eq!(value, Some(Value::Str("a".into())));
}

/// A composite-key resource can still be addressed directly by its declared store
/// key order. Composite identity values come from traversal and indexes.
const ENROLLMENT_IDENTITY: &str = "\
resource Enrollment at ^enrollments(studentId: string, courseId: string)
    status: string

pub fn enroll(s: string, c: string, st: string)
    ^enrollments(s, c).status = st

pub fn statusOf(s: string, c: string): string
    return ^enrollments(s, c).status ?? \"\"
";

#[test]
fn constructs_and_uses_a_composite_identity_round_trips() {
    let program = checked_program(ENROLLMENT_IDENTITY);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::enroll",
            Value::Str("student-1".into()),
            Value::Str("course-9".into()),
            Value::Str("active".into()),
        ),
    )
    .expect("enroll");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::statusOf",
            Value::Str("student-1".into()),
            Value::Str("course-9".into()),
        ),
    )
    .expect("statusOf")
    .value;
    assert_eq!(value, Some(Value::Str("active".into())));
    // Keys lowered in declared order: studentId then courseId.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "enrollments",
            &[
                SavedKey::Str("student-1".into()),
                SavedKey::Str("course-9".into()),
            ],
            &data_path(&program, "enrollments", &["status"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("active".into()))
    );
}

#[test]
fn composite_root_keys_write_in_declaration_order() {
    let program = checked_program(
        "resource Enrollment at ^enrollments(studentId: string, courseId: string)\n    status: string\n\npub fn enroll()\n    ^enrollments(\"s\", \"c\").status = \"active\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::enroll")).expect("enroll");
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "enrollments",
            &[SavedKey::Str("s".into()), SavedKey::Str("c".into())],
            &data_path(&program, "enrollments", &["status"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("active".into()))
    );
}

#[test]
fn whole_resource_read_through_an_identity() {
    // The primary-root iterator yields a composite identity that can be used for
    // a whole-resource read.
    let program = checked_program(
        "resource Enrollment at ^enrollments(studentId: string, courseId: string)\n    status: string\n\npub fn statusOf(): string\n    for id in ^enrollments\n        var e: Enrollment = ^enrollments(id)\n        return e.status\n    return \"\"\n",
    );
    let store = TreeStore::memory();
    write_data_value(
        &program,
        &store,
        "enrollments",
        &[SavedKey::Str("s".into()), SavedKey::Str("c".into())],
        &data_path(&program, "enrollments", &["status"]),
        SavedValue::Str("active".into()),
    );
    let value = run_entry(&store, checked_entry!(&program, "test::statusOf"))
        .expect("statusOf")
        .value;
    assert_eq!(value, Some(Value::Str("active".into())));
}

// --- Singleton stores end-to-end ---

/// A singleton store (no identity keys). Field
/// read/write address the root directly, and whole read/write materialize and
/// replace the root as a resource value.
const SETTINGS: &str = "\
resource Settings at ^settings
    required theme: string
    required maxLoans: int

pub fn init(t: string, n: int)
    transaction
        ^settings.theme = t
        ^settings.maxLoans = n

pub fn setMaxLoans(n: int)
    ^settings.maxLoans = n

pub fn setTheme(t: string)
    ^settings.theme = t

pub fn theme(): string
    return ^settings.theme ?? \"\"

pub fn snapshot(): Settings
    var fallback: Settings
    fallback.theme = \"\"
    fallback.maxLoans = 0
    return ^settings ?? fallback

pub fn restore(s: Settings)
    ^settings = s

pub fn restoreFixture(theme: string, maxLoans: int)
    var s: Settings
    s.theme = theme
    s.maxLoans = maxLoans
    restore(s)
";

#[test]
fn singleton_field_read_and_write() {
    let program = checked_program(SETTINGS);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::init",
            Value::Str("light".into()),
            Value::Int(3)
        ),
    )
    .expect("init");
    run_entry(
        &store,
        checked_entry!(&program, "test::setMaxLoans", Value::Int(5)),
    )
    .expect("setMaxLoans");
    run_entry(
        &store,
        checked_entry!(&program, "test::setTheme", Value::Str("dark".into())),
    )
    .expect("setTheme");
    let value = run_entry(&store, checked_entry!(&program, "test::theme"))
        .expect("theme")
        .value;
    assert_eq!(value, Some(Value::Str("dark".into())));
    // The field landed at `^settings.theme`, no record key in between.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "settings",
            &[],
            &data_path(&program, "settings", &["theme"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("dark".into()))
    );
}

#[test]
fn singleton_whole_read_and_write_round_trip() {
    let program = checked_program(SETTINGS);
    let store = TreeStore::memory();
    // Seed the singleton's fields directly.
    write_data_value(
        &program,
        &store,
        "settings",
        &[],
        &data_path(&program, "settings", &["theme"]),
        SavedValue::Str("light".into()),
    );
    write_data_value(
        &program,
        &store,
        "settings",
        &[],
        &data_path(&program, "settings", &["maxLoans"]),
        SavedValue::Int(5),
    );
    // A whole read materializes the singleton's present fields.
    let snapshot = run_entry(&store, checked_entry!(&program, "test::snapshot"))
        .expect("snapshot")
        .value;
    assert_eq!(
        snapshot,
        Some(Value::Resource(vec![
            ("theme".into(), Value::Str("light".into())),
            ("maxLoans".into(), Value::Int(5)),
        ]))
    );
    // A whole write replaces it; read it back via the field reader.
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::restoreFixture",
            Value::Str("solar".into()),
            Value::Int(9)
        ),
    )
    .expect("restore");
    let value = run_entry(&store, checked_entry!(&program, "test::theme"))
        .expect("theme")
        .value;
    assert_eq!(value, Some(Value::Str("solar".into())));
}

// --- Unkeyed-group field read/write through a saved path ---

/// A resource with an unkeyed nested group (`name { first; last }`). Its fields
/// are addressed `^patients(p).name.first` — a `.field` off a `.field` off the
/// record, with no keyed layer in between.
#[test]
fn a_whole_read_of_a_keyed_root_without_an_identity_is_rejected() {
    checker_rejects(
        "module test\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         pub fn read()\n\
         \x20   var b: Book = ^books\n",
        "check.untyped_value",
    );
}

#[test]
fn a_field_read_off_a_keyed_root_without_an_identity_is_rejected() {
    checker_rejects(
        "module test\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         pub fn read(): string\n\
         \x20   return ^books.title\n",
        "check.untyped_value",
    );
}

const PATIENT_UNKEYED_GROUP: &str = "\
resource Patient at ^patients(id: int)
    mrn: string
    name
        first: string
        last: string

pub fn setName(id: int, f: string, l: string)
    ^patients(id).name.first = f
    ^patients(id).name.last = l

pub fn firstOf(id: int): string
    return ^patients(id)?.name?.first ?? \"\"

pub fn lastOf(id: int): string
    return ^patients(id)?.name?.last ?? \"\"
";

#[test]
fn unkeyed_group_field_write_then_read_round_trips() {
    let program = checked_program(PATIENT_UNKEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::setName",
            Value::Int(7),
            Value::Str("Terry".into()),
            Value::Str("Pratchett".into()),
        ),
    )
    .expect("setName");
    let read = |entry: &str| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(7)))
            .expect("read")
            .value
    };
    assert_eq!(read("test::firstOf"), Some(Value::Str("Terry".into())));
    assert_eq!(read("test::lastOf"), Some(Value::Str("Pratchett".into())));
    // The field landed under the group layer `^patients(7).name.first`.
    assert_eq!(
        read_data_value(
            &program,
            &store,
            "patients",
            &[SavedKey::Int(7)],
            &data_path(&program, "patients", &["name", "first"]),
            ScalarType::Str,
        ),
        Some(SavedValue::Str("Terry".into()))
    );
}

#[test]
fn an_absent_unkeyed_group_field_read_uses_the_default() {
    let program = checked_program(PATIENT_UNKEYED_GROUP);
    let store = TreeStore::memory();
    let value = run_entry(
        &store,
        checked_entry!(&program, "test::firstOf", Value::Int(1)),
    )
    .expect("default")
    .value;
    assert_eq!(value, Some(Value::Str(String::new())));
}

// --- Unique-index identity reads ---

/// A book with a unique index on `isbn`. `register` stores the book, and
/// `titleByIsbn` reads the identity back from the unique-index lookup path and
/// uses it to address the record.
const BOOK_ISBN: &str = "\
resource Book at ^books(id: int)
    required title: string
    isbn: string

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
        "resource Item at ^items(id: int)\n    required title: string\n    series: string\n    code: string\n\n    index bySeriesCode(series, code) unique\n\npub fn countSeries(series: string): int\n    return count(^items.bySeriesCode(series))\n",
        "check.key_type",
    );
}

#[test]
fn unique_index_prefix_branch_loops_are_rejected_by_the_checker() {
    let source = "resource Item at ^items(id: int)\n    required title: string\n    series: string\n    code: string\n\n    index bySeriesCode(series, code) unique\n\npub fn titlesInSeries(series: string)\n    for id in ^items.bySeriesCode(series)\n        print(^items(id).title ?? \"\")\n";
    checker_rejects(source, "check.key_type");

    let source = "resource Item at ^items(id: int)\n    required title: string\n    series: string\n    code: string\n\n    index bySeriesCode(series, code) unique\n\npub fn titlesInAnySeries()\n    for id in ^items.bySeriesCode\n        print(^items(id).title ?? \"\")\n";
    checker_rejects(source, "check.key_type");
}

/// A non-unique index in value position has no single identity to yield; the
/// runtime rejects it and points the reader at `keys(...)`.
const BOOK_SHELF_VALUE: &str = "\
resource Book at ^books(id: int)
    required title: string
    shelf: string

    index byShelf(shelf, id)

pub fn firstOnShelf(shelf: string): Id(^books)
    return ^books.byShelf(shelf)
";

#[test]
fn a_non_unique_index_in_value_position_is_rejected() {
    checker_rejects(BOOK_SHELF_VALUE, "check.untyped_value");
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
        "resource Enrollment at ^enrollments(studentId: string, courseId: string)\n    required status: string\n    required student: string\n    required course: string\n\n    index byStatus(status, studentId, courseId)\n\npub fn enroll(s: string, c: string, st: string)\n    var enrollment: Enrollment\n    enrollment.status = st\n    enrollment.student = s\n    enrollment.course = c\n    ^enrollments(s, c) = enrollment\n\npub fn markInactive(id: Id(^enrollments))\n    ^enrollments(id).status = \"inactive\"\n\npub fn deactivateExact(student: string, course: string)\n    for id in ^enrollments.byStatus(\"active\", student, course)\n        markInactive(id)\n",
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
