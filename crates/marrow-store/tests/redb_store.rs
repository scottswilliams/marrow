//! Native (redb) store behavior beyond the shared backend contract: durable
//! persistence, single-writer locking, read-only opens, and dumps that round
//! trip between memory and native storage. The contract itself is exercised by
//! the per-backend conformance tests inside the crate.
#![cfg(feature = "native")]

use marrow_store::backend::{Backend, ScanPage, StoreError};
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, encode_path};
use marrow_store::redb::RedbStore;

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

#[test]
fn redb_rejects_a_second_writer() {
    let dir = tempfile::tempdir().expect("create a temp dir");
    let path = dir.path().join("store.redb");
    let _first = RedbStore::open(&path).expect("open the first writer");
    // A second writer for the same open file is refused with a typed lock error.
    match RedbStore::open(&path) {
        Ok(_) => panic!("a second writer must be refused"),
        Err(error) => assert_eq!(error.code(), "store.locked"),
    }
}

#[test]
fn open_read_only_reads_an_existing_store_without_creating_one() {
    let dir = tempfile::tempdir().expect("create a temp dir");
    let path = dir.path().join("store.redb");

    // Read-only opening a path that does not exist is an error and creates nothing.
    assert!(RedbStore::open_read_only(&path).is_err());
    assert!(!path.exists(), "read-only open must not create the store");

    // Create and populate the store, then reopen it read-only and read it back.
    {
        let mut store = RedbStore::open(&path).expect("create");
        store.write(b"k", b"v".to_vec()).expect("write");
    }
    let store = RedbStore::open_read_only(&path).expect("open read-only");
    assert_eq!(store.read(b"k").expect("read"), Some(b"v".to_vec()));
    // A read-only open neither corrupts the store nor keeps it locked: it reopens
    // read-write afterward.
    drop(store);
    RedbStore::open(&path).expect("reopen read-write after a read-only open");
}

#[test]
fn open_read_only_allows_simultaneous_readers() {
    let dir = tempfile::tempdir().expect("create a temp dir");
    let path = dir.path().join("store.redb");

    {
        let mut store = RedbStore::open(&path).expect("create");
        store.write(b"k", b"v".to_vec()).expect("write");
    }

    let first = RedbStore::open_read_only(&path).expect("open first read-only");
    let second = RedbStore::open_read_only(&path).expect("open second read-only");

    assert_eq!(first.read(b"k").expect("read first"), Some(b"v".to_vec()));
    assert_eq!(second.read(b"k").expect("read second"), Some(b"v".to_vec()));
}

#[test]
fn live_read_only_handle_refuses_read_write_open_until_dropped() {
    let dir = tempfile::tempdir().expect("create a temp dir");
    let path = dir.path().join("store.redb");

    {
        let mut store = RedbStore::open(&path).expect("create");
        store.write(b"k", b"v".to_vec()).expect("write");
    }

    let reader = RedbStore::open_read_only(&path).expect("open read-only");
    match RedbStore::open(&path) {
        Ok(_) => panic!("a writer must be refused while a read-only handle is alive"),
        Err(error) => assert_eq!(error.code(), "store.locked"),
    }

    drop(reader);
    let store = RedbStore::open(&path).expect("reopen read-write after dropping reader");
    assert_eq!(store.read(b"k").expect("read"), Some(b"v".to_vec()));
}

#[test]
fn open_read_only_refuses_write_capability_operations() {
    let dir = tempfile::tempdir().expect("create a temp dir");
    let path = dir.path().join("store.redb");

    {
        let mut store = RedbStore::open(&path).expect("create");
        store
            .write(b"keep", b"original".to_vec())
            .expect("seed keep");
        store
            .write(b"delete-target", b"still here".to_vec())
            .expect("seed delete target");
    }

    let assert_read_only = |result: Result<(), StoreError>, op| match result {
        Err(error) => assert_eq!(error.code(), "store.read_only", "{op} error"),
        Ok(()) => panic!("{op} must be refused on a read-only store"),
    };

    {
        let mut store = RedbStore::open_read_only(&path).expect("open read-only for write");
        assert_read_only(store.write(b"keep", b"changed".to_vec()), "write");
    }
    {
        let mut store = RedbStore::open_read_only(&path).expect("open read-only for delete");
        assert_read_only(store.delete(b"delete-target"), "delete");
    }
    {
        let mut store = RedbStore::open_read_only(&path).expect("open read-only for begin");
        assert_read_only(store.begin(), "begin");
    }

    let store = RedbStore::open(&path).expect("reopen read-write");
    assert_eq!(
        store.read(b"keep").expect("read keep"),
        Some(b"original".to_vec())
    );
    assert_eq!(
        store.read(b"delete-target").expect("read delete target"),
        Some(b"still here".to_vec())
    );
    assert_eq!(store.read(b"new").expect("read absent new"), None);
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
    // The dump is a backend-independent path/value stream: a memory
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
