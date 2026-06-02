//! Debug/admin-only raw saved-path archive checks.

use std::io::Cursor;

use marrow_store::backend::{Backend, StoreError};
use marrow_store::debug_admin::{read_raw_saved_path_archive, write_raw_saved_path_archive};
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, encode_path};
use marrow_store::value::{SavedValue, encode_value};

fn raw_stream_fixture(store: &MemStore) -> marrow_store::backend::ScanPage {
    let page = store.scan(&[], 32);
    assert!(!page.truncated, "archive fixture exceeded its scan limit");
    page
}

fn book_title(id: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::Field("title".into()),
    ])
}

fn encoded(value: &SavedValue) -> Vec<u8> {
    encode_value(value).expect("in-range value encodes")
}

#[test]
fn debug_admin_archive_round_trips_through_a_fresh_store() {
    let mut source = MemStore::new();
    source.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    source.write(&book_title(2), encoded(&SavedValue::Str("Sand".into())));

    let mut buffer = Vec::new();
    let written = write_raw_saved_path_archive(&source, &mut buffer).expect("write raw archive");
    assert_eq!(written, 2);

    let mut target = MemStore::new();
    let read = read_raw_saved_path_archive(&mut Cursor::new(&buffer), &mut target)
        .expect("read raw archive");
    assert_eq!(read, 2);

    assert_eq!(raw_stream_fixture(&target), raw_stream_fixture(&source));
}

#[test]
fn empty_debug_admin_archive_reads_as_empty() {
    let source = MemStore::new();
    let mut buffer = Vec::new();
    assert_eq!(
        write_raw_saved_path_archive(&source, &mut buffer).expect("write"),
        0
    );

    let mut target = MemStore::new();
    assert_eq!(
        read_raw_saved_path_archive(&mut Cursor::new(&buffer), &mut target).expect("read"),
        0
    );
    assert!(target.roots().expect("roots").is_empty());
}

#[test]
fn non_archive_debug_admin_input_is_a_typed_error() {
    let mut store = MemStore::new();
    let result =
        read_raw_saved_path_archive(&mut Cursor::new(b"not an archive".to_vec()), &mut store);
    assert!(
        matches!(result, Err(StoreError::Corruption { .. })),
        "{result:?}"
    );
}

#[test]
fn unsupported_debug_admin_archive_version_is_rejected() {
    let source = MemStore::new();
    let mut buffer = Vec::new();
    write_raw_saved_path_archive(&source, &mut buffer).expect("write");
    buffer[8] = 2;

    let mut store = MemStore::new();
    let result = read_raw_saved_path_archive(&mut Cursor::new(&buffer), &mut store);
    assert!(
        matches!(
            result,
            Err(StoreError::FormatVersion {
                found: 2,
                supported: 1
            })
        ),
        "{result:?}"
    );
}

#[test]
fn truncated_debug_admin_archive_rolls_the_target_back_whole() {
    let mut source = MemStore::new();
    source.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    source.write(&book_title(2), encoded(&SavedValue::Str("Sand".into())));
    let mut archive = Vec::new();
    assert_eq!(
        write_raw_saved_path_archive(&source, &mut archive).expect("write"),
        2
    );
    archive.pop();

    let mut target = MemStore::new();
    target.write(&book_title(9), encoded(&SavedValue::Str("Keep".into())));
    let before = raw_stream_fixture(&target);

    let result = read_raw_saved_path_archive(&mut Cursor::new(&archive), &mut target);
    assert!(
        matches!(result, Err(StoreError::Corruption { .. })),
        "a truncated archive is a typed corruption error: {result:?}"
    );
    assert_eq!(
        raw_stream_fixture(&target),
        before,
        "the target is rolled back with no half-written record"
    );
}

#[test]
fn debug_admin_archive_with_trailing_records_after_count_is_rejected() {
    let mut source = MemStore::new();
    source.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    source.write(&book_title(2), encoded(&SavedValue::Str("Sand".into())));
    let mut archive = Vec::new();
    assert_eq!(
        write_raw_saved_path_archive(&source, &mut archive).expect("write"),
        2
    );
    archive[12] = 1;

    let mut target = MemStore::new();
    target.write(&book_title(9), encoded(&SavedValue::Str("Keep".into())));
    let before = raw_stream_fixture(&target);

    let result = read_raw_saved_path_archive(&mut Cursor::new(&archive), &mut target);
    assert!(
        matches!(result, Err(StoreError::Corruption { .. })),
        "an archive with body bytes after its declared record count is corrupt: {result:?}"
    );
    assert_eq!(
        raw_stream_fixture(&target),
        before,
        "the archive read rolls back instead of committing a declared-count prefix"
    );
}

#[test]
fn equal_debug_admin_data_produces_identical_raw_archives() {
    let mut a = MemStore::new();
    a.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    let mut b = MemStore::new();
    b.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));

    let mut buffer_a = Vec::new();
    let mut buffer_b = Vec::new();
    write_raw_saved_path_archive(&a, &mut buffer_a).expect("write a");
    write_raw_saved_path_archive(&b, &mut buffer_b).expect("write b");

    assert_eq!(buffer_a, buffer_b);
    assert!(buffer_a.starts_with(b"MARROW\0A"), "{:?}", &buffer_a[..8]);
}

#[cfg(feature = "native")]
#[test]
fn debug_admin_archive_can_transfer_between_memory_and_native() {
    use marrow_store::redb::RedbStore;

    let dir = tempfile::tempdir().expect("create a temp dir");
    let path = dir.path().join("debug-admin.redb");

    let mut source = MemStore::new();
    source.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    source.write(&book_title(2), encoded(&SavedValue::Str("Sand".into())));

    let mut buffer = Vec::new();
    assert_eq!(
        write_raw_saved_path_archive(&source, &mut buffer).expect("write from memory"),
        2
    );

    let mut redb = RedbStore::open(&path).expect("open redb");
    assert_eq!(
        read_raw_saved_path_archive(&mut Cursor::new(&buffer), &mut redb).expect("read into redb"),
        2
    );

    let mut redb_buffer = Vec::new();
    assert_eq!(
        write_raw_saved_path_archive(&redb, &mut redb_buffer).expect("write from redb"),
        2
    );

    let mut target = MemStore::new();
    assert_eq!(
        read_raw_saved_path_archive(&mut Cursor::new(&redb_buffer), &mut target)
            .expect("read into memory"),
        2
    );
    assert_eq!(raw_stream_fixture(&target), raw_stream_fixture(&source));
}
