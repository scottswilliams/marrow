//! The engine-resident catalog table lives in its own physical family, written
//! inside the caller's transaction and invisible to every data, index, and meta
//! access. These tests drive the public `TreeStore` catalog API and prove the
//! family boundary holds both ways: data/index/backup access never sees a catalog
//! row, and the catalog read never sees a data/index/meta cell.

use crate::common;
use common::catalog_id;
use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, DataPathSegment, TreeStore};

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
                accepted_index_shape: None,
                accepted_struct: None,
            },
            CatalogEntry {
                kind: CatalogEntryKind::ResourceMember,
                path: "books.title".to_string(),
                stable_id: stable_id(2),
                aliases: Vec::new(),
                lifecycle: CatalogLifecycle::Active,
                accepted_key_shape: None,
                accepted_index_shape: None,
                accepted_struct: Some("leaf:string".to_string()),
            },
        ],
    )
    .expect("catalog builds")
}

/// A single-store snapshot whose only store entry carries the given accepted key
/// shape, so a sibling pair differs only in `accepted_key_shape`.
fn store_snapshot_with_key_shape(key_shape: &str) -> CatalogMetadata {
    CatalogMetadata::new(
        7,
        vec![CatalogEntry {
            kind: CatalogEntryKind::Store,
            path: "books".to_string(),
            stable_id: stable_id(1),
            aliases: Vec::new(),
            lifecycle: CatalogLifecycle::Active,
            accepted_key_shape: Some(key_shape.to_string()),
            accepted_index_shape: None,
            accepted_struct: None,
        }],
    )
    .expect("catalog builds")
}

/// A single-member snapshot whose only member entry carries the given accepted
/// structural signature, so a sibling pair differs only in `accepted_struct`.
fn member_snapshot_with_struct(struct_signature: &str) -> CatalogMetadata {
    CatalogMetadata::new(
        7,
        vec![CatalogEntry {
            kind: CatalogEntryKind::ResourceMember,
            path: "books.title".to_string(),
            stable_id: stable_id(2),
            aliases: Vec::new(),
            lifecycle: CatalogLifecycle::Active,
            accepted_key_shape: None,
            accepted_index_shape: None,
            accepted_struct: Some(struct_signature.to_string()),
        }],
    )
    .expect("catalog builds")
}

/// A single store-index snapshot whose only index entry carries the given accepted
/// index shape, so a sibling pair differs only in `accepted_index_shape` — here the
/// uniqueness flag, the dimension the evolution discharge keys a derived rebuild on.
fn index_snapshot_with_index_shape(index_shape: &str) -> CatalogMetadata {
    CatalogMetadata::new(
        7,
        vec![CatalogEntry {
            kind: CatalogEntryKind::StoreIndex,
            path: "books.byTitle".to_string(),
            stable_id: stable_id(3),
            aliases: Vec::new(),
            lifecycle: CatalogLifecycle::Active,
            accepted_key_shape: None,
            accepted_index_shape: Some(index_shape.to_string()),
            accepted_struct: None,
        }],
    )
    .expect("catalog builds")
}

/// Publish two snapshots that differ only in one accepted-shape field and return the
/// store digest each one yields back. A digest read recomputes the canonical digest
/// from the stored entry rows, so equal digests would prove the differing shape never
/// reached durable rows — the fingerprint-only collapse this lane forbids.
fn republished_digests(left: &CatalogMetadata, right: &CatalogMetadata) -> (String, String) {
    let store = TreeStore::memory();
    store
        .replace_catalog_snapshot(left)
        .expect("publish left snapshot");
    let left_digest = store
        .catalog_snapshot_digest()
        .expect("left digest")
        .expect("left catalog present");
    store
        .replace_catalog_snapshot(right)
        .expect("publish right snapshot");
    let right_digest = store
        .catalog_snapshot_digest()
        .expect("right digest")
        .expect("right catalog present");
    (left_digest, right_digest)
}

