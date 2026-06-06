//! Saved scalar reads and writes, transaction commit and required-field rules,
//! exists/coalesce presence, optional chains, and next-id allocation.

#[macro_use]
mod support;

use support::*;

use marrow_check::CheckedRuntimeProgram;
use marrow_run::{RUN_ABSENT, RUN_TYPE, RUN_UNCAUGHT_THROW, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

#[test]
fn print_writes_a_line_to_output() {
    let program = checked_program("pub fn main()\n    print($\"hello {1}\")\n");
    let outcome = run_full(checked_entry!(&program, "test::main")).expect("run");
    assert_eq!(outcome.value, None);
    assert_eq!(outcome.output, "hello 1\n");
}

#[test]
fn write_does_not_add_a_newline() {
    let program = checked_program("pub fn main()\n    write(\"a\")\n    write(\"b\")\n");
    let outcome = run_full(checked_entry!(&program, "test::main")).expect("run");
    assert_eq!(outcome.output, "ab");
}

#[test]
fn output_accumulates_across_calls() {
    let program = checked_program(
        "pub fn greet(name: string)\n    print($\"hi {name}\")\n\npub fn main()\n    greet(\"a\")\n    greet(\"b\")\n",
    );
    let outcome = run_full(checked_entry!(&program, "test::main")).expect("run");
    assert_eq!(outcome.output, "hi a\nhi b\n");
}

#[test]
fn print_takes_one_argument() {
    let program = checked_program("pub fn main()\n    print()\n");
    let result = run_full(checked_entry!(&program, "test::main"));
    assert_run_error(result, RUN_TYPE);
}

/// A program with a saved `Book` resource and functions that read a title.
const BOOK_READER: &str = "\
resource Book at ^books(id: int)
    required title: string

pub fn title_of(id: int): string
    return ^books(id).title

pub fn show(id: int)
    print($\"title: {^books(id).title}\")
";

fn store_with_title(program: &CheckedRuntimeProgram, id: i64, title: &str) -> TreeStore {
    let store = empty_store();
    write_data_value(
        program,
        &store,
        "books",
        &[SavedKey::Int(id)],
        &data_path(program, "books", &["title"]),
        SavedValue::Str(title.into()),
    );
    store
}

#[test]
fn reads_a_scalar_field_from_saved_data() {
    let program = checked_program(BOOK_READER);
    let store = store_with_title(&program, 1, "Mort");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::title_of", Value::Int(1)),
    )
    .expect("run");
    assert_eq!(outcome.value, Some(Value::Str("Mort".into())));
}

#[test]
fn reading_an_absent_field_is_an_error() {
    let program = checked_program(BOOK_READER);
    let store = TreeStore::memory(); // empty: the title is absent
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::title_of", Value::Int(1)),
    );
    assert_run_error(result, RUN_ABSENT);
}

#[test]
fn a_saved_read_interpolates_and_prints() {
    let program = checked_program(BOOK_READER);
    let store = store_with_title(&program, 7, "Mort");
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::show", Value::Int(7)),
    )
    .expect("run");
    assert_eq!(outcome.output, "title: Mort\n");
}

#[test]
fn whole_resource_read_rejects_missing_required_durable_fields() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n    required title: string\n    required shelf: string\n\npub fn read(id: int): Book\n    var fallback: Book\n    fallback.title = \"\"\n    fallback.shelf = \"\"\n    return ^books(id) ?? fallback\n",
    );
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["shelf"]),
        SavedValue::Str("fiction".into()),
    );
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::read", Value::Int(1)),
    );

    assert_run_error(result, RUN_TYPE);
}

/// A program that writes and reads a `Book` title.
const BOOK_WRITER: &str = "\
resource Book at ^books(id: int)
    required title: string

pub fn set_title(id: int, t: string)
    ^books(id).title = t

pub fn title_of(id: int): string
    return ^books(id).title
";

#[test]
fn a_field_write_updates_saved_data() {
    let program = checked_program(BOOK_WRITER);
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
    // Read it back through the runtime against the same store.
    let outcome = run_entry(
        &store,
        checked_entry!(&program, "test::title_of", Value::Int(1)),
    )
    .expect("read");
    assert_eq!(outcome.value, Some(Value::Str("Mort".into())));
}

#[test]
fn out_of_transaction_field_write_rejects_partial_required_record() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n\
         \x20   required name: string\n\
         \x20   shelf: string\n\n\
         pub fn set_shelf(id: int)\n\
         \x20   ^items(id).shelf = \"fiction\"\n\n\
         pub fn has_item(id: int): bool\n\
         \x20   return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::set_shelf", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the rejected sparse write must leave no partial record"
    );
}

