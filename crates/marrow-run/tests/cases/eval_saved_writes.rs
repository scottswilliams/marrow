//! Saved field writes, the out-of-transaction and transaction-commit
//! required-field rules, joined nested-transaction commit and abort metadata, and
//! the mistyped-write rejection.

use crate::support;
use support::*;

use marrow_run::{RUN_UNCAUGHT_THROW, Value};
use marrow_store::tree::TreeStore;

/// A program that writes and reads a `Book` title.
const BOOK_WRITER: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn set_title(id: int, t: string)
    ^books(id).title = t

pub fn title_of(id: int): string
    return ^books(id).title ?? \"\"
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

/// A program exercising sparse positional write and delete over a saved sequence,
/// the operations a local sequence must mirror.
const SAVED_SEQUENCE: &str = "\
resource Doc
    title: string
    tags: sequence[string]
store ^docs(id: int): Doc

pub fn sparse(id: int): string
    ^docs(id).title = \"t\"
    append(^docs(id).tags, \"a\")
    ^docs(id).tags(5) = \"sparse\"
    const at5: string = ^docs(id).tags(5) ?? \"none\"
    const at3: string = ^docs(id).tags(3) ?? \"hole\"
    const n: int = count(^docs(id).tags)
    return $\"{at5};{at3};{n}\"

pub fn drop(id: int): string
    ^docs(id).title = \"t\"
    append(^docs(id).tags, \"x\")
    append(^docs(id).tags, \"y\")
    append(^docs(id).tags, \"z\")
    delete ^docs(id).tags(2)
    const gone: string = ^docs(id).tags(2) ?? \"absent\"
    const n: int = count(^docs(id).tags)
    const at: int = append(^docs(id).tags, \"w\")
    var positions: string = \"\"
    for pos in ^docs(id).tags
        positions = $\"{positions}{pos};\"
    return $\"{gone};{n};{at};{positions}\"
";

#[test]
fn saved_sequence_sparse_write_and_delete_match_the_local_contract() {
    let program = checked_program(SAVED_SEQUENCE);
    let store = TreeStore::memory();
    // The saved sparse write yields the same `value;hole;count` a local sequence does.
    let sparse = run_entry(
        &store,
        checked_entry!(&program, "test::sparse", Value::Int(1)),
    )
    .expect("sparse write");
    assert_eq!(sparse.value, Some(Value::Str("sparse;hole;2".into())));
    // The saved delete leaves a hole, count drops to the stored entries, append lands
    // past the highest position, and iteration skips the deleted position — identical
    // to the local-sequence delete test.
    let drop = run_entry(
        &store,
        checked_entry!(&program, "test::drop", Value::Int(2)),
    )
    .expect("delete");
    assert_eq!(drop.value, Some(Value::Str("absent;2;4;1;3;4;".into())));
}

/// A program that writes and reads a `Log` error code through a dynamic value.
const ERROR_CODE_WRITER: &str = "\
resource Log
    required code: ErrorCode
store ^logs(id: int): Log

pub fn set_code(id: int, c: string)
    ^logs(id).code = c

pub fn code_of(id: int): string
    return ^logs(id).code ?? \"\"
";

#[test]
fn a_dynamic_invalid_error_code_write_faults_and_persists_nothing() {
    let program = checked_program(ERROR_CODE_WRITER);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_code",
            Value::Int(1),
            Value::Str("no good code".into())
        ),
    );
    assert_run_error(result, "run.type");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::code_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str(String::new())),
        "an invalid error code must never reach saved data"
    );
}

#[test]
fn a_dynamic_invalid_error_code_constructor_field_faults() {
    let program = checked_program(
        "resource Log\n\
         \x20   required code: ErrorCode\n\
         store ^logs(id: int): Log\n\n\
         pub fn make(id: int, c: string)\n\
         \x20   ^logs(id) = Log(code: c)\n\n\
         pub fn code_of(id: int): string\n\
         \x20   return ^logs(id).code ?? \"\"\n",
    );
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::make",
            Value::Int(1),
            Value::Str("no good code".into())
        ),
    );
    assert_run_error(result, "run.type");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::code_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str(String::new())),
        "a constructor must not let invalid error-code text reach saved data"
    );
}

#[test]
fn a_dynamic_valid_error_code_write_persists() {
    let program = checked_program(ERROR_CODE_WRITER);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_code",
            Value::Int(1),
            Value::Str("app.missing".into())
        ),
    )
    .expect("write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::code_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("app.missing".into()))
    );
}

/// A program that writes and reads a keyed-leaf `ErrorCode` through a dynamic value.
const KEYED_LEAF_ERROR_CODE_WRITER: &str = "\
resource Log
    tags(k: int): ErrorCode
store ^logs(id: int): Log

pub fn set_tag(id: int, k: int, c: string)
    ^logs(id).tags(k) = c

pub fn tag_of(id: int, k: int): string
    return ^logs(id).tags(k) ?? \"\"
";

#[test]
fn a_dynamic_invalid_keyed_leaf_error_code_write_faults_and_persists_nothing() {
    let program = checked_program(KEYED_LEAF_ERROR_CODE_WRITER);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_tag",
            Value::Int(1),
            Value::Int(1),
            Value::Str("no good code".into())
        ),
    );
    assert_run_error(result, "run.type");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_of", Value::Int(1), Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str(String::new())),
        "an invalid error code must never reach a keyed-leaf place"
    );
}

#[test]
fn a_dynamic_valid_keyed_leaf_error_code_write_persists() {
    let program = checked_program(KEYED_LEAF_ERROR_CODE_WRITER);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_tag",
            Value::Int(1),
            Value::Int(1),
            Value::Str("app.ok".into())
        ),
    )
    .expect("write");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::tag_of", Value::Int(1), Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("app.ok".into()))
    );
}

