//! The engine-resident catalog table lives in its own physical family, written
//! inside the caller's transaction and invisible to every data, index, and meta
//! access. These tests drive the public `TreeStore` catalog API and prove the
//! family boundary holds both ways: data/index/backup access never sees a catalog
//! row, and the catalog read never sees a data/index/meta cell.

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, DataPathSegment, TreeStore};

mod common;
use common::catalog_id;

fn stable_id(suffix: u8) -> String {
    format!("cat_{suffix:032x}")
}

/// A snapshot exercising aliases, a store key shape, and a member structural
/// signature, so every optional field has a populated value to round-trip.
fn sample_snapshot() -> CatalogMetadata {
    CatalogMetadata::new(
        7,
        vec![
            CatalogEntry {
                kind: CatalogEntryKind::Store,
                path: "books".to_string(),
                stable_id: stable_id(1),
                aliases: vec!["library".to_string()],
                lifecycle: CatalogLifecycle::Active,
                accepted_key_shape: Some("int".to_string()),
                accepted_struct: None,
            },
            CatalogEntry {
                kind: CatalogEntryKind::ResourceMember,
                path: "books.title".to_string(),
                stable_id: stable_id(2),
                aliases: Vec::new(),
                lifecycle: CatalogLifecycle::Active,
                accepted_key_shape: None,
                accepted_struct: Some("leaf:string".to_string()),
            },
        ],
    )
}

fn sample_commit_metadata() -> CommitMetadata {
    CommitMetadata {
        commit_id: 0,
        catalog_epoch: 9,
        layout_epoch: 0,
        source_digest: "sha256:0000000000000000000000000000000000000000000000000000000000000009"
            .to_string(),
        engine_profile_digest: [0; 8],
        changed_root_catalog_ids: Vec::new(),
        changed_index_catalog_ids: Vec::new(),
    }
}

#[test]
fn memory_store_round_trips_a_catalog_snapshot() {
    let snapshot = sample_snapshot();
    let store = TreeStore::memory();
    store
        .replace_catalog_snapshot(&snapshot)
        .expect("publish catalog");

    let read = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("catalog present");
    assert_eq!(read.epoch, snapshot.epoch);
    assert_eq!(read.digest, snapshot.digest);
    assert_eq!(read.entries, snapshot.entries);
    // The populated optional fields survived the round-trip.
    assert_eq!(read.entries[0].aliases, vec!["library".to_string()]);
    assert_eq!(read.entries[0].accepted_key_shape.as_deref(), Some("int"));
    assert_eq!(
        read.entries[1].accepted_struct.as_deref(),
        Some("leaf:string")
    );
    assert_eq!(
        store.catalog_snapshot_digest().expect("digest"),
        Some(snapshot.digest)
    );
}

#[cfg(feature = "native")]
#[test]
fn redb_persists_catalog_rows_across_close_and_reopen() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("catalog.redb");
    let snapshot = sample_snapshot();
    {
        let store = TreeStore::open(&path).expect("open");
        store
            .replace_catalog_snapshot(&snapshot)
            .expect("publish catalog");
    }

    let store = TreeStore::open(&path).expect("reopen");
    let read = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("catalog survives reopen");
    assert_eq!(read, snapshot);
}

