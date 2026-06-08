//! Cross-root transaction atomicity: a single `transaction` block spanning two
//! distinct saved roots commits both or rolls back both. Single-root rollback is
//! covered elsewhere; this pins the cross-root boundary, where a fault mid-block
//! must leave every touched root at its pre-transaction value.

#[macro_use]
mod support;

use support::*;

use marrow_run::{RUN_DIVIDE_BY_ZERO, Value};
use marrow_store::tree::TreeStore;

/// Two independent saved roots plus a transaction that writes to both, optionally
/// faulting after staging both writes. Helpers read each root's presence and field
/// so a write's effect (or its rollback) is observable per root.
const TWO_ROOTS: &str = "\
resource Book at ^books(id: int)
    required title: string

resource Author at ^authors(id: int)
    required name: string

pub fn seed()
    ^books(1).title = \"old-book\"
    ^authors(1).name = \"old-author\"

pub fn write_both()
    transaction
        ^books(1).title = \"new-book\"
        ^authors(1).name = \"new-author\"

pub fn write_both_then_fault()
    transaction
        ^books(1).title = \"new-book\"
        ^authors(1).name = \"new-author\"
        const boom = 1 / 0

pub fn create_both_then_fault()
    transaction
        ^books(2).title = \"fresh-book\"
        ^authors(2).name = \"fresh-author\"
        const boom = 1 / 0

pub fn has_book(id: int): bool
    return exists(^books(id))

pub fn has_author(id: int): bool
    return exists(^authors(id))

pub fn book_title(id: int): string
    return ^books(id).title

pub fn author_name(id: int): string
    return ^authors(id).name
";

#[test]
fn a_cross_root_transaction_commits_both_roots_on_normal_exit() {
    // The success case: a transaction that writes both roots persists both writes,
    // proving the cross-root commit is not a single-root commit plus a lost sibling.
    let program = checked_program(TWO_ROOTS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::write_both")).expect("commit both roots");
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::book_title", Value::Int(1))
        )
        .expect("read book")
        .value,
        Some(Value::Str("new-book".into())),
        "the books root committed"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::author_name", Value::Int(1))
        )
        .expect("read author")
        .value,
        Some(Value::Str("new-author".into())),
        "the authors root committed in the same transaction"
    );
}

#[test]
fn a_cross_root_fault_rolls_back_both_roots_to_their_pre_transaction_values() {
    // The atomicity case: a transaction mutates both roots, then faults. Both roots
    // revert to the values they held before the transaction — neither sibling keeps a
    // half-applied write.
    let program = checked_program(TWO_ROOTS);
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed both roots");

    let result = run_entry(
        &store,
        checked_entry!(&program, "test::write_both_then_fault"),
    );
    assert_run_error(result, RUN_DIVIDE_BY_ZERO);

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::book_title", Value::Int(1))
        )
        .expect("read book")
        .value,
        Some(Value::Str("old-book".into())),
        "the books root rolled back to its pre-transaction value"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::author_name", Value::Int(1))
        )
        .expect("read author")
        .value,
        Some(Value::Str("old-author".into())),
        "the authors root rolled back to its pre-transaction value"
    );
}

#[test]
fn a_cross_root_fault_leaves_both_freshly_created_roots_absent() {
    // When both roots are created (not just mutated) inside the faulting transaction,
    // the rollback leaves neither record behind: a cross-root create is all-or-nothing.
    let program = checked_program(TWO_ROOTS);
    let store = TreeStore::memory();

    let result = run_entry(
        &store,
        checked_entry!(&program, "test::create_both_then_fault"),
    );
    assert_run_error(result, RUN_DIVIDE_BY_ZERO);

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_book", Value::Int(2))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the new books record rolled back"
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::has_author", Value::Int(2))
        )
        .expect("presence check")
        .value,
        Some(Value::Bool(false)),
        "the new authors record rolled back with its sibling"
    );
}
