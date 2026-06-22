//! Maintenance mode and managed-root protection: gated whole-root and required
//! deletes, the maintenance required-delete exemption, and undeclared saved-field
//! rules.

use crate::support;
use support::*;

use marrow_run::{Host, RUN_TRAVERSAL, Value};
use marrow_store::tree::TreeStore;

// --- Maintenance mode & managed-root protection ---

/// A two-key books program over the canonical shelf/index schema fixture: it can
/// seed records, drop the whole `^books` root, and count remaining records and
/// index entries so a root drop's effect is observable.
fn maintenance_books() -> String {
    format!(
        "{BOOK_SHELF_INDEX_SCHEMA}pub fn seed()\n    ^books(1).title = \"Mort\"\n    ^books(1).shelf = \"fiction\"\n    ^books(2).title = \"Guards\"\n    ^books(2).shelf = \"fiction\"\n\npub fn drop_root()\n    delete ^books\n\npub fn drop_root_while_iterating_index()\n    for id in keys(^books.byShelf(\"fiction\"))\n        delete ^books\n\npub fn record_count(): int\n    var c = 0\n    for book in ^books\n        c = c + 1\n    return c\n\npub fn shelf_count(s: string): int\n    var c = 0\n    for id in keys(^books.byShelf(s))\n        c = c + 1\n    return c\n"
    )
}

/// The required `name` / sparse `shelf` Item store shared by the partial-record
/// maintenance tests, with the `has_item` presence reader they assert against.
/// Each test appends only its distinctive `create` body.
const ITEM_SCHEMA: &str = "\
resource Item
    required name: string
    shelf: string
store ^items(id: int): Item

pub fn has_item(id: int): bool
    return exists(^items(id))

";

#[test]
fn deleting_a_whole_root_without_maintenance_is_rejected() {
    // `delete ^books` on a keyed root is maintenance work; with no maintenance
    // capability the run is rejected with `write.requires_maintenance`.
    let program = checked_program(&maintenance_books());
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    let result = run_entry(&store, checked_entry!(&program, "test::drop_root"));
    assert_run_error(result, "write.requires_maintenance");
    // The records still exist: the rejected delete did not touch the store.
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::record_count"))
            .expect("count")
            .value,
        Some(Value::Int(2))
    );
}

#[test]
fn deleting_a_whole_root_under_maintenance_drops_records_and_indexes() {
    // With the maintenance capability, `delete ^books` drops the entire managed
    // root subtree: no records and no index entries remain.
    let program = checked_program(&maintenance_books());
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::drop_root"))
        .expect("drop root");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::record_count")
        )
        .expect("count")
        .value,
        Some(Value::Int(0)),
        "no records remain after the root drop"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_count", Value::Str("fiction".into()))
        )
        .expect("count")
        .value,
        Some(Value::Int(0)),
        "no index entries remain after the root drop"
    );
}

#[test]
fn maintenance_root_delete_while_iterating_an_index_is_a_traversal_fault() {
    let program = checked_program(&maintenance_books());
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(&store, &host, checked_entry!(&program, "test::seed")).expect("seed");
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::drop_root_while_iterating_index"),
    );
    assert_run_error(result, RUN_TRAVERSAL);
}

#[test]
fn whole_identity_delete_stays_ungated_under_no_maintenance() {
    // `delete ^books(1)` is ordinary whole-identity work: it must still succeed
    // with no maintenance capability, leaving the sibling record in place.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn seed()\n    ^books(1).title = \"Mort\"\n    ^books(2).title = \"Guards\"\n\npub fn drop_one()\n    delete ^books(1)\n\npub fn record_count(): int\n    var c = 0\n    for book in ^books\n        c = c + 1\n    return c\n"
    ));
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed");
    run_entry(&store, checked_entry!(&program, "test::drop_one"))
        .expect("ordinary identity delete");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::record_count"))
            .expect("count")
            .value,
        Some(Value::Int(1)),
        "the sibling record survives an ordinary identity delete"
    );
}