#[test]
fn out_of_transaction_group_field_write_rejects_partial_required_record() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   binding\n\
         \x20       cover: string\n\n\
         pub fn set_cover(id: int)\n\
         \x20   ^books(id).binding.cover = \"hard\"\n\n\
         pub fn has_book(id: int): bool\n\
         \x20   return exists(^books(id))\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::set_cover", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(1))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the rejected group-field write must leave no partial record"
    );
}

#[test]
fn transaction_commit_rejects_partial_required_record() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n\
         \x20   required name: string\n\
         \x20   shelf: string\n\n\
         pub fn set_shelf(id: int)\n\
         \x20   transaction\n\
         \x20       ^items(id).shelf = \"fiction\"\n\n\
         pub fn has_item(id: int): bool\n\
         \x20   return exists(^items(id))\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::set_shelf", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the rejected transaction must roll back the partial record"
    );
}

#[test]
fn transaction_required_field_checks_cross_helper_calls() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n\
         \x20   required name: string\n\
         \x20   shelf: string\n\n\
         pub fn set_shelf(id: int)\n\
         \x20   ^items(id).shelf = \"fiction\"\n\n\
         pub fn create(id: int)\n\
         \x20   transaction\n\
         \x20       set_shelf(id)\n\
         \x20       ^items(id).name = \"Mort\"\n\n\
         pub fn name_of(id: int): string\n\
         \x20   return ^items(id).name\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::create", Value::Int(1)),
    )
    .expect("commit");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::name_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into()))
    );
}

#[test]
fn nested_transaction_defers_required_check_until_outer_commit() {
    let program = checked_program(
        "resource Item at ^items(id: int)\n\
         \x20   required name: string\n\
         \x20   shelf: string\n\n\
         pub fn create(id: int)\n\
         \x20   transaction\n\
         \x20       transaction\n\
         \x20           ^items(id).shelf = \"fiction\"\n\
         \x20       ^items(id).name = \"Mort\"\n\n\
         pub fn name_of(id: int): string\n\
         \x20   return ^items(id).name\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::create", Value::Int(1)),
    )
    .expect("commit");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::name_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("Mort".into()))
    );
}

#[test]
fn transaction_commit_metadata_reports_every_touched_root_and_index() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   shelf: string\n\
         \x20   index byShelf(shelf, id)\n\n\
         resource Audit at ^audits(id: int)\n\
         \x20   required message: string\n\n\
         pub fn save()\n\
         \x20   transaction\n\
         \x20       ^books(1).title = \"Mort\"\n\
         \x20       ^books(1).shelf = \"fiction\"\n\
         \x20       ^audits(1).message = \"created\"\n",
    );
    let books = store_catalog_id(&program, "books");
    let audits = store_catalog_id(&program, "audits");
    let by_shelf = index_catalog_id(&program, "books", "byShelf");
    let store = TreeStore::memory();

    run_entry(&store, checked_entry!(&program, "test::save")).expect("transaction commits");

    let commit = store
        .read_commit_metadata()
        .expect("read commit metadata")
        .expect("commit metadata is stamped");
    assert_eq!(commit.commit_id, 1);
    assert_eq!(commit.source_digest, program.source_digest());
    assert!(
        commit.changed_root_catalog_ids.contains(&books),
        "books root missing from commit metadata: {commit:#?}"
    );
    assert!(
        commit.changed_root_catalog_ids.contains(&audits),
        "audits root missing from commit metadata: {commit:#?}"
    );
    assert_eq!(
        commit.changed_root_catalog_ids.len(),
        2,
        "commit metadata should report each changed root once: {commit:#?}"
    );
    assert_eq!(commit.changed_index_catalog_ids, vec![by_shelf]);
}

#[test]
fn nested_transaction_commit_metadata_reports_the_outer_durable_commit() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   shelf: string\n\
         \x20   index byShelf(shelf, id)\n\n\
         resource Audit at ^audits(id: int)\n\
         \x20   required message: string\n\n\
         pub fn save()\n\
         \x20   transaction\n\
         \x20       ^books(1).title = \"Mort\"\n\
         \x20       transaction\n\
         \x20           ^audits(1).message = \"created\"\n\
         \x20       ^books(1).shelf = \"fiction\"\n",
    );
    let books = store_catalog_id(&program, "books");
    let audits = store_catalog_id(&program, "audits");
    let by_shelf = index_catalog_id(&program, "books", "byShelf");
    let store = TreeStore::memory();

    run_entry(&store, checked_entry!(&program, "test::save")).expect("transaction commits");

    let commit = store
        .read_commit_metadata()
        .expect("read commit metadata")
        .expect("commit metadata is stamped");
    assert_eq!(commit.commit_id, 1);
    assert_eq!(commit.source_digest, program.source_digest());
    assert!(
        commit.changed_root_catalog_ids.contains(&books),
        "books root missing from nested commit metadata: {commit:#?}"
    );
    assert!(
        commit.changed_root_catalog_ids.contains(&audits),
        "audits root missing from nested commit metadata: {commit:#?}"
    );
    assert_eq!(
        commit.changed_root_catalog_ids.len(),
        2,
        "inner and outer writes are reported on the same durable commit: {commit:#?}"
    );
    assert_eq!(commit.changed_index_catalog_ids, vec![by_shelf]);
}

