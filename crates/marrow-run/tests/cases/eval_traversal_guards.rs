//! Loop traversal-write guards: mutating a saved layer while iterating it is
//! guarded both statically and at runtime. The checker rejects direct mutation
//! (check.loop_mutates_traversed_layer) and the circumvention paths that would
//! otherwise sneak past it: passing a traversed key into a helper that mutates
//! (check.call_argument) and snapshotting saved keys into a local
//! (check.key_type). Mutations the checker cannot see statically fault at
//! runtime (RUN_TRAVERSAL).

use crate::support;
use support::*;

use marrow_run::{RUN_TRAVERSAL, Value};
use marrow_store::tree::TreeStore;

#[test]
fn traversal_faults_are_not_catchable_errors() {
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn clear(): string\n    try\n        for id in ^books\n            delete ^books(id)\n        return \"completed\"\n    catch error: Error\n        return error.code\n"
        ),
        "check.loop_mutates_traversed_layer",
    );
}

#[test]
fn appending_to_the_sequence_being_traversed_is_a_traversal_fault() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    const p: int = append(^books(1).tags, \"x\")\n\npub fn grow()\n    for tag in ^books(1).tags\n        const p: int = append(^books(1).tags, \"y\")\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::grow"));
    assert_run_error(faulted, RUN_TRAVERSAL);
}

#[test]
fn helper_appending_to_the_sequence_being_traversed_is_a_traversal_fault() {
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    append(^books(1).tags, \"x\")\n\npub fn grow()\n    append(^books(1).tags, \"y\")\n\npub fn walk()\n    for tag in ^books(1).tags\n        grow()\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let faulted = run_entry(&store, checked_entry!(&program, "test::walk"));
    assert_run_error(faulted, RUN_TRAVERSAL);
}

#[test]
fn helper_deleting_from_the_root_being_traversed_is_a_traversal_fault() {
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn remove(id: int)\n    delete ^books(id)\n\npub fn walk()\n    for id in ^books\n        remove(id)\n"
        ),
        "check.call_argument",
    );
}

#[test]
fn field_write_creating_a_record_in_the_traversed_root_is_checker_rejected() {
    // A field write at a literal key that is not the loop key may insert a new record
    // into the traversed root. The key is statically not the loop key, so the checker
    // rejects it rather than deferring to the runtime traversal guard.
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn grow()\n    for id in ^books\n        ^books(99).title = \"new\"\n"
        ),
        "check.loop_mutates_traversed_layer",
    );
}

#[test]
fn collecting_saved_keys_as_a_local_snapshot_is_checker_rejected() {
    checker_rejects(
        &format!(
            "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n\npub fn clear()\n    const ids = keys(^books)\n    for id in ids\n        delete ^books(id)\n\npub fn remaining(): int\n    return count(^books)\n"
        ),
        "check.key_type",
    );
}

#[test]
fn mutating_a_different_record_layer_while_traversing_is_allowed() {
    // Traversing `^books(1).tags` and appending to `^books(2).tags` touches a
    // different record's layer, so it is not a traversal fault.
    let program = checked_program(&format!(
        "{BOOK_TAGS_SCHEMA}pub fn seed()\n    ^books(1).title = \"a\"\n    ^books(2).title = \"b\"\n    const p: int = append(^books(1).tags, \"x\")\n\npub fn copy()\n    for tag in ^books(1).tags\n        const p: int = append(^books(2).tags, \"y\")\n\npub fn tags2(): int\n    return count(^books(2).tags)\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry(&store, checked_entry!(&program, "test::copy")).expect("copy");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::tags2"))
            .expect("count")
            .value,
        Some(Value::Int(1))
    );
}