#[test]
fn deleting_a_required_field_under_maintenance_succeeds() {
    // A required-field delete is rejected without maintenance (existing behavior),
    // but a maintenance run lifts the guard and actually removes the field.
    let program = checked_program(&format!(
        "{BOOK_PRIMARY_SCHEMA}pub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n\npub fn drop_title(id: int)\n    delete ^books(id).title\n\npub fn has_title(id: int): bool\n    return exists(^books(id).title)\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::drop_title", Value::Int(1)),
    )
    .expect("maintenance lifts the required-field guard");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "the required field is gone after a maintenance delete"
    );
}

#[test]
fn maintenance_transaction_can_delete_required_field_after_a_field_write() {
    let program = checked_program(&format!(
        "{BOOK_SHELF_SCHEMA}pub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\npub fn repair(id: int)\n    transaction\n        ^books(id).shelf = \"legacy\"\n        delete ^books(id).title\n\npub fn has_title(id: int): bool\n    return exists(^books(id).title)\n\npub fn shelf_of(id: int): string\n    return ^books(id).shelf ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::repair", Value::Int(1)),
    )
    .expect("maintenance transaction");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "maintenance still permits required-field repair deletes"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("legacy".into())),
        "the transaction's ordinary field write still committed"
    );
}

#[test]
fn maintenance_transaction_still_rejects_new_partial_required_record() {
    let program = checked_program(&format!(
        "{ITEM_SCHEMA}pub fn create(id: int)\n    transaction\n        ^items(id).shelf = \"legacy\"\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "maintenance does not make a sparse field write a partial-record creator"
    );
}

#[test]
fn maintenance_transaction_noop_required_delete_does_not_permit_partial_record() {
    let program = checked_program(&format!(
        "{ITEM_SCHEMA}pub fn create(id: int)\n    transaction\n        ^items(id).shelf = \"legacy\"\n        delete ^items(id).name\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "a no-op required delete is not a license to create partial data"
    );
}

#[test]
fn maintenance_transaction_staged_required_delete_does_not_permit_partial_record() {
    let program = checked_program(&format!(
        "{ITEM_SCHEMA}pub fn create(id: int)\n    transaction\n        ^items(id).name = \"temporary\"\n        ^items(id).shelf = \"legacy\"\n        delete ^items(id).name\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "staged-only required data cannot mint a maintenance exemption"
    );
}

#[test]
fn maintenance_transaction_whole_resource_required_delete_does_not_permit_partial_record() {
    let program = checked_program(&format!(
        "{ITEM_SCHEMA}pub fn create(id: int)\n    var item: Item\n    item.name = \"temporary\"\n    item.shelf = \"legacy\"\n    transaction\n        ^items(id) = item\n        delete ^items(id).name\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "a whole-resource write cannot create then delete required data to bypass validation"
    );
}

#[test]
fn maintenance_transaction_whole_group_required_delete_does_not_permit_partial_entry() {
    let program = checked_program(
        "resource Book\n    required title: string\n    versions(version: int)\n        required title: string\n        note: string\nstore ^books(id: int): Book\n\npub fn seed(id: int)\n    ^books(id).title = \"root\"\n\npub fn create_version(id: int)\n    var version: Book\n    version.title = \"temporary\"\n    transaction\n        ^books(id).versions(1) = version\n        delete ^books(id).versions(1).title\n\npub fn has_version(id: int): bool\n    return exists(^books(id).versions(1))\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create_version", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_version", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "a whole group-entry write cannot create then delete required data to bypass validation"
    );
}

#[test]
fn maintenance_outer_delete_of_inner_created_required_field_is_rejected() {
    let program = checked_program(&format!(
        "{ITEM_SCHEMA}pub fn create(id: int)\n    transaction\n        transaction\n            ^items(id).name = \"temporary\"\n            ^items(id).shelf = \"legacy\"\n        delete ^items(id).name\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    let result = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::create", Value::Int(1)),
    );
    assert_run_error(result, "write.required_absent");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_item", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false)),
        "an outer transaction must still validate entries created by an inner commit"
    );
}

#[test]
fn maintenance_required_delete_exemption_covers_parent_validation_for_child_writes() {
    let program = checked_program(
        "resource Book\n    required title: string\n    shelf: string\n    notes(note: string)\n        required text: string\nstore ^books(id: int): Book\n\npub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\npub fn repair(id: int)\n    transaction\n        delete ^books(id).title\n        ^books(id).notes(\"n1\").text = \"indexed\"\n\npub fn has_title(id: int): bool\n    return exists(^books(id).title)\n\npub fn note_text(id: int): string\n    return ^books(id).notes(\"n1\").text ?? \"\"\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::repair", Value::Int(1)),
    )
    .expect("parent maintenance delete should not block child validation");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::note_text", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("indexed".into()))
    );
}

#[test]
fn maintenance_required_delete_exemption_crosses_nested_transaction_commit() {
    let program = checked_program(&format!(
        "{BOOK_SHELF_SCHEMA}pub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\npub fn inner_delete(id: int)\n    transaction\n        ^books(id).shelf = \"outer\"\n        transaction\n            delete ^books(id).title\n\npub fn outer_delete(id: int)\n    transaction\n        delete ^books(id).title\n        transaction\n            ^books(id).shelf = \"inner\"\n\npub fn has_title(id: int): bool\n    return exists(^books(id).title)\n\npub fn shelf_of(id: int): string\n    return ^books(id).shelf ?? \"\"\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::inner_delete", Value::Int(1)),
    )
    .expect("inner maintenance delete should satisfy outer validation");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_of", Value::Int(1))
        )
        .expect("read")
        .value,
        Some(Value::Str("outer".into()))
    );

    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(2)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::outer_delete", Value::Int(2)),
    )
    .expect("outer maintenance delete should satisfy inner validation");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::has_title", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Bool(false))
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_of", Value::Int(2))
        )
        .expect("read")
        .value,
        Some(Value::Str("inner".into()))
    );
}

