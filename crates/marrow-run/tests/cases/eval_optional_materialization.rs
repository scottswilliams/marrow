//! Materializing a maybe-present value into a `T?` slot: an absent saved read,
//! `T?`-returning call, or `absent` flows into a `const`/`var` binding or a `T?`
//! argument as the empty optional, while a required whole-record read missing its
//! durable data stays a fatal fault.

use crate::support;
use support::*;

use marrow_run::{RUN_ABSENT, Value};
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::value::SavedValue;

/// A `Book` with a sparse `subtitle`. Every entry materializes the maybe-present
/// `subtitle` into a `T?` slot along a different path, then resolves it with `??`
/// so the test observes whether the empty optional reached the slot.
const BOOK_OPTIONAL: &str = "\
resource Book
    required title: string
    subtitle: string
store ^books(id: int): Book

pub fn write_subtitle(id: int, s: string)
    ^books(id).subtitle = s

pub fn const_slot(id: int): string
    const x: string? = ^books(id).subtitle
    return x ?? \"NONE\"

pub fn var_slot(id: int): string
    var x: string? = ^books(id).subtitle
    return x ?? \"NONE\"

fn tagOf(id: int): string?
    return ^books(id).subtitle

fn label(t: string?): string
    return t ?? \"NONE\"

pub fn arg_from_read(id: int): string
    return label(^books(id).subtitle)

pub fn arg_from_call(id: int): string
    return label(tagOf(id))

fn chainB(id: int): string?
    return ^books(id).subtitle

fn chainA(id: int): string?
    return chainB(id)

pub fn chain_top(id: int): string
    return chainA(id) ?? \"NONE\"
";

fn program() -> marrow_check::CheckedRuntimeProgram {
    checked_program(BOOK_OPTIONAL)
}

fn store_with_title(program: &marrow_check::CheckedRuntimeProgram, id: i64) -> TreeStore {
    let store = empty_store();
    write_data_value(
        program,
        &store,
        "books",
        &[SavedKey::Int(id)],
        &data_path(program, "books", &["title"]),
        SavedValue::Str("Mort".into()),
    );
    store
}

fn read(
    program: &marrow_check::CheckedRuntimeProgram,
    store: &TreeStore,
    entry: &str,
    id: i64,
) -> Value {
    run_entry(store, checked_entry!(program, entry, Value::Int(id)))
        .expect("run")
        .value
        .expect("a value")
}

#[test]
fn an_absent_saved_read_binds_the_empty_optional_to_a_const() {
    let program = program();
    let store = store_with_title(&program, 1); // subtitle absent
    assert_eq!(
        read(&program, &store, "test::const_slot", 1),
        Value::Str("NONE".into())
    );
}

#[test]
fn an_absent_saved_read_binds_the_empty_optional_to_a_var() {
    let program = program();
    let store = store_with_title(&program, 1);
    assert_eq!(
        read(&program, &store, "test::var_slot", 1),
        Value::Str("NONE".into())
    );
}

#[test]
fn an_absent_saved_read_flows_into_a_t_optional_argument() {
    let program = program();
    let store = store_with_title(&program, 1);
    assert_eq!(
        read(&program, &store, "test::arg_from_read", 1),
        Value::Str("NONE".into())
    );
}

#[test]
fn an_absent_call_flows_into_a_t_optional_argument() {
    let program = program();
    let store = store_with_title(&program, 1);
    assert_eq!(
        read(&program, &store, "test::arg_from_call", 1),
        Value::Str("NONE".into())
    );
}

#[test]
fn returning_absent_through_a_helper_chain_resolves_at_the_top() {
    let program = program();
    let store = store_with_title(&program, 1);
    assert_eq!(
        read(&program, &store, "test::chain_top", 1),
        Value::Str("NONE".into())
    );
}

#[test]
fn a_present_saved_read_binds_its_value_through_the_optional_slot() {
    let program = program();
    let store = store_with_title(&program, 1);
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::write_subtitle",
            Value::Int(1),
            Value::Str("A Discworld Novel".into())
        ),
    )
    .expect("write subtitle");

    for entry in [
        "test::const_slot",
        "test::var_slot",
        "test::arg_from_read",
        "test::arg_from_call",
        "test::chain_top",
    ] {
        assert_eq!(
            read(&program, &store, entry, 1),
            Value::Str("A Discworld Novel".into()),
            "{entry} should read the present value",
        );
    }
}

/// Materializing a whole record whose durable required field is missing is invalid
/// attached data: it stays a fatal `run.absent_element`, never collapsing to the
/// empty optional, even though the record node exists.
const REQUIRED_RECORD: &str = "\
resource Rec
    required a: string
    required b: string
store ^recs(id: int): Rec

pub fn read_rec(id: int): string
    const r: Rec = ^recs(id) ?? Rec(a: \"\", b: \"\")
    return r.a
";

#[test]
fn a_required_read_of_genuinely_absent_required_data_stays_fatal() {
    let program = checked_program(REQUIRED_RECORD);
    let store = empty_store();
    // The record exists but its required `b` was never written: reading it back is a
    // fatal invalid-attached-data fault, not the empty optional.
    write_data_value(
        &program,
        &store,
        "recs",
        &[SavedKey::Int(1)],
        &data_path(&program, "recs", &["a"]),
        SavedValue::Str("present".into()),
    );
    assert_run_error(
        run_entry(
            &store,
            checked_entry!(&program, "test::read_rec", Value::Int(1)),
        ),
        RUN_ABSENT,
    );
}

/// A widget with a sparse `sku` under a unique index. Assigning a materialized
/// absent `T?` local (not the `absent` literal) clears the node and its index entry.
const WIDGET_MATERIALIZED_CLEAR: &str = "\
resource Widget
    sku: string
store ^widgets(id: int): Widget

    index bySku(sku) unique

pub fn seed(id: int, sku: string)
    ^widgets(id).sku = sku

fn skuOfOther(id: int): string?
    return ^widgets(id).sku

pub fn clearWithMaterializedAbsent(id: int, absentId: int)
    const cleared: string? = skuOfOther(absentId)
    ^widgets(id).sku = cleared

pub fn skuOf(id: int): string
    return ^widgets(id).sku ?? \"<none>\"

pub fn idBySku(sku: string): int
    if const w = ^widgets.bySku(sku)
        return key(w)
    return -1
";

#[test]
fn saving_a_materialized_absent_optional_clears_the_node_and_index() {
    let program = checked_program(WIDGET_MATERIALIZED_CLEAR);
    let store = empty_store();
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::seed",
            Value::Int(1),
            Value::Str("abc".into())
        ),
    )
    .expect("seed");

    // Materialize an absent `T?` from a missing record, then save it over the node.
    run_entry(
        &store,
        checked_entry!(
            &program,
            "test::clearWithMaterializedAbsent",
            Value::Int(1),
            Value::Int(999)
        ),
    )
    .expect("clear via materialized absent");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::skuOf", Value::Int(1))
        )
        .expect("read sku")
        .value,
        Some(Value::Str("<none>".into()))
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::idBySku", Value::Str("abc".into()))
        )
        .expect("lookup")
        .value,
        Some(Value::Int(-1))
    );
}
