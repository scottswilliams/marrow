//! The in-memory store satisfies the `Backend` contract, including transaction
//! savepoints (`begin`/`commit`/`rollback`, with nesting as savepoints).
//!
//! These exercise [`MemStore`] through `&mut dyn Backend`, the way a generic
//! consumer reaches a backend (its inherent methods stay available to direct
//! callers and take priority on a concrete value).

use marrow_store::backend::Backend;
use marrow_store::mem::MemStore;

#[test]
fn reads_what_it_writes_through_the_trait() {
    let mut store = MemStore::new();
    let store: &mut dyn Backend = &mut store;
    store.write(b"a", b"1".to_vec()).unwrap();
    assert_eq!(store.read(b"a").unwrap(), Some(b"1".to_vec()));
    assert_eq!(store.read(b"b").unwrap(), None);
    store.delete(b"a").unwrap();
    assert_eq!(store.read(b"a").unwrap(), None);
}

#[test]
fn a_committed_transaction_keeps_its_writes() {
    let mut store = MemStore::new();
    let store: &mut dyn Backend = &mut store;
    store.begin().unwrap();
    store.write(b"a", b"1".to_vec()).unwrap();
    // Read-your-writes inside the transaction.
    assert_eq!(store.read(b"a").unwrap(), Some(b"1".to_vec()));
    store.commit().unwrap();
    assert_eq!(store.read(b"a").unwrap(), Some(b"1".to_vec()));
}

#[test]
fn a_rolled_back_transaction_discards_its_writes() {
    let mut store = MemStore::new();
    let store: &mut dyn Backend = &mut store;
    store.write(b"keep", b"0".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"a", b"1".to_vec()).unwrap();
    store.rollback().unwrap();
    assert_eq!(store.read(b"a").unwrap(), None, "the staged write is gone");
    assert_eq!(
        store.read(b"keep").unwrap(),
        Some(b"0".to_vec()),
        "the pre-transaction value remains"
    );
}

#[test]
fn nested_transactions_are_savepoints() {
    let mut store = MemStore::new();
    let store: &mut dyn Backend = &mut store;
    store.begin().unwrap();
    store.write(b"outer", b"1".to_vec()).unwrap();
    store.begin().unwrap();
    store.write(b"inner", b"2".to_vec()).unwrap();
    store.rollback().unwrap(); // undo the inner savepoint only
    assert_eq!(store.read(b"inner").unwrap(), None, "inner rolled back");
    assert_eq!(
        store.read(b"outer").unwrap(),
        Some(b"1".to_vec()),
        "the outer write survives the inner rollback"
    );
    store.commit().unwrap(); // commit the outer
    assert_eq!(store.read(b"outer").unwrap(), Some(b"1".to_vec()));
}
