//! Identity values reconstructed from index traversal: unique-index lookups in
//! value position, and composite-identity index loops.

#[macro_use]
mod support;

use support::*;

use marrow_run::{RUN_TRAVERSAL, RUN_UNSUPPORTED, Value};
use marrow_store::tree::TreeStore;

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
