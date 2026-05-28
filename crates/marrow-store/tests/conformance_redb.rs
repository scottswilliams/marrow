//! The native (redb) store satisfies the same backend conformance suite as the
//! in-memory store — one contract, two backends.
#![cfg(feature = "native")]

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
