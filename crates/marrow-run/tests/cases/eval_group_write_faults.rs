//! Cross-call-boundary write-fault recovery and dotted-code propagation, and
//! unkeyed-group delete contracts: sparse no-op, required-field rejection, and
//! maintenance override.
//!
//! Also covers typed keyed-resource entry-write contracts: required-field
//! rejection on entry write, empty-entry presence preservation, and read
//! materialization of values written through plain and nested entry writes.

use crate::support;
use support::*;

use marrow_run::{Host, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

const BOOK_ISBN_SAVE: &str = "\
module test
resource Book
    isbn: string
store ^books(id: int): Book
    index byIsbn(isbn) unique
fn save(i: int, code: string)
    ^books(i).isbn = code
";

#[test]
fn a_recoverable_write_fault_is_catchable_across_a_call_boundary() {
    // A write fault raised in a CALLED function must be catchable by the caller's
    // try/catch (the transaction-recovery contract), not only within the same frame.
    let program = checked_program(&format!(
        "{BOOK_ISBN_SAVE}\
         pub fn run(): string\n\
         \x20   save(1, \"x\")\n\
         \x20   try\n\
         \x20       save(2, \"x\")\n\
         \x20       return \"uncaught\"\n\
         \x20   catch e: Error\n\
         \x20       return e.code\n"
    ));
    let store = TreeStore::memory();
    let value = run_entry(&store, checked_entry!(&program, "test::run"))
        .expect("run")
        .value;
    assert_eq!(value, Some(Value::Str("write.unique_conflict".into())));
}

#[test]
fn an_uncaught_cross_boundary_write_fault_keeps_its_dotted_code() {
    // Crossing a call boundary must not collapse an uncaught fault to
    // run.uncaught_error: it surfaces with its own dotted code (and exit code).
    let program = checked_program(&format!(
        "{BOOK_ISBN_SAVE}\
         pub fn run()\n\
         \x20   save(1, \"x\")\n\
         \x20   save(2, \"x\")\n"
    ));
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::run")),
        "write.unique_conflict",
    );
}

const PATIENT_SPARSE_GROUP: &str = "\
module test
resource Patient
    name
        first: string
        last: string
store ^patients(id: string): Patient
";

const PATIENT_REQUIRED_GROUP: &str = "\
module test
resource Patient
    name
        required first: string
        last: string
store ^patients(id: string): Patient
";

const PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP: &str = "\
module test
resource Patient
    visits(pos: int)
        name
            required first: string
            last: string
store ^patients(id: string): Patient