/// The store-resident catalog rows are the full accepted shape, not an identity
/// fingerprint. Two snapshots that differ only in one accepted-shape field — key
/// arity, leaf type, or index uniqueness — must round-trip back with the differing
/// field intact and must yield different store digests. A fingerprint-only store
/// would collapse these into one indistinguishable row beyond a single snapshot, so
/// these assertions bite the moment the shared `CatalogMetadata` shrinks across the
/// store seam.
#[test]
fn store_catalog_rows_discriminate_full_accepted_shape_not_a_fingerprint() {
    // Read-back fidelity: every accepted_* field survives publish/read exactly.
    let snapshot = sample_snapshot();
    let store = TreeStore::memory();
    store
        .replace_catalog_snapshot(&snapshot)
        .expect("publish catalog");
    let read = store
        .read_catalog_snapshot()
        .expect("read catalog")
        .expect("catalog present");
    for (read_entry, written_entry) in read.entries.iter().zip(snapshot.entries.iter()) {
        assert_eq!(
            read_entry.accepted_key_shape,
            written_entry.accepted_key_shape
        );
        assert_eq!(read_entry.accepted_struct, written_entry.accepted_struct);
        assert_eq!(
            read_entry.accepted_index_shape,
            written_entry.accepted_index_shape
        );
    }

    // Key-arity divergence: one int key vs. a composite int,string key.
    let (one_key, two_keys) = republished_digests(
        &store_snapshot_with_key_shape("int"),
        &store_snapshot_with_key_shape("int,string"),
    );
    assert_ne!(
        one_key, two_keys,
        "a store digest must distinguish key arity, not collapse it to a fingerprint"
    );

    // Leaf-type divergence on a member: leaf:int vs. leaf:string.
    let (leaf_int, leaf_string) = republished_digests(
        &member_snapshot_with_struct("leaf:int"),
        &member_snapshot_with_struct("leaf:string"),
    );
    assert_ne!(
        leaf_int, leaf_string,
        "a store digest must distinguish a member leaf type, not collapse it"
    );

    // Index-uniqueness divergence: the same key column, unique vs. non-unique.
    let (non_unique, unique) = republished_digests(
        &index_snapshot_with_index_shape(
            "unique=false;keys=[member:cat_00000000000000000000000000000002:string]",
        ),
        &index_snapshot_with_index_shape(
            "unique=true;keys=[member:cat_00000000000000000000000000000002:string]",
        ),
    );
    assert_ne!(
        non_unique, unique,
        "a store digest must distinguish index uniqueness, not collapse it"
    );
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
fn memory_store_round_trips_and_discriminates_a_catalog_snapshot() {
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

    // Equality alone a fingerprint-only store could satisfy for one snapshot, so the
    // round-trip is strengthened to a discrimination oracle: a sibling differing only
    // in one member leaf type publishes a different store digest.
    let (leaf_int, leaf_string) = republished_digests(
        &member_snapshot_with_struct("leaf:int"),
        &member_snapshot_with_struct("leaf:string"),
    );
    assert_ne!(
        leaf_int, leaf_string,
        "the store digest reflects member shape, not just identity"
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

/// A catalog snapshot whose entries cover composite store key shapes reconstructs
/// exactly across a reopen, and the persisted digest discriminates store key arity:
/// a sibling differing only in its accepted key shape persists a different digest, so
/// a reopened native store cannot have collapsed the row to an identity fingerprint.
#[cfg(feature = "native")]
#[test]
fn redb_persists_a_shape_discriminating_catalog_digest() {
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
            accepted_index_shape: None,
            accepted_struct: None,
        }],
    )
    .expect("catalog builds");
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
        Some(digest.clone())
    );

    // The same entry re-keyed to a single int identity persists a different digest, so
    // the reopened row carried the full key shape rather than a collapsed fingerprint.
    let rekeyed = CatalogMetadata::new(
        3,
        vec![CatalogEntry {
            kind: CatalogEntryKind::Store,
            path: "orders".to_string(),
            stable_id: stable_id(5),
            aliases: vec!["purchases".to_string()],
            lifecycle: CatalogLifecycle::Reserved,
            accepted_key_shape: Some("int".to_string()),
            accepted_index_shape: None,
            accepted_struct: None,
        }],
    )
    .expect("catalog builds");
    store
        .replace_catalog_snapshot(&rekeyed)
        .expect("publish rekeyed");
    drop(store);
    let store = TreeStore::open(&path).expect("reopen rekeyed");
    let rekeyed_digest = store
        .catalog_snapshot_digest()
        .expect("rekeyed digest")
        .expect("rekeyed catalog present");
    assert_ne!(
        rekeyed_digest, digest,
        "a native store digest must distinguish store key arity across a reopen"
    );
}
