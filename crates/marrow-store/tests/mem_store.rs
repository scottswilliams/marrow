//! In-memory store behavior: write/read round-trips, the four presence states,
//! and subtree delete. These are the first store-conformance laws.

use marrow_store::backend::Presence;
use marrow_store::mem::MemStore;
use marrow_store::path::{PathSegment, SavedKey, encode_path};

/// The encoded path `^books(id)`.
fn book(id: i64) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
    ])
}

/// The encoded path `^books(id).<field>`.
fn book_field(id: i64, field: &str) -> Vec<u8> {
    encode_path(&[
        PathSegment::Root("books".into()),
        PathSegment::RecordKey(SavedKey::Int(id)),
        PathSegment::Field(field.into()),
    ])
}

#[test]
fn write_then_read_returns_the_value() {
    let mut store = MemStore::new();
    store.write(&book_field(1, "title"), b"Dune".to_vec());
    assert_eq!(store.read(&book_field(1, "title")), Some(&b"Dune"[..]));
}

#[test]
fn reading_an_absent_path_returns_none() {
    let store = MemStore::new();
    assert_eq!(store.read(&book_field(1, "title")), None);
}

#[test]
fn writing_replaces_the_existing_value() {
    let mut store = MemStore::new();
    store.write(&book_field(1, "title"), b"draft".to_vec());
    store.write(&book_field(1, "title"), b"final".to_vec());
    assert_eq!(store.read(&book_field(1, "title")), Some(&b"final"[..]));
}

#[test]
fn presence_reports_all_four_states() {
    let mut store = MemStore::new();
    assert_eq!(store.presence(&book(1)), Ok(Presence::Absent));

    // A whole-resource value at the record, no children yet.
    store.write(&book(1), b"whole".to_vec());
    assert_eq!(store.presence(&book(1)), Ok(Presence::ValueOnly));

    // A field below the record adds children.
    store.write(&book_field(1, "title"), b"Dune".to_vec());
    assert_eq!(store.presence(&book(1)), Ok(Presence::ValueAndChildren));

    // A different record with only a field below it, no whole value.
    store.write(&book_field(2, "title"), b"Sand".to_vec());
    assert_eq!(store.presence(&book(2)), Ok(Presence::ChildrenOnly));
}

#[test]
fn delete_removes_the_value_and_its_subtree() {
    let mut store = MemStore::new();
    store.write(&book(1), b"whole".to_vec());
    store.write(&book_field(1, "title"), b"Dune".to_vec());
    store.write(&book_field(1, "author"), b"Herbert".to_vec());
    store.write(&book_field(2, "title"), b"Other".to_vec());

    store.delete(&book(1));

    assert_eq!(store.presence(&book(1)), Ok(Presence::Absent));
    assert_eq!(store.read(&book_field(1, "title")), None);
    assert_eq!(store.read(&book_field(1, "author")), None);
    // A sibling record is untouched.
    assert_eq!(store.read(&book_field(2, "title")), Some(&b"Other"[..]));
}

#[test]
fn deleting_an_absent_path_is_a_no_op() {
    let mut store = MemStore::new();
    store.write(&book_field(2, "title"), b"Other".to_vec());
    store.delete(&book(1));
    assert_eq!(store.read(&book_field(2, "title")), Some(&b"Other"[..]));
}

#[test]
fn dump_and_restore_reproduce_the_store() {
    let mut store = MemStore::new();
    store.write(&book(1), b"whole".to_vec());
    store.write(&book_field(1, "title"), b"Dune".to_vec());
    store.write(&book_field(1, "author"), b"Herbert".to_vec());
    store.write(&book_field(2, "title"), b"Sand".to_vec());

    // Dumping from the empty prefix yields every entry in Marrow order — the
    // portable path/value stream.
    let dump = store.scan(&[], usize::MAX);
    assert_eq!(dump.entries.len(), 4);

    // Restoring re-writes the encoded pairs into a fresh store.
    let mut restored = MemStore::new();
    for (path, value) in &dump.entries {
        restored.write(path, value.clone());
    }

    // The restored store reproduces the dump, the roots, and presence exactly.
    assert_eq!(restored.scan(&[], usize::MAX), dump);
    assert_eq!(restored.roots(), store.roots());
    assert_eq!(restored.presence(&book(1)), Ok(Presence::ValueAndChildren));
}

#[test]
fn scan_is_bounded_by_the_limit() {
    let mut store = MemStore::new();
    for n in 1..=5 {
        store.write(&book_field(n, "title"), b"x".to_vec());
    }
    // A limit below the total truncates.
    let page = store.scan(&[], 3);
    assert_eq!(page.entries.len(), 3);
    assert!(page.truncated);
    // A limit at or above the total does not.
    let page = store.scan(&[], 5);
    assert_eq!(page.entries.len(), 5);
    assert!(!page.truncated);
}

#[test]
fn a_corrupt_stored_path_is_a_typed_error() {
    use marrow_store::backend::StoreError;

    // A key that is not a valid segment sequence: 0xFF is not a kind tag.
    let mut store = MemStore::new();
    store.write(&[0xFF], b"x".to_vec());
    assert!(matches!(store.roots(), Err(StoreError::CorruptPath { .. })));

    // A valid root with a malformed child segment below it.
    let mut store = MemStore::new();
    let root = encode_path(&[PathSegment::Root("x".into())]);
    let mut corrupt = root.clone();
    corrupt.push(0xFF);
    store.write(&corrupt, b"x".to_vec());
    assert!(matches!(
        store.child_keys(&root),
        Err(StoreError::CorruptPath { .. })
    ));
}

#[test]
fn store_errors_expose_stable_codes_and_messages() {
    use std::path::PathBuf;

    use marrow_store::backend::StoreError;

    let cases = [
        (
            StoreError::CorruptPath { path: vec![0xFF] },
            "store.corrupt_path",
        ),
        (
            StoreError::Io {
                op: "read",
                message: "disk gone".into(),
            },
            "store.io",
        ),
        (
            StoreError::Locked {
                data_dir: PathBuf::from("/data/marrow.redb"),
            },
            "store.locked",
        ),
        (
            StoreError::FormatVersion {
                found: 2,
                supported: 1,
            },
            "store.format_version",
        ),
        (
            StoreError::Corruption {
                message: "torn page".into(),
            },
            "store.corruption",
        ),
        (
            StoreError::LimitExceeded {
                limit: "key length",
            },
            "store.limit",
        ),
    ];
    for (error, code) in cases {
        assert_eq!(error.code(), code, "code for {error:?}");
        // Every variant renders a non-empty human message.
        assert!(!error.to_string().is_empty(), "message for {error:?}");
    }

    // A lock error names the store file it could not open, not "the data
    // directory" (the path is a `.redb` file).
    let locked = StoreError::Locked {
        data_dir: PathBuf::from("/data/marrow.redb"),
    };
    let message = locked.to_string();
    assert!(message.contains("already open"), "{message}");
    assert!(message.contains("marrow.redb"), "{message}");
}
