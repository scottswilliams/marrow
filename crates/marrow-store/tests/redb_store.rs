#![cfg(feature = "native")]

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;

fn catalog_id(hex: &str) -> CatalogId {
    CatalogId::new(format!("cat_{hex:0>32}")).unwrap()
}

#[test]
fn native_tree_cells_survive_reopen() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("store.redb");
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    {
        let store = TreeStore::open(&path).expect("open");
        store
            .write_leaf(&books, &[SavedKey::Int(1)], &title, b"Dune".to_vec())
            .expect("write");
    }

    let store = TreeStore::open(&path).expect("reopen");
    assert_eq!(
        store
            .read_leaf(&books, &[SavedKey::Int(1)], &title)
            .expect("read"),
        Some(b"Dune".to_vec())
    );
}

#[test]
fn native_tree_store_rejects_a_second_writer() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("store.redb");
    let _first = TreeStore::open(&path).expect("open the first writer");

    match TreeStore::open(&path) {
        Err(StoreError::Locked { data_dir }) => assert_eq!(data_dir, path),
        Ok(_) => panic!("expected store.locked, got Ok"),
        Err(error) => panic!("expected store.locked, got {error:?}"),
    }
}

#[test]
fn native_read_only_open_requires_an_existing_store() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("missing.redb");

    assert!(TreeStore::open_read_only(&path).is_err());
}

#[test]
fn native_read_only_can_read_existing_cells() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("store.redb");
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    {
        let store = TreeStore::open(&path).expect("create");
        store
            .write_leaf(&books, &[SavedKey::Int(1)], &title, b"Dune".to_vec())
            .expect("write");
    }

    let store = TreeStore::open_read_only(&path).expect("open read-only");
    assert_eq!(
        store
            .read_leaf(&books, &[SavedKey::Int(1)], &title)
            .expect("read"),
        Some(b"Dune".to_vec())
    );
}

#[test]
fn multiple_native_read_only_handles_can_coexist() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("store.redb");
    {
        TreeStore::open(&path).expect("create");
    }

    let _first = TreeStore::open_read_only(&path).expect("open first read-only");
    let _second = TreeStore::open_read_only(&path).expect("open second read-only");
}

#[test]
fn native_writer_is_locked_out_while_read_only_handle_lives() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("store.redb");
    {
        TreeStore::open(&path).expect("create");
    }

    let reader = TreeStore::open_read_only(&path).expect("open read-only");
    match TreeStore::open(&path) {
        Err(StoreError::Locked { data_dir }) => assert_eq!(data_dir, path),
        Ok(_) => panic!("expected store.locked while read-only handle is open, got Ok"),
        Err(error) => panic!("expected store.locked while read-only handle is open, got {error:?}"),
    }
    drop(reader);

    TreeStore::open(&path).expect("reopen read-write after dropping reader");
}

#[test]
fn native_read_only_rejects_write_capability_operations() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("store.redb");
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    {
        let store = TreeStore::open(&path).expect("create");
        store
            .write_leaf(&books, &[SavedKey::Int(1)], &title, b"Dune".to_vec())
            .expect("write");
    }

    let store = TreeStore::open_read_only(&path).expect("open read-only");
    assert!(matches!(
        store.write_leaf(&books, &[SavedKey::Int(2)], &title, b"Other".to_vec()),
        Err(StoreError::ReadOnly { op: "write" })
    ));
    assert!(matches!(
        store.delete_leaf(&books, &[SavedKey::Int(1)], &title),
        Err(StoreError::ReadOnly { op: "delete" })
    ));
    assert!(matches!(
        store.begin(),
        Err(StoreError::ReadOnly { op: "begin" })
    ));
    assert_eq!(
        store
            .read_leaf(&books, &[SavedKey::Int(1)], &title)
            .expect("read existing value"),
        Some(b"Dune".to_vec())
    );
}