#[test]
fn nested_transaction_rollback_does_not_stamp_attempted_inner_writes() {
    let program = checked_program(
        "resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   shelf: string\n\
         \x20   index byShelf(shelf, id)\n\n\
         resource Audit at ^audits(id: int)\n\
         \x20   required message: string\n\n\
         pub fn seed()\n\
         \x20   ^books(1).title = \"Mort\"\n\n\
         pub fn fail()\n\
         \x20   transaction\n\
         \x20       ^books(1).shelf = \"fiction\"\n\
         \x20       transaction\n\
         \x20           ^audits(1).message = \"attempt\"\n\
         \x20       throw Error(code: \"test.rollback\", message: \"stop\")\n\n\
         pub fn shelf(): string\n\
         \x20   return ^books(1).shelf ?? \"\"\n\n\
         pub fn has_audit(): bool\n\
         \x20   return exists(^audits(1))\n",
    );
    let books = store_catalog_id(&program, "books");
    let store = TreeStore::memory();

    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed commits");
    let before = store
        .read_commit_metadata()
        .expect("read commit metadata")
        .expect("seed commit metadata");
    assert_eq!(before.commit_id, 1);
    assert_eq!(before.changed_root_catalog_ids, vec![books]);

    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::fail")),
        RUN_UNCAUGHT_THROW,
    );

    let after = store
        .read_commit_metadata()
        .expect("read commit metadata")
        .expect("commit metadata remains");
    assert_eq!(after, before);
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::shelf"))
            .expect("read shelf")
            .value,
        Some(Value::Str(String::new()))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::has_audit"))
            .expect("read audit")
            .value,
        Some(Value::Bool(false))
    );
}

#[test]
fn a_mistyped_field_write_is_rejected() {
    checker_rejects(
        &format!("{BOOK_PRIMARY_SCHEMA}pub fn bad(id: int)\n    ^books(id).title = 5\n"),
        "check.assignment_type",
    );
}

/// A program that queries saved `Book` data with `exists` and the `??`
/// absence-default.
const BOOK_QUERY: &str = "\
resource Book at ^books(id: int)
    required title: string
    subtitle: string

pub fn has_book(id: int): bool
    return exists(^books(id))

pub fn has_title(id: int): bool
    return exists(^books(id).title)

pub fn subtitle_or(id: int, fallback: string): string
    return ^books(id).subtitle ?? fallback
";

#[test]
fn exists_reports_record_and_field_presence() {
    let program = checked_program(BOOK_QUERY);
    let store = store_with_title(&program, 1, "Mort");
    let value = |entry, id| {
        run_entry(&store, checked_entry!(&program, entry, Value::Int(id)))
            .expect("run")
            .value
    };
    // Record 1 exists (it has the title child); record 2 does not.
    assert_eq!(value("test::has_book", 1), Some(Value::Bool(true)));
    assert_eq!(value("test::has_book", 2), Some(Value::Bool(false)));
    // Its title field is present; its sparse subtitle is not.
    assert_eq!(value("test::has_title", 1), Some(Value::Bool(true)));
}

#[test]
fn coalesce_returns_the_default_for_an_absent_field() {
    let program = checked_program(BOOK_QUERY);
    let store = store_with_title(&program, 1, "Mort"); // subtitle is absent
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::subtitle_or",
            Value::Int(1),
            Value::Str("(none)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(none)".into())));
}

#[test]
fn coalesce_returns_the_value_when_present() {
    let program = checked_program(BOOK_QUERY);
    let store = store_with_title(&program, 1, "Mort");
    // Populate the sparse subtitle directly.
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(1)],
        &data_path(&program, "books", &["subtitle"]),
        SavedValue::Str("A Discworld Novel".into()),
    );
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::subtitle_or",
            Value::Int(1),
            Value::Str("(none)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("A Discworld Novel".into())));
}

/// A `Patient` with an unkeyed `name` group, queried with a `?.` chain defaulted
/// by `??`. The chain `^patients(id)?.name?.first` reads the first name when the
/// whole record, the `name` group, and the field are all populated, and is absent
/// (caught by `??`) when any step along the way is missing.
const PATIENT_CHAIN: &str = "\
resource Patient at ^patients(id: int)
    name
        first: string
        last: string

pub fn first_name_or(id: int, fallback: string): string
    return ^patients(id)?.name?.first ?? fallback
