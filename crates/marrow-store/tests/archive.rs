//! A backup archive is the store's canonical (path, value) stream, framed so it
//! restores byte-for-byte into any backend.

use std::io::Cursor;

use marrow_store::archive::{read_archive, write_archive};
use marrow_store::mem::{MemStore, StoreError};
use marrow_store::path::{PathSegment, SavedKey, encode_path};
use marrow_store::value::{SavedValue, encode_value};

/// The encoded path `^books(id).title`.
fn book_title(id: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::Field("title".into()),
    ])
}

#[test]
fn an_archive_round_trips_through_a_fresh_store() {
    let mut source = MemStore::new();
    source.write(
        &book_title(1),
        encode_value(&SavedValue::Str("Dune".into())),
    );
    source.write(
        &book_title(2),
        encode_value(&SavedValue::Str("Sand".into())),
    );

    let mut buffer = Vec::new();
    let written = write_archive(&source, &mut buffer).expect("write archive");
    assert_eq!(written, 2);

    let mut restored = MemStore::new();
    let read = read_archive(&mut Cursor::new(&buffer), &mut restored).expect("read archive");
    assert_eq!(read, 2);

    // The restored store reproduces the source's dump byte-for-byte.
    assert_eq!(restored.scan(&[], usize::MAX), source.scan(&[], usize::MAX));
}

#[test]
fn an_empty_store_archives_and_restores_as_empty() {
    let source = MemStore::new();
    let mut buffer = Vec::new();
    assert_eq!(write_archive(&source, &mut buffer).expect("write"), 0);

    let mut restored = MemStore::new();
    assert_eq!(
        read_archive(&mut Cursor::new(&buffer), &mut restored).expect("read"),
        0
    );
    assert!(restored.roots().expect("roots").is_empty());
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
fn equal_data_produces_identical_archives() {
    // The archive is the store's ordered stream behind a fixed header, so equal
    // data always serializes to identical bytes.
    let mut a = MemStore::new();
    a.write(
        &book_title(1),
        encode_value(&SavedValue::Str("Dune".into())),
    );
    let mut b = MemStore::new();
    b.write(
        &book_title(1),
        encode_value(&SavedValue::Str("Dune".into())),
    );

    let mut buffer_a = Vec::new();
    let mut buffer_b = Vec::new();
    write_archive(&a, &mut buffer_a).expect("write a");
    write_archive(&b, &mut buffer_b).expect("write b");

    assert_eq!(buffer_a, buffer_b);
    assert!(buffer_a.starts_with(b"MARROW\0A"), "{:?}", &buffer_a[..8]);
}
