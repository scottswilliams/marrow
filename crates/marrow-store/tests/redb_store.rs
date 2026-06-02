//! Native (redb) store behavior beyond the shared backend contract: durable
//! persistence, single-writer locking, read-only opens, and raw record copies
//! between memory and native storage. The contract itself is exercised by the
//! per-backend conformance tests inside the crate.
#![cfg(feature = "native")]

use marrow_store::backend::{Backend, StoreError};
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, encode_path};
use marrow_store::redb::RedbStore;

const RAW_COPY_SCAN_LIMIT: usize = 2;

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
fn read_only_handle_blocks_read_write_open_before_drop() {
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

fn collect_raw_records(store: &dyn Backend) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut records = Vec::new();
    let mut cursor = None;
    loop {
        let page = match cursor.as_deref() {
            Some(cursor) => store
                .scan_after(&[], cursor, RAW_COPY_SCAN_LIMIT)
                .expect("scan next raw record page"),
            None => store
                .scan(&[], RAW_COPY_SCAN_LIMIT)
                .expect("scan raw records"),
        };
        let next_cursor = page.entries.last().map(|(path, _)| path.clone());
        records.extend(page.entries);
        if !page.truncated {
            return records;
        }
        if let (Some(previous), Some(next)) = (cursor.as_ref(), next_cursor.as_ref()) {
            assert!(
                next > previous,
                "truncated scan did not advance the raw record cursor"
            );
        }
        cursor = next_cursor;
        assert!(cursor.is_some(), "truncated scan returned no record cursor");
    }
}

fn copy_raw_records(store: &mut dyn Backend, source: &[(Vec<u8>, Vec<u8>)]) {
    for (path, value) in source {
        store.write(path, value.clone()).expect("copy raw record");
    }
}

#[test]
fn raw_records_copy_between_memory_and_native() {
    let dir = tempfile::tempdir().expect("create a temp dir");

    let mut mem = MemStore::new();
    let seed = vec![
        (book_title(1), b"Dune".to_vec()),
        (book_title(2), b"Sand".to_vec()),
    ];
    copy_raw_records(&mut mem, &seed);
    let source = collect_raw_records(&mem);
    assert_eq!(source.len(), 2);

    let mut redb = RedbStore::open(&dir.path().join("from-mem.redb")).expect("open redb");
    copy_raw_records(&mut redb, &source);
    assert_eq!(
        collect_raw_records(&redb),
        source,
        "native reproduces memory raw records"
    );

    let mut mem_again = MemStore::new();
    let native_records = collect_raw_records(&redb);
    copy_raw_records(&mut mem_again, &native_records);
    assert_eq!(
        collect_raw_records(&mem_again),
        source,
        "memory reproduces native raw records"
    );
}
