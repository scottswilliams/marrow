//! Commit-id conformance laws for runtime writes across TreeStore backends.

use crate::support;
use support::*;

use marrow_run::{RUN_UNCAUGHT_THROW, Value};
use marrow_store::tree::TreeStore;
use marrow_store::{AccessMode, SealedStore};

const WRITES: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

pub fn set_title(id: int, title: string)
    ^books(id).title = title

pub fn fail_after_write(id: int)
    transaction
        ^books(id).title = \"attempt\"
        throw Error(code: \"test.rollback\", message: \"stop\")

pub fn title_of(id: int): string
    return ^books(id).title ?? \"\"
";

#[test]
fn memory_commit_ids_are_dense_across_commits_and_rollbacks() {
    let store = TreeStore::memory();
    commit_id_density_law(&store);
}

#[test]
fn native_commit_ids_are_dense_across_commits_and_rollbacks() {
    let dir = TempDir::new("marrow-run-commit-id-conformance").expect("create temp dir");
    let store = SealedStore::open(&dir.path().join("store.redb"), AccessMode::Create)
        .expect("open native store")
        .into_store();
    commit_id_density_law(&store);
}

fn commit_id_density_law(store: &TreeStore) {
    let program = checked_program(WRITES);
    assert_eq!(commit_high_water(store), 0);

    set_title(store, &program, 1, "one");
    assert_eq!(commit_high_water(store), 1);

    assert_run_error(
        run_entry(
            store,
            checked_entry!(&program, "test::fail_after_write", Value::Int(2)),
        ),
        RUN_UNCAUGHT_THROW,
    );
    assert_eq!(commit_high_water(store), 1);
    assert_eq!(title_of(store, &program, 2), "");

    set_title(store, &program, 2, "two");
    assert_eq!(commit_high_water(store), 2);

    set_title(store, &program, 3, "three");
    assert_eq!(commit_high_water(store), 3);
}

fn set_title(
    store: &TreeStore,
    program: &marrow_check::CheckedRuntimeProgram,
    id: i64,
    title: &str,
) {
    run_entry(
        store,
        checked_entry!(
            program,
            "test::set_title",
            Value::Int(id),
            Value::Str(title.to_string())
        ),
    )
    .expect("set title");
}

fn title_of(store: &TreeStore, program: &marrow_check::CheckedRuntimeProgram, id: i64) -> String {
    let result = run_entry(
        store,
        checked_entry!(program, "test::title_of", Value::Int(id)),
    )
    .expect("read title")
    .value;
    match result {
        Some(Value::Str(title)) => title,
        other => panic!("expected title string, got {other:?}"),
    }
}

fn commit_high_water(store: &TreeStore) -> u64 {
    store
        .read_commit_metadata()
        .expect("read commit metadata")
        .map_or(0, |commit| commit.commit_id)
}
