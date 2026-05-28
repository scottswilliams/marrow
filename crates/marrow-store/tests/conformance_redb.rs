//! The native (redb) store satisfies the same backend conformance suite as the
//! in-memory store — one contract, two backends.
#![cfg(feature = "native")]

use marrow_store::backend::Backend;
use marrow_store::conformance;
use marrow_store::redb::RedbStore;

#[test]
fn redb_store_passes_the_conformance_suite() {
    let dir = tempfile::tempdir().expect("create a temp dir");
    let mut counter = 0;
    conformance::run_all(|| {
        // Each law gets a fresh redb file in the shared temp dir; the dir (and
        // its files) outlives every store, dropping only when the test ends.
        counter += 1;
        let path = dir.path().join(format!("store-{counter}.redb"));
        RedbStore::open(&path).expect("open a fresh redb store")
    });
}

#[test]
fn redb_persists_and_reopens() {
    let dir = tempfile::tempdir().expect("create a temp dir");
    let path = dir.path().join("store.redb");
    {
        let mut store = RedbStore::open(&path).expect("open");
        store.write(b"k", b"v".to_vec()).expect("write");
    } // the store drops here, closing the file
    // Reopening checks the recorded format version and sees the persisted data.
    let store = RedbStore::open(&path).expect("reopen");
    assert_eq!(store.read(b"k").expect("read"), Some(b"v".to_vec()));
}
