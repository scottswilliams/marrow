//! A raw archive is the saved-path stream, framed so it replays byte-for-byte into
//! any backend.

use std::io::Cursor;

use marrow_store::archive::{read_archive, write_archive};
use marrow_store::backend::{Backend, StoreError};
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, encode_path};
use marrow_store::value::{SavedValue, encode_value};

fn raw_stream_fixture(store: &MemStore) -> marrow_store::backend::ScanPage {
    let page = store.scan(&[], 32);
    assert!(!page.truncated, "archive fixture exceeded its scan limit");
    page
}

/// The encoded path `^books(id).title`.
fn book_title(id: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::Field("title".into()),
    ])
}

/// Encode a value known to be in range, unwrapping the canonical bytes.
fn encoded(value: &SavedValue) -> Vec<u8> {
    encode_value(value).expect("in-range value encodes")
}

#[test]
fn an_archive_round_trips_through_a_fresh_store() {
    let mut source = MemStore::new();
    source.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    source.write(&book_title(2), encoded(&SavedValue::Str("Sand".into())));

    let mut buffer = Vec::new();
    let written = write_archive(&source, &mut buffer).expect("write archive");
    assert_eq!(written, 2);

    let mut target = MemStore::new();
    let read = read_archive(&mut Cursor::new(&buffer), &mut target).expect("read archive");
    assert_eq!(read, 2);

    assert_eq!(raw_stream_fixture(&target), raw_stream_fixture(&source));
}

#[test]
fn an_empty_store_archives_and_reads_as_empty() {
    let source = MemStore::new();
    let mut buffer = Vec::new();
    assert_eq!(write_archive(&source, &mut buffer).expect("write"), 0);

    let mut target = MemStore::new();
    assert_eq!(
        read_archive(&mut Cursor::new(&buffer), &mut target).expect("read"),
        0
    );
    assert!(target.roots().expect("roots").is_empty());
}

#[test]
fn a_non_archive_input_is_a_typed_error() {
    let mut store = MemStore::new();
    let result = read_archive(&mut Cursor::new(b"not an archive".to_vec()), &mut store);
    assert!(
        matches!(result, Err(StoreError::Corruption { .. })),
        "{result:?}"
    );
}

#[test]
fn an_unsupported_version_is_rejected() {
    let source = MemStore::new();
    let mut buffer = Vec::new();
    write_archive(&source, &mut buffer).expect("write");
    // The version is the little-endian u32 right after the 8-byte magic.
    buffer[8] = 2;
    let mut store = MemStore::new();
    let result = read_archive(&mut Cursor::new(&buffer), &mut store);
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
fn a_truncated_archive_read_rolls_the_target_back_whole() {
    // A two-record archive cut off mid-second-record is the worst case for a
    // replay: the first record's write already landed before the truncation is
    // hit. The read runs the replay in one transaction, so the target must end
    // exactly as it began — all-or-nothing, not half-written.
    let mut source = MemStore::new();
    source.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    source.write(&book_title(2), encoded(&SavedValue::Str("Sand".into())));
    let mut archive = Vec::new();
    assert_eq!(write_archive(&source, &mut archive).expect("write"), 2);
    // Drop the final byte so the second record's value chunk ends mid-read; the
    // header and first record stay intact, so the replay writes record one and
    // only then meets the corruption.
    archive.pop();

    // A non-empty target whose prior contents must survive the failed replay.
    let mut target = MemStore::new();
    target.write(&book_title(9), encoded(&SavedValue::Str("Keep".into())));
    let before = raw_stream_fixture(&target);

    let result = read_archive(&mut Cursor::new(&archive), &mut target);
    assert!(
        matches!(result, Err(StoreError::Corruption { .. })),
        "a truncated archive is a typed corruption error: {result:?}"
    );
    assert_eq!(
        raw_stream_fixture(&target),
        before,
        "the target is rolled back to its prior state, with no half-written record"
    );
}

#[test]
fn an_archive_with_trailing_records_after_count_is_rejected() {
    let mut source = MemStore::new();
    source.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    source.write(&book_title(2), encoded(&SavedValue::Str("Sand".into())));
    let mut archive = Vec::new();
    assert_eq!(write_archive(&source, &mut archive).expect("write"), 2);
    // The little-endian record count starts after the 8-byte magic and 4-byte
    // version. Forcing it down to 1 leaves one whole record as trailing bytes.
    archive[12] = 1;

    let mut target = MemStore::new();
    target.write(&book_title(9), encoded(&SavedValue::Str("Keep".into())));
    let before = raw_stream_fixture(&target);

    let result = read_archive(&mut Cursor::new(&archive), &mut target);
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
fn equal_data_produces_identical_archives() {
    // The archive is the store's ordered stream behind a fixed header, so equal
    // data always serializes to identical bytes.
    let mut a = MemStore::new();
    a.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));
    let mut b = MemStore::new();
    b.write(&book_title(1), encoded(&SavedValue::Str("Dune".into())));

    let mut buffer_a = Vec::new();
    let mut buffer_b = Vec::new();
    write_archive(&a, &mut buffer_a).expect("write a");
    write_archive(&b, &mut buffer_b).expect("write b");

    assert_eq!(buffer_a, buffer_b);
    assert!(buffer_a.starts_with(b"MARROW\0A"), "{:?}", &buffer_a[..8]);
}