#[test]
fn unquoted_undeclared_field_write_is_rejected_at_check() {
    // A saved record is a fixed typed tree, so writing a statically undeclared field is
    // rejected at check the same way reading one is; maintenance does not loosen the
    // schema. The runtime backstop remains for paths the checker cannot prove.
    checker_rejects(
        &format!("{BOOK_PRIMARY_SCHEMA}pub fn typo(id: int)\n    ^books(id).nope = \"x\"\n"),
        "check.unknown_field",
    );
}

#[test]
fn managed_write_to_a_declared_field_is_unaffected() {
    let program = checked_program(&format!(
        "{BOOK_SHELF_INDEX_SCHEMA}pub fn seed(id: int)\n    ^books(id).title = \"Mort\"\n    ^books(id).shelf = \"fiction\"\n\npub fn move_shelf(id: int, s: string)\n    ^books(id).shelf = s\n\npub fn shelf_at(s: string): int\n    var c = 0\n    for id in keys(^books.byShelf(s))\n        c = c + 1\n    return c\n"
    ));
    let store = TreeStore::memory();
    let host = Host::new().with_maintenance();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::seed", Value::Int(1)),
    )
    .expect("seed");
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::move_shelf",
            Value::Int(1),
            Value::Str("history".into())
        ),
    )
    .expect("managed write to a declared field");
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_at", Value::Str("history".into()))
        )
        .expect("count")
        .value,
        Some(Value::Int(1)),
        "the managed write moved the record's index entry to the new shelf"
    );
    assert_eq!(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(&program, "test::shelf_at", Value::Str("fiction".into()))
        )
        .expect("count")
        .value,
        Some(Value::Int(0)),
        "no stale entry remains on the old shelf"
    );
}
