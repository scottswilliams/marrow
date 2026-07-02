#![cfg(feature = "native")]

use crate::common;
use common::catalog_id;
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment;
use marrow_store::{AccessMode, SealedStore};

fn title_path(title: &CatalogId) -> [DataPathSegment; 1] {
    [DataPathSegment::Member(title.clone())]
}

#[test]
fn native_tree_cells_survive_reopen() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("store.redb");
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    {
        let store = SealedStore::open(&path, AccessMode::Create)
            .expect("open")
            .into_store();
        store
            .write_data_value(
                &books,
                &[SavedKey::Int(1)],
                &title_path(&title),
                b"Dune".to_vec(),
            )
            .expect("write");
    }

    let store = SealedStore::open(&path, AccessMode::Create)
        .expect("reopen")
        .into_store();
    assert_eq!(
        store
            .read_data_value(&books, &[SavedKey::Int(1)], &title_path(&title))
            .expect("read"),
        Some(b"Dune".to_vec())
    );
}

#[test]
fn native_tree_store_rejects_a_second_writer() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("store.redb");
    let _first = SealedStore::open(&path, AccessMode::Create)
        .expect("open the first writer")
        .into_store();

    match SealedStore::open(&path, AccessMode::Create).map(SealedStore::into_store) {
        Err(StoreError::Locked { data_dir }) => assert_eq!(data_dir, path),
        Ok(_) => panic!("expected store.locked, got Ok"),
        Err(error) => panic!("expected store.locked, got {error:?}"),
    }
}

#[test]
fn native_read_only_open_requires_an_existing_store() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("missing.redb");

    assert!(SealedStore::open(&path, AccessMode::Read).is_err());
}

#[test]
fn native_read_only_can_read_existing_cells() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("store.redb");
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    {
        let store = SealedStore::open(&path, AccessMode::Create)
            .expect("create")
            .into_store();
        store
            .write_data_value(
                &books,
                &[SavedKey::Int(1)],
                &title_path(&title),
                b"Dune".to_vec(),
            )
            .expect("write");
    }

    let store = SealedStore::open(&path, AccessMode::Read)
        .expect("open read-only")
        .into_store();
    assert_eq!(
        store
            .read_data_value(&books, &[SavedKey::Int(1)], &title_path(&title))
            .expect("read"),
        Some(b"Dune".to_vec())
    );
}

#[test]
fn multiple_native_read_only_handles_can_coexist() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("store.redb");
    {
        SealedStore::open(&path, AccessMode::Create).expect("create");
    }

    let _first = SealedStore::open(&path, AccessMode::Read)
        .expect("open first read-only")
        .into_store();
    let _second = SealedStore::open(&path, AccessMode::Read)
        .expect("open second read-only")
        .into_store();
}

#[test]
fn native_writer_is_locked_out_while_read_only_handle_lives() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("store.redb");
    {
        SealedStore::open(&path, AccessMode::Create).expect("create");
    }

    let reader = SealedStore::open(&path, AccessMode::Read)
        .expect("open read-only")
        .into_store();
    match SealedStore::open(&path, AccessMode::Create).map(SealedStore::into_store) {
        Err(StoreError::Locked { data_dir }) => assert_eq!(data_dir, path),
        Ok(_) => panic!("expected store.locked while read-only handle is open, got Ok"),
        Err(error) => panic!("expected store.locked while read-only handle is open, got {error:?}"),
    }
    drop(reader);

    SealedStore::open(&path, AccessMode::Create).expect("reopen read-write after dropping reader");
}

#[test]
fn native_read_only_is_locked_out_while_writer_handle_lives() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("store.redb");
    let writer = SealedStore::open(&path, AccessMode::Create)
        .expect("open writer")
        .into_store();

    match SealedStore::open(&path, AccessMode::Read).map(SealedStore::into_store) {
        Err(StoreError::Locked { data_dir }) => assert_eq!(data_dir, path),
        Ok(_) => panic!("expected store.locked while writer handle is open, got Ok"),
        Err(error) => panic!("expected store.locked while writer handle is open, got {error:?}"),
    }
    drop(writer);

    SealedStore::open(&path, AccessMode::Read).expect("reopen read-only after dropping writer");
}

#[test]
fn native_same_handle_snapshot_write_conflicts_are_transaction_errors() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("store.redb");
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let store = SealedStore::open(&path, AccessMode::Create)
        .expect("open writer")
        .into_store();
    store
        .write_data_value(
            &books,
            &[SavedKey::Int(1)],
            &title_path(&title),
            b"Dune".to_vec(),
        )
        .expect("write");

    let snapshot = store.read_snapshot().expect("pin read snapshot");
    let write = store
        .write_data_value(
            &books,
            &[SavedKey::Int(2)],
            &title_path(&title),
            b"Other".to_vec(),
        )
        .expect_err("same-handle write must reject a pinned snapshot");
    assert_eq!(write.code(), "store.transaction");
    let begin = store
        .begin()
        .expect_err("same-handle transaction must reject a pinned snapshot");
    assert_eq!(begin.code(), "store.transaction");
    drop(snapshot);

    store
        .write_data_value(
            &books,
            &[SavedKey::Int(2)],
            &title_path(&title),
            b"Other".to_vec(),
        )
        .expect("write after snapshot drops");
}

#[test]
fn native_read_only_rejects_write_capability_operations() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("store.redb");
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    {
        let store = SealedStore::open(&path, AccessMode::Create)
            .expect("create")
            .into_store();
        store
            .write_data_value(
                &books,
                &[SavedKey::Int(1)],
                &title_path(&title),
                b"Dune".to_vec(),
            )
            .expect("write");
    }

    let store = SealedStore::open(&path, AccessMode::Read)
        .expect("open read-only")
        .into_store();
    assert!(matches!(
        store.write_data_value(
            &books,
            &[SavedKey::Int(2)],
            &title_path(&title),
            b"Other".to_vec()
        ),
        Err(StoreError::ReadOnly { op: "write" })
    ));
    assert!(matches!(
        store.delete_data_subtree(&books, &[SavedKey::Int(1)], &title_path(&title)),
        Err(StoreError::ReadOnly { op: "delete" })
    ));
    assert!(matches!(
        store.begin(),
        Err(StoreError::ReadOnly { op: "begin" })
    ));
    assert_eq!(
        store
            .read_data_value(&books, &[SavedKey::Int(1)], &title_path(&title))
            .expect("read existing value"),
        Some(b"Dune".to_vec())
    );
}