/// The catalog family is disjoint from data, index, and meta. After publishing a
/// catalog and writing user data, an index entry, and a meta stamp, no data-family
/// scan, index-family scan, or backup traversal observes a catalog row, and the
/// catalog read observes none of those cells.
#[test]
fn the_catalog_family_is_invisible_to_data_index_and_meta_access() {
    let store_id = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let index = catalog_id("3333333333333333");
    let path = [DataPathSegment::Member(title)];
    let store = TreeStore::memory();

    store
        .replace_catalog_snapshot(&sample_snapshot())
        .expect("publish catalog");
    store
        .write_data_value(&store_id, &[SavedKey::Int(1)], &path, b"Mort".to_vec())
        .expect("write data");
    store
        .write_index_entry(
            &index,
            &[SavedKey::Str("Mort".into())],
            &[SavedKey::Int(1)],
            Vec::new(),
        )
        .expect("write index");
    store
        .write_commit_metadata(&sample_commit_metadata())
        .expect("stamp commit metadata");

    // A backup traversal carries only data-family cells: every cell it yields is a
    // data cell under the data store, never a catalog row.
    let mut backup_count = 0;
    store
        .visit_backup_cells(|cell| {
            backup_count += 1;
            assert_eq!(cell.data_key().store.as_str(), store_id.as_str());
            Ok(())
        })
        .expect("backup traversal");
    assert!(backup_count > 0, "the data write produced backup cells");

    // Data and index navigation see their own children, not catalog rows. A data
    // child scan finds the seeded identity; deleting the whole data and index
    // subtrees leaves the catalog intact.
    assert!(
        store
            .data_subtree_exists(&store_id, &[SavedKey::Int(1)], &path)
            .expect("data exists")
    );
    assert_eq!(
        store
            .index_first_child(&index, &[])
            .expect("index first child"),
        Some(SavedKey::Str("Mort".into()))
    );

    // The catalog read is unaffected by the data, index, and commit metadata cells.
    let read = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("catalog present");
    assert_eq!(read, sample_snapshot());

    // Clearing all data and index cells does not touch the catalog.
    store
        .delete_record_subtree(&store_id, &[])
        .expect("delete data");
    store
        .delete_index_subtree(&index, &[])
        .expect("delete index");
    assert!(store.is_empty().expect("data and index are empty"));
    assert_eq!(
        store.read_catalog_snapshot().expect("read catalog"),
        Some(sample_snapshot()),
        "the catalog survives a full data and index wipe"
    );
}

/// Normal data APIs cannot reach a catalog row: a data write does not change the
/// catalog digest, and a published catalog is not visible through any data read,
/// scan, or delete. Data keys carry the data family, never the catalog family, so
/// the two never address the same cell.
#[test]
fn data_apis_cannot_read_write_or_delete_a_catalog_row() {
    let store_id = catalog_id("1111111111111111");
    let member = catalog_id("2222222222222222");
    let path = [DataPathSegment::Member(member)];
    let store = TreeStore::memory();

    store
        .replace_catalog_snapshot(&sample_snapshot())
        .expect("publish catalog");
    let digest_before = store.catalog_snapshot_digest().expect("digest before");
    assert!(digest_before.is_some());

    // A data write under crafted ids does not disturb the catalog digest.
    store
        .write_data_value(&store_id, &[SavedKey::Int(1)], &path, b"v".to_vec())
        .expect("write data");
    assert_eq!(
        store.catalog_snapshot_digest().expect("digest after write"),
        digest_before,
        "a data write must not change the catalog digest"
    );

    // The catalog rows are not addressable as data: a read of the same logical place
    // returns only the data value, and the catalog read still holds.
    assert_eq!(
        store
            .read_data_value(&store_id, &[SavedKey::Int(1)], &path)
            .expect("read data"),
        Some(b"v".to_vec())
    );

    // Deleting the data subtree leaves the catalog untouched.
    store
        .delete_data_subtree(&store_id, &[SavedKey::Int(1)], &path)
        .expect("delete data");
    assert_eq!(
        store
            .catalog_snapshot_digest()
            .expect("digest after delete"),
        digest_before,
        "deleting data must not change the catalog digest"
    );
    assert_eq!(
        store.read_catalog_snapshot().expect("read catalog"),
        Some(sample_snapshot())
    );
}

/// A catalog snapshot whose entries cover composite store key shapes still
/// reconstructs exactly, so the digest the checker will compare against is stable
/// across a store reopen.
#[cfg(feature = "native")]
#[test]
fn redb_round_trips_a_catalog_digest_used_for_comparison() {
    let dir = common::TempDir::new("marrow-store-test").expect("temp dir");
    let path = dir.path().join("digest.redb");
    let snapshot = CatalogMetadata::new(
        3,
        vec![CatalogEntry {
            kind: CatalogEntryKind::Store,
            path: "orders".to_string(),
            stable_id: stable_id(5),
            aliases: vec!["purchases".to_string()],
            lifecycle: CatalogLifecycle::Reserved,
            accepted_key_shape: Some("int,string".to_string()),
            accepted_struct: None,
        }],
    );
    let digest = snapshot.digest.clone();
    {
        let store = TreeStore::open(&path).expect("open");
        store
            .replace_catalog_snapshot(&snapshot)
            .expect("publish catalog");
    }
    let store = TreeStore::open(&path).expect("reopen");
    assert_eq!(
        store.catalog_snapshot_digest().expect("digest"),
        Some(digest)
    );
}