";

fn store_with_first_name(program: &CheckedRuntimeProgram, id: i64, first: &str) -> TreeStore {
    let store = empty_store();
    write_data_value(
        program,
        &store,
        "patients",
        &[SavedKey::Int(id)],
        &data_path(program, "patients", &["name", "first"]),
        SavedValue::Str(first.into()),
    );
    store
}

#[test]
fn optional_chain_with_default_reads_a_present_value() {
    let program = checked_program(PATIENT_CHAIN);
    let store = store_with_first_name(&program, 1, "Granny");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(1),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("Granny".into())));
}

#[test]
fn optional_chain_defaults_when_the_record_is_absent() {
    let program = checked_program(PATIENT_CHAIN);
    // Record 2 was never written: the whole record is absent, so the chain
    // short-circuits and `??` supplies the default.
    let store = store_with_first_name(&program, 1, "Granny");
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(2),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(unknown)".into())));
}

#[test]
fn optional_chain_defaults_when_an_intermediate_field_is_absent() {
    let program = checked_program(PATIENT_CHAIN);
    // The record exists (it has a `last` name) but `name.first` does not: the
    // final hop short-circuits the chain, and `??` supplies the default.
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "patients",
        &[SavedKey::Int(1)],
        &data_path(&program, "patients", &["name", "last"]),
        SavedValue::Str("Weatherwax".into()),
    );
    let value = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::first_name_or",
            Value::Int(1),
            Value::Str("(unknown)".into())
        ),
    )
    .expect("run")
    .value;
    assert_eq!(value, Some(Value::Str("(unknown)".into())));
}

#[test]
fn an_unguarded_optional_chain_that_ends_absent_is_rejected() {
    checker_rejects(
        "resource Patient at ^patients(id: int)\n    name\n        first: string\n        last: string\n\npub fn first_name(id: int): string\n    return ^patients(id)?.name?.first\n",
        "check.bare_maybe_present_read",
    );
}

#[test]
fn next_id_allocates_past_the_highest_record() {
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn fresh(): Id(^books)\n    return nextId(^books)\n"
    ));
    let store = empty_store();
    // Empty root: the next id is 1.
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        "books",
        &[SavedKey::Int(1)],
    );
    // Seed records 1 and 4; the next id is one past the highest.
    for id in [1, 4] {
        write_data_value(
            &program,
            &store,
            "books",
            &[SavedKey::Int(id)],
            &data_path(&program, "books", &["title"]),
            SavedValue::Str("t".into()),
        );
    }
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        "books",
        &[SavedKey::Int(5)],
    );
}

#[test]
fn next_id_skips_ahead_after_restore() {
    // After a restore the store may hold records far above any contiguous run.
    // `nextId` chooses one past the highest existing key, never reusing a gap.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn fresh(): Id(^books)\n    return nextId(^books)\n"
    ));
    let store = empty_store();
    write_data_value(
        &program,
        &store,
        "books",
        &[SavedKey::Int(900)],
        &data_path(&program, "books", &["title"]),
        SavedValue::Str("t".into()),
    );
    assert_identity_value(
        run_entry(&store, checked_entry!(&program, "test::fresh"))
            .expect("run")
            .value,
        "books",
        &[SavedKey::Int(901)],
    );
}

/// `nextId` over a composite-identity root faults with `write.next_id_unsupported`
/// rather than inventing a bogus `Int(1)`: composite identities have no default
/// allocation policy.
#[test]
fn next_id_over_a_composite_root_faults() {
    checker_rejects(
        "resource Enrollment at ^enrollments(studentId: int, courseId: int)\n    required grade: string\n\npub fn fresh(): int\n    return nextId(^enrollments)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` over a keyless singleton root faults: a singleton has no generated
/// identity to allocate.
#[test]
fn next_id_over_a_singleton_root_faults() {
    checker_rejects(
        "resource Settings at ^settings\n    required theme: string\n\npub fn fresh(): int\n    return nextId(^settings)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` over a single non-integer (string) identity key faults: only an
/// `int` identity has the default policy.
#[test]
fn next_id_over_a_string_keyed_root_faults() {
    checker_rejects(
        "resource Tag at ^tags(slug: string)\n    required name: string\n\npub fn fresh(): int\n    return nextId(^tags)\n",
        "check.next_id_requires_single_int",
    );
}

/// `nextId` of a saved root no store declares is a `run.unsupported`: there is
/// no schema to decide an allocation policy (mirrors `eval_append`'s unknown-root
/// path).
#[test]
fn next_id_over_an_undeclared_root_is_unsupported() {
    checker_rejects(
        &format!("{BOOK_PRIMARY_SCHEMA}pub fn fresh(): int\n    return nextId(^bogus)\n"),
        "check.untyped_value",
    );
}
