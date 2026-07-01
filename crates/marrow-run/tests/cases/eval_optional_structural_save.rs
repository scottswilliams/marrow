//! Present-or-clear structural save for a `T?` saved place: assigning `absent`
//! removes the data node and its unique-index entry through the node-delete
//! planner, atomically, and a corrupted index resolves missing vs dangling per the
//! runtime contract.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::{SavedValue, ScalarType};

/// A widget with a sparse `sku` under a unique index. `seed` writes it, `clearSku`
/// assigns `absent` over it, `skuOf` reads it back, and `idBySku` resolves the
/// unique lookup to the owning id or a sentinel when the key has no entry.
const WIDGET_SKU: &str = "\
resource Widget
    sku: string
store ^widgets(id: int): Widget

    index bySku(sku) unique

pub fn seed(id: int, sku: string)
    ^widgets(id).sku = sku

pub fn clearSku(id: int)
    ^widgets(id).sku = absent

pub fn clearThenFault(id: int)
    transaction
        ^widgets(id).sku = absent
        throw Error(code: \"test.boom\", message: \"boom\")

pub fn skuOf(id: int): string
    return ^widgets(id).sku ?? \"<none>\"

pub fn idBySku(sku: string): int
    if const w = ^widgets.bySku(sku)
        return key(w)
    return -1
";

fn seed(program: &marrow_check::CheckedRuntimeProgram, store: &TreeStore, id: i64, sku: &str) {
    run_entry(
        store,
        checked_entry!(
            program,
            "test::seed",
            Value::Int(id),
            Value::Str(sku.into())
        ),
    )
    .expect("seed widget");
}

fn sku_of(program: &marrow_check::CheckedRuntimeProgram, store: &TreeStore, id: i64) -> Value {
    run_entry(
        store,
        checked_entry!(program, "test::skuOf", Value::Int(id)),
    )
    .expect("read sku")
    .value
    .expect("sku value")
}

fn id_by_sku(program: &marrow_check::CheckedRuntimeProgram, store: &TreeStore, sku: &str) -> i64 {
    let Some(Value::Int(id)) = run_entry(
        store,
        checked_entry!(program, "test::idBySku", Value::Str(sku.into())),
    )
    .expect("lookup by sku")
    .value
    else {
        panic!("idBySku returned a non-int");
    };
    id
}

fn stored_sku(
    program: &marrow_check::CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
) -> Option<SavedValue> {
    read_data_value(
        program,
        store,
        "widgets",
        &[SavedKey::Int(id)],
        &data_path(program, "widgets", &["sku"]),
        ScalarType::Str,
    )
}

#[test]
fn assigning_absent_clears_the_node_and_unique_index_and_frees_the_key() {
    let program = checked_program(WIDGET_SKU);
    let store = empty_store();
    seed(&program, &store, 1, "abc");

    // The value is stored and both the data node and the unique-index entry resolve.
    assert_eq!(
        stored_sku(&program, &store, 1),
        Some(SavedValue::Str("abc".into()))
    );
    assert_eq!(sku_of(&program, &store, 1), Value::Str("abc".into()));
    assert_eq!(id_by_sku(&program, &store, "abc"), 1);

    run_entry(
        &store,
        checked_entry!(&program, "test::clearSku", Value::Int(1)),
    )
    .expect("clear sku");

    // The data node is gone, the unique-index entry is gone, and the key is free to
    // rebind to another record without a unique conflict.
    assert_eq!(stored_sku(&program, &store, 1), None);
    assert_eq!(sku_of(&program, &store, 1), Value::Str("<none>".into()));
    assert_eq!(id_by_sku(&program, &store, "abc"), -1);

    seed(&program, &store, 2, "abc");
    assert_eq!(id_by_sku(&program, &store, "abc"), 2);
}

#[test]
fn a_mid_apply_fault_rolls_back_both_the_node_delete_and_index_removal() {
    let program = checked_program(WIDGET_SKU);
    let store = empty_store();
    seed(&program, &store, 1, "abc");

    // The clear is staged inside the transaction, then a fault aborts it. Both the
    // node delete and its index removal must roll back atomically.
    let error = run_entry(
        &store,
        checked_entry!(&program, "test::clearThenFault", Value::Int(1)),
    )
    .expect_err("the transaction faults");
    assert_eq!(error.uncaught_throw_code().as_deref(), Some("test.boom"));

    assert_eq!(
        stored_sku(&program, &store, 1),
        Some(SavedValue::Str("abc".into()))
    );
    assert_eq!(sku_of(&program, &store, 1), Value::Str("abc".into()));
    assert_eq!(id_by_sku(&program, &store, "abc"), 1);
}

#[test]
fn a_missing_key_is_absent_while_a_dangling_index_entry_is_fatal() {
    let program = checked_program(WIDGET_SKU);
    let store = empty_store();
    seed(&program, &store, 1, "abc");

    // A key with no entry resolves to the empty optional, not a fault.
    assert_eq!(id_by_sku(&program, &store, "missing"), -1);

    // Delete the record's data directly, leaving the unique-index entry dangling.
    store
        .delete_record_subtree(&store_catalog_id(&program, "widgets"), &[SavedKey::Int(1)])
        .expect("drop the record data behind the index");

    let error = run_entry(
        &store,
        checked_entry!(&program, "test::idBySku", Value::Str("abc".into())),
    )
    .expect_err("a dangling unique-index entry is a fatal integrity fault");
    assert_eq!(error.code(), "run.absent_element");
    assert!(
        !error.is_catchable(),
        "a dangling index entry is fatal, not catchable"
    );
}
