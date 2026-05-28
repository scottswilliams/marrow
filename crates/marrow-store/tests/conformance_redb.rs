//! The native (redb) store satisfies the same backend conformance suite as the
//! in-memory store — one contract, two backends.
#![cfg(feature = "native")]

use marrow_store::backend::Backend;
use marrow_store::conformance;
use marrow_store::mem::{MemStore, ScanPage};
use marrow_store::path::{PathSegment, SavedKey, encode_path};
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

/// The encoded path `^books(id).title`.
fn book_title(id: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::Field("title".into()),
    ])
}

/// Dump the whole store as the portable path/value stream.
fn dump(store: &dyn Backend) -> ScanPage {
    store.scan(&[], usize::MAX).expect("dump")
}

/// Restore a dump into `store` by replaying its (path, value) pairs.
fn restore(store: &mut dyn Backend, source: &ScanPage) {
    for (path, value) in &source.entries {
        store.write(path, value.clone()).expect("restore write");
    }
}

#[test]
fn dumps_round_trip_between_memory_and_native() {
    // The dump is a backend-independent path/value stream (roadmap §9): a memory
    // dump restores byte-for-byte into native storage, and back again.
    let dir = tempfile::tempdir().expect("create a temp dir");

    let mut mem = MemStore::new();
    restore(&mut mem, &{
        let mut seed = ScanPage::default();
        seed.entries.push((book_title(1), b"Dune".to_vec()));
        seed.entries.push((book_title(2), b"Sand".to_vec()));
        seed
    });
    let source = dump(&mem);

    // Memory -> native reproduces the dump exactly.
    let mut redb = RedbStore::open(&dir.path().join("from-mem.redb")).expect("open redb");
    restore(&mut redb, &source);
    assert_eq!(dump(&redb), source, "native reproduces the memory dump");

    // Native -> memory reproduces it too.
    let mut mem_again = MemStore::new();
    restore(&mut mem_again, &dump(&redb));
    assert_eq!(
        dump(&mem_again),
        source,
        "memory reproduces the native dump"
    );
}