/// A program that appends to and reads a `sequence[ErrorCode]` through a dynamic value.
const SEQUENCE_ERROR_CODE_WRITER: &str = "\
resource Log
    codes: sequence[ErrorCode]
store ^logs(id: int): Log

pub fn add_code(id: int, c: string)
    append(^logs(id).codes, c)

pub fn first_code(id: int): string
    return ^logs(id).codes(1) ?? \"\"
";

#[test]
fn a_dynamic_invalid_sequence_error_code_append_faults_and_persists_nothing() {
    let program = checked_program(SEQUENCE_ERROR_CODE_WRITER);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_code",
            Value::Int(1),
            Value::Str("no good code".into())
        ),
    );
    assert_run_error(result, "run.type");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::first_code", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str(String::new())),
        "an invalid error code must never be appended to a sequence place"
    );
}

#[test]
fn a_dynamic_valid_sequence_error_code_append_persists() {
    let program = checked_program(SEQUENCE_ERROR_CODE_WRITER);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::add_code",
            Value::Int(1),
            Value::Str("app.ok".into())
        ),
    )
    .expect("append");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::first_code", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("app.ok".into()))
    );
}

/// A program that seeds and then directly overwrites a position of a
/// `sequence[ErrorCode]` through a dynamic value, the keyed-assignment sibling of
/// the append path.
const SEQUENCE_ERROR_CODE_ASSIGNER: &str = "\
resource Log
    codes: sequence[ErrorCode]
store ^logs(id: int): Log

pub fn seed(id: int)
    append(^logs(id).codes, \"app.ok\")
    append(^logs(id).codes, \"app.ok\")

pub fn set_code(id: int, c: string)
    ^logs(id).codes(2) = c

pub fn code_at(id: int, k: int): string
    return ^logs(id).codes(k) ?? \"\"
";

#[test]
fn a_dynamic_invalid_sequence_error_code_assignment_faults_and_persists_nothing() {
    let program = checked_program(SEQUENCE_ERROR_CODE_ASSIGNER);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    let result = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_code",
            Value::Int(1),
            Value::Str("no good code".into())
        ),
    );
    assert_run_error(result, "run.type");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::code_at", Value::Int(1), Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("app.ok".into())),
        "an invalid error code must never overwrite a sequence position"
    );
}

#[test]
fn a_dynamic_valid_sequence_error_code_assignment_persists() {
    let program = checked_program(SEQUENCE_ERROR_CODE_ASSIGNER);
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::set_code",
            Value::Int(1),
            Value::Str("app.replaced".into())
        ),
    )
    .expect("assign");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::code_at", Value::Int(1), Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("app.replaced".into()))
    );
}

#[test]
fn out_of_transaction_field_write_rejects_partial_required_record() {
    let program = checked_program(
        "resource Item\n\
         \x20   required name: string\n\
         \x20   shelf: string\nstore ^items(id: int): Item\n\n\
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
    let message = run_error_message(run_entry(
        &store,
        checked_entry!(&program, "test::set_shelf", Value::Int(1)),
    ));
    assert!(
        message.contains("transaction"),
        "required-absent guidance should point at grouping writes in a transaction: {message}"
    );
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
        "resource Book\n\
         \x20   required title: string\n\
         \x20   binding\n\
         \x20       cover: string\nstore ^books(id: int): Book\n\n\
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
        "resource Item\n\
         \x20   required name: string\n\
         \x20   shelf: string\nstore ^items(id: int): Item\n\n\
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
        "resource Item\n\
         \x20   required name: string\n\
         \x20   shelf: string\nstore ^items(id: int): Item\n\n\
         pub fn set_shelf(id: int)\n\
         \x20   ^items(id).shelf = \"fiction\"\n\n\
         pub fn create(id: int)\n\
         \x20   transaction\n\
         \x20       set_shelf(id)\n\
         \x20       ^items(id).name = \"Mort\"\n\n\
         pub fn name_of(id: int): string\n\
         \x20   return ^items(id).name ?? \"\"\n",
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
        "resource Item\n\
         \x20   required name: string\n\
         \x20   shelf: string\nstore ^items(id: int): Item\n\n\
         pub fn create(id: int)\n\
         \x20   transaction\n\
         \x20       transaction\n\
         \x20           ^items(id).shelf = \"fiction\"\n\
         \x20       ^items(id).name = \"Mort\"\n\n\
         pub fn name_of(id: int): string\n\
         \x20   return ^items(id).name ?? \"\"\n",
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
        "resource Book\n\
         \x20   required title: string\n\
         \x20   shelf: string\nstore ^books(id: int): Book\n\
         \x20   index byShelf(shelf, id)\n\n\
         resource Audit\n\
         \x20   required message: string\n\
         store ^audits(id: int): Audit\n\n\
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
        "resource Book\n\
         \x20   required title: string\n\
         \x20   shelf: string\nstore ^books(id: int): Book\n\
         \x20   index byShelf(shelf, id)\n\n\
         resource Audit\n\
         \x20   required message: string\n\
         store ^audits(id: int): Audit\n\n\
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
fn nested_transaction_abort_does_not_stamp_attempted_writes() {
    let program = checked_program(
        "resource Book\n\
         \x20   required title: string\n\
         \x20   shelf: string\nstore ^books(id: int): Book\n\
         \x20   index byShelf(shelf, id)\n\n\
         resource Audit\n\
         \x20   required message: string\n\
         store ^audits(id: int): Audit\n\n\
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