pub fn seed()
    ^patients(\"p1\").visits(1).name.first = \"Sam\"
    ^patients(\"p1\").visits(1).name.last = \"Vimes\"

pub fn drop()
    delete ^patients(\"p1\").visits(1).name

pub fn visit_first(): string
    return ^patients(\"p1\").visits(1).name.first ?? \"\"
";

const POST_TYPED_KEYED_COMMENTS: &str = "\
module test
resource Reply
    required body: string
resource Comment
    required body: string
    meta
        author: string
    replies(seq: int): Reply
store ^comments(id: int): Comment
resource Post
    comments(seq: int): Comment
store ^posts(id: int): Post

pub fn save_missing_body()
    var comment: Comment
    ^posts(1).comments(1) = comment

pub fn save_complete()
    ^comments(1).body = \"hello\"
    ^comments(1).meta.author = \"Ann\"
    if exists(^comments(1))
        ^posts(1).comments(1) = ^comments(1)

pub fn read_comment(): Comment
    var fallback: Comment
    fallback.body = \"missing\"
    return ^posts(1).comments(1) ?? fallback

pub fn save_nested_reply()
    ^comments(1).body = \"root\"
    ^comments(1).meta.author = \"Ann\"
    if exists(^comments(1))
        ^posts(1).comments(1) = ^comments(1)
    var reply: Reply
    reply.body = \"hi\"
    ^posts(1).comments(1).replies(1) = reply

pub fn save_nested_reply_without_parent_body()
    var reply: Reply
    reply.body = \"hi\"
    ^posts(1).comments(1).replies(1) = reply

pub fn read_nested_reply(): Reply
    var fallback: Reply
    fallback.body = \"missing\"
    return ^posts(1).comments(1).replies(1) ?? fallback
";

const POST_TYPED_KEYED_MARKERS: &str = "\
module test
resource Marker
    note: string
resource Post
    markers(seq: int): Marker
store ^posts(id: int): Post

pub fn save_empty_marker()
    var marker: Marker
    ^posts(1).markers(1) = marker

pub fn marker_exists(): bool
    return exists(^posts(1).markers(1))

pub fn read_marker(): Marker
    var fallback: Marker
    fallback.note = \"missing\"
    return ^posts(1).markers(1) ?? fallback

pub fn marker_count(): int
    var total = 0
    for seq in keys(^posts(1).markers)
        total = total + 1
    return total
";

#[test]
fn deleting_a_sparse_field_inside_an_unkeyed_group_is_allowed() {
    // Field delete descends unkeyed-group layers. Sparse descendants may still be
    // deleted independently.
    let program = checked_program(&format!(
        "{PATIENT_SPARSE_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name.last\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::drop"))
        .expect("sparse group-field delete is a no-op");
}

#[test]
fn deleting_a_required_field_inside_an_unkeyed_group_is_rejected() {
    let program = checked_program(&format!(
        "{PATIENT_REQUIRED_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name.first\n"
    ));
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert_run_error(result, "write.required_field");
}

#[test]
fn deleting_an_unkeyed_group_with_required_descendants_is_rejected() {
    let program = checked_program(&format!(
        "{PATIENT_REQUIRED_GROUP}\
         pub fn drop()\n\
         \x20   delete ^patients(\"p1\").name\n"
    ));
    let store = TreeStore::memory();
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert_run_error(result, "write.required_field");
}

#[test]
fn deleting_a_nested_unkeyed_group_with_required_descendants_is_rejected() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let result = run_entry(&store, checked_entry!(&program, "test::drop"));
    assert_run_error(result, "write.required_field");
}

#[test]
fn maintenance_can_delete_a_nested_unkeyed_group_with_required_descendants() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let host = Host::new().with_maintenance();
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::drop")).expect("drop");
    for field in ["first", "last"] {
        assert_eq!(
            read_data_bytes(
                &program,
                &store,
                "patients",
                &[SavedKey::Str("p1".into())],
                &keyed_data_path(
                    &program,
                    "patients",
                    &[("visits", vec![SavedKey::Int(1)])],
                    &["name", field],
                ),
            ),
            None,
            "{field} removed under maintenance"
        );
    }
}

#[test]
fn keyed_group_entry_read_materializes_unkeyed_group_descendants() {
    let program = checked_program(PATIENT_REQUIRED_GROUP_UNDER_KEYED_GROUP);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let outcome = run_entry(&store, checked_entry!(&program, "test::visit_first")).expect("read");
    assert_eq!(outcome.value, Some(Value::Str("Sam".into())));
}

#[test]
fn typed_keyed_resource_entry_write_requires_required_fields() {
    let program = checked_program(POST_TYPED_KEYED_COMMENTS);
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::save_missing_body")),
        "write.required_absent",
    );
}

#[test]
fn typed_keyed_resource_entry_write_read_materializes_value() {
    let program = checked_program(POST_TYPED_KEYED_COMMENTS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save_complete")).expect("save");
    let outcome = run_entry(&store, checked_entry!(&program, "test::read_comment")).expect("read");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![
            ("body".into(), Value::Str("hello".into())),
            (
                "meta".into(),
                Value::Resource(vec![("author".into(), Value::Str("Ann".into()))])
            ),
        ]))
    );
}

#[test]
fn typed_keyed_resource_entry_write_preserves_empty_entry_presence() {
    let program = checked_program(POST_TYPED_KEYED_MARKERS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save_empty_marker")).expect("save");

    let exists =
        run_entry(&store, checked_entry!(&program, "test::marker_exists")).expect("exists");
    assert_eq!(exists.value, Some(Value::Bool(true)));

    let marker = run_entry(&store, checked_entry!(&program, "test::read_marker")).expect("read");
    assert_eq!(marker.value, Some(Value::Resource(vec![])));

    let count = run_entry(&store, checked_entry!(&program, "test::marker_count")).expect("count");
    assert_eq!(count.value, Some(Value::Int(1)));
}

#[test]
fn nested_typed_keyed_resource_entry_write_read_materializes_value() {
    let program = checked_program(POST_TYPED_KEYED_COMMENTS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::save_nested_reply")).expect("save");
    let outcome =
        run_entry(&store, checked_entry!(&program, "test::read_nested_reply")).expect("read");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![(
            "body".into(),
            Value::Str("hi".into())
        )]))
    );
}

#[test]
fn nested_typed_keyed_resource_entry_write_requires_parent_required_fields() {
    let program = checked_program(POST_TYPED_KEYED_COMMENTS);
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::save_nested_reply_without_parent_body"),
        ),
        "write.required_absent",
    );
    let outcome =
        run_entry(&store, checked_entry!(&program, "test::read_nested_reply")).expect("read");
    assert_eq!(
        outcome.value,
        Some(Value::Resource(vec![(
            "body".into(),
            Value::Str("missing".into())
        )]))
    );
}
