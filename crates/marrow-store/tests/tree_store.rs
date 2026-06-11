use marrow_store::cell::{CatalogId, DataCellKind};
use marrow_store::key::SavedKey;
use marrow_store::tree::{
    CommitMetadata, DataPathSegment, EngineProfile, IndexPage, TreeEnumMember, TreeStore,
    decode_tree_enum_member, encode_tree_enum_member,
};

mod common;
use common::{catalog_id, collect_children};

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// A commit metadata record whose activation receipt block is the empty/zero
/// default, parameterized on the fields the round-trip tests vary.
fn sample_commit_metadata(
    commit_id: u64,
    catalog_epoch: u64,
    layout_epoch: u64,
    source_digest: &str,
    engine_profile_digest: [u8; 8],
    roots: Vec<CatalogId>,
    indexes: Vec<CatalogId>,
) -> CommitMetadata {
    CommitMetadata {
        commit_id,
        catalog_epoch,
        layout_epoch,
        source_digest: source_digest.to_string(),
        engine_profile_digest,
        changed_root_catalog_ids: roots,
        changed_index_catalog_ids: indexes,
        activation_evolution_digest: String::new(),
        activation_proposal_catalog_digest: None,
        activation_proposal_new_catalog_ids: Vec::new(),
        activation_records_backfilled: 0,
        activation_default_records_by_id: Vec::new(),
        activation_indexes_rebuilt: 0,
        activation_records_retired: 0,
        activation_retire_evidence_digest: String::new(),
        activation_records_retired_by_id: Vec::new(),
        activation_records_transformed: 0,
    }
}

fn index_rows(page: IndexPage) -> Vec<(Vec<SavedKey>, Vec<u8>)> {
    page.entries
        .into_iter()
        .map(|entry| (entry.identity, entry.value))
        .collect()
}

fn data_key(key: SavedKey) -> DataPathSegment {
    DataPathSegment::Key(key)
}

fn data_children(
    store: &TreeStore,
    root: &CatalogId,
    identity: &[SavedKey],
    path: &[DataPathSegment],
) -> Vec<SavedKey> {
    collect_children(
        || store.data_first_child(root, identity, path),
        |child| store.data_next_child(root, identity, path, child),
    )
}

fn data_children_rev(
    store: &TreeStore,
    root: &CatalogId,
    identity: &[SavedKey],
    path: &[DataPathSegment],
) -> Vec<SavedKey> {
    collect_children(
        || store.data_last_child(root, identity, path),
        |child| store.data_prev_child(root, identity, path, child),
    )
}

fn record_children(store: &TreeStore, root: &CatalogId, prefix: &[SavedKey]) -> Vec<SavedKey> {
    collect_children(
        || store.record_first_child(root, prefix),
        |child| store.record_next_child(root, prefix, child),
    )
}

fn index_children(store: &TreeStore, index: &CatalogId, prefix: &[SavedKey]) -> Vec<SavedKey> {
    collect_children(
        || store.index_first_child(index, prefix),
        |child| store.index_next_child(index, prefix, child),
    )
}

#[test]
fn enum_member_values_store_catalog_ids_not_source_order() {
    let status = catalog_id("aaaaaaaaaaaaaaaa");
    let active = catalog_id("bbbbbbbbbbbbbbbb");
    let archived = catalog_id("cccccccccccccccc");

    let value = TreeEnumMember::new(status.clone(), active.clone());
    let encoded = encode_tree_enum_member(&value).expect("encode enum member");

    assert_eq!(
        decode_tree_enum_member(&encoded).expect("decode enum member"),
        value
    );
    assert_eq!(
        encoded,
        encode_tree_enum_member(&TreeEnumMember::new(status.clone(), active))
            .expect("source reorder leaves catalog-backed bytes alone")
    );
    assert_ne!(
        encoded,
        encode_tree_enum_member(&TreeEnumMember::new(status, archived))
            .expect("different member identity changes bytes")
    );
    for spelling in ["Status", "active", "archived", "enabled"] {
        assert!(
            !contains_subslice(&encoded, spelling.as_bytes()),
            "enum bytes contain source spelling {spelling:?}: {encoded:?}"
        );
    }
}

#[test]
fn profile_and_metadata_cells_round_trip_in_memory() {
    let profile = EngineProfile::new(9);
    assert_eq!(profile.layout_epoch(), 9);
    assert_eq!(profile.key_profile_version(), 0);
    assert_eq!(profile.digest_bytes().len(), 8);
    assert_eq!(profile.digest_hex(), EngineProfile::new(9).digest_hex());
    assert_ne!(profile.digest_hex(), EngineProfile::new(10).digest_hex());

    let root = catalog_id("aaaaaaaaaaaaaaaa");
    let index = catalog_id("bbbbbbbbbbbbbbbb");
    let store = TreeStore::memory();
    store.write_catalog_epoch(44).expect("write catalog epoch");
    store
        .write_engine_profile(&profile)
        .expect("write engine profile");
    store
        .write_commit_metadata(&sample_commit_metadata(
            55,
            44,
            profile.layout_epoch(),
            "sha256:0000000000000000000000000000000000000000000000000000000000000044",
            profile.digest_bytes(),
            vec![root.clone()],
            vec![index.clone()],
        ))
        .expect("write commit");

    assert_eq!(
        store.read_catalog_epoch().expect("read catalog epoch"),
        Some(44)
    );
    assert_eq!(
        store.read_layout_epoch().expect("read layout epoch"),
        Some(profile.layout_epoch())
    );
    assert_eq!(
        store
            .read_engine_profile_digest()
            .expect("read engine profile digest"),
        Some(profile.digest_bytes())
    );
    assert_eq!(
        store.read_commit_metadata().expect("read commit"),
        Some(sample_commit_metadata(
            55,
            44,
            profile.layout_epoch(),
            "sha256:0000000000000000000000000000000000000000000000000000000000000044",
            profile.digest_bytes(),
            vec![root],
            vec![index],
        ))
    );
}

#[test]
fn exact_index_tuple_scan_pages_by_identity() {
    let by_shelf = catalog_id("4444444444444444");
    let identity = [SavedKey::Int(7)];
    let store = TreeStore::memory();

    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &[SavedKey::Int(8)],
            b"other".to_vec(),
        )
        .expect("write other index");
    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &identity,
            b"present".to_vec(),
        )
        .expect("write index");
    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("fiction".into()), SavedKey::Bool(false)],
            &identity,
            b"longer".to_vec(),
        )
        .expect("write longer index");

    let first_page = store
        .scan_index_tuple(&by_shelf, &[SavedKey::Str("fiction".into())], 1)
        .expect("scan index tuple");
    assert_eq!(
        index_rows(first_page.clone()),
        vec![(vec![SavedKey::Int(7)], b"present".to_vec())]
    );
    assert!(first_page.truncated);
    let cursor = first_page.cursor.as_ref().expect("cursor for next page");

    let second_page = store
        .scan_index_tuple_after(&by_shelf, &[SavedKey::Str("fiction".into())], cursor, 10)
        .expect("scan next index tuple page");
    assert!(!second_page.truncated);
    assert_eq!(
        index_rows(second_page),
        vec![(vec![SavedKey::Int(8)], b"other".to_vec())]
    );
}

#[test]
fn exact_index_tuple_cursor_rejects_another_tuple() {
    let by_shelf = catalog_id("4444444444444444");
    let identity = [SavedKey::Int(7)];
    let store = TreeStore::memory();

    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &identity,
            b"present".to_vec(),
        )
        .expect("write index");
    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &[SavedKey::Int(8)],
            b"other".to_vec(),
        )
        .expect("write other index");
    let page = store
        .scan_index_tuple(&by_shelf, &[SavedKey::Str("fiction".into())], 1)
        .expect("scan index tuple");
    assert!(page.truncated);
    let cursor = page.cursor.as_ref().expect("cursor");

    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("nonfiction".into())],
            &identity,
            b"wrong tuple".to_vec(),
        )
        .expect("write another tuple");
    let error = store
        .scan_index_tuple_after(&by_shelf, &[SavedKey::Str("nonfiction".into())], cursor, 10)
        .expect_err("cursor from another exact tuple is invalid");
    assert_eq!(error.code(), "store.cursor");
}

#[test]
fn exact_index_tuple_zero_limit_returns_empty_page() {
    let by_shelf = catalog_id("4444444444444444");
    let store = TreeStore::memory();

    let empty_page = store
        .scan_index_tuple(&by_shelf, &[SavedKey::Str("fiction".into())], 0)
        .expect("zero-limit scan");
    assert!(empty_page.entries.is_empty());
    assert!(empty_page.cursor.is_none());
    assert!(!empty_page.truncated);
}

#[test]
fn nested_data_paths_use_member_catalog_ids_and_typed_keys() {
    let books = catalog_id("1111111111111111");
    let versions = catalog_id("2222222222222222");
    let comments = catalog_id("3333333333333333");
    let text = catalog_id("4444444444444444");
    let identity = [SavedKey::Int(7)];
    let path = [
        DataPathSegment::Member(versions.clone()),
        data_key(SavedKey::Int(2)),
        DataPathSegment::Member(comments.clone()),
        data_key(SavedKey::Str("a".into())),
        DataPathSegment::Member(text.clone()),
    ];
    let store = TreeStore::memory();

    store
        .write_data_value(&books, &identity, &path, b"hello".to_vec())
        .expect("write nested value");

    assert_eq!(
        store
            .read_data_value(&books, &identity, &path)
            .expect("read nested value"),
        Some(b"hello".to_vec())
    );
}

#[test]
fn data_child_navigation_walks_typed_layer_keys() {
    let books = catalog_id("1111111111111111");
    let tags = catalog_id("2222222222222222");
    let identity = [SavedKey::Int(7)];
    let store = TreeStore::memory();

    for key in [SavedKey::Int(1), SavedKey::Int(3), SavedKey::Int(2)] {
        store
            .write_data_value(
                &books,
                &identity,
                &[DataPathSegment::Member(tags.clone()), data_key(key)],
                b"tag".to_vec(),
            )
            .expect("write keyed layer value");
    }

    assert_eq!(
        data_children(
            &store,
            &books,
            &identity,
            &[DataPathSegment::Member(tags.clone())]
        ),
        vec![SavedKey::Int(1), SavedKey::Int(2), SavedKey::Int(3)]
    );
    assert_eq!(
        data_children_rev(
            &store,
            &books,
            &identity,
            &[DataPathSegment::Member(tags.clone())]
        ),
        vec![SavedKey::Int(3), SavedKey::Int(2), SavedKey::Int(1)]
    );
    assert_eq!(
        store
            .data_next_child(
                &books,
                &identity,
                &[DataPathSegment::Member(tags.clone())],
                &SavedKey::Int(1),
            )
            .expect("next child"),
        Some(SavedKey::Int(2))
    );
    assert_eq!(
        store
            .data_prev_child(
                &books,
                &identity,
                &[DataPathSegment::Member(tags)],
                &SavedKey::Int(3),
            )
            .expect("previous child"),
        Some(SavedKey::Int(2))
    );
}

#[test]
fn record_and_index_child_navigation_use_catalog_roots() {
    let books = catalog_id("1111111111111111");
    let by_shelf = catalog_id("2222222222222222");
    let store = TreeStore::memory();

    for id in [2, 1, 3] {
        store
            .write_node(&books, &[SavedKey::Int(id)])
            .expect("write record node");
        store
            .write_index_entry(
                &by_shelf,
                &[SavedKey::Str("fiction".into())],
                &[SavedKey::Int(id)],
                b"present".to_vec(),
            )
            .expect("write index entry");
    }

    assert_eq!(
        record_children(&store, &books, &[]),
        vec![SavedKey::Int(1), SavedKey::Int(2), SavedKey::Int(3)]
    );
    assert_eq!(
        index_children(&store, &by_shelf, &[SavedKey::Str("fiction".into())]),
        vec![SavedKey::Int(1), SavedKey::Int(2), SavedKey::Int(3)]
    );
}

#[test]
fn record_navigation_requires_node_cells() {
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let store = TreeStore::memory();

    store
        .write_data_value(
            &books,
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(title)],
            b"leaf without node".to_vec(),
        )
        .expect("write orphan leaf debris");
    store
        .write_node(&books, &[SavedKey::Int(2)])
        .expect("write record node");

    assert!(
        !store
            .data_subtree_exists(&books, &[SavedKey::Int(1)], &[])
            .expect("record presence"),
        "a leaf without its record node is not record presence"
    );
    assert_eq!(
        store.record_child_count(&books, &[]).expect("record count"),
        1
    );
    assert_eq!(record_children(&store, &books, &[]), vec![SavedKey::Int(2)]);

    let mut visited = Vec::new();
    store
        .for_each_record(&books, 1, &mut |identity| {
            visited.push(identity.to_vec());
            Ok(())
        })
        .expect("visit records");
    assert_eq!(visited, vec![vec![SavedKey::Int(2)]]);
}

#[test]
fn record_navigation_cursors_do_not_require_existing_anchor_nodes() {
    let books = catalog_id("1111111111111111");
    let store = TreeStore::memory();

    store
        .write_node(&books, &[SavedKey::Int(1)])
        .expect("write first record node");
    store
        .write_node(&books, &[SavedKey::Int(3)])
        .expect("write second record node");

    assert_eq!(
        store
            .record_next_child(&books, &[], &SavedKey::Int(2))
            .expect("next child after gap"),
        Some(SavedKey::Int(3))
    );
    assert_eq!(
        store
            .record_prev_child(&books, &[], &SavedKey::Int(2))
            .expect("previous child before gap"),
        Some(SavedKey::Int(1))
    );
}

#[test]
fn descendant_record_node_does_not_fabricate_a_shorter_record_identity() {
    let books = catalog_id("1111111111111111");
    let store = TreeStore::memory();

    store
        .write_node(&books, &[SavedKey::Int(1), SavedKey::Int(2)])
        .expect("write composite record node");

    assert!(
        !store
            .data_subtree_exists(&books, &[SavedKey::Int(1)], &[])
            .expect("short identity presence"),
        "a descendant node is not the shorter identity's record node"
    );
    assert_eq!(
        store
            .record_child_count(&books, &[])
            .expect("single-key record count"),
        0,
        "final-level count requires exact child nodes"
    );

    let mut one_key_records = Vec::new();
    store
        .for_each_record(&books, 1, &mut |identity| {
            one_key_records.push(identity.to_vec());
            Ok(())
        })
        .expect("visit one-key records");
    assert!(one_key_records.is_empty());

    let mut two_key_records = Vec::new();
    store
        .for_each_record(&books, 2, &mut |identity| {
            two_key_records.push(identity.to_vec());
            Ok(())
        })
        .expect("visit two-key records");
    assert_eq!(
        two_key_records,
        vec![vec![SavedKey::Int(1), SavedKey::Int(2)]]
    );
}

#[test]
fn backup_round_trips_sparse_record_nodes() {
    let books = catalog_id("1111111111111111");
    let store = TreeStore::memory();
    store
        .write_node(&books, &[SavedKey::Int(1)])
        .expect("write sparse record node");

    let mut cells = Vec::new();
    let mut backup_bytes = Vec::new();
    store
        .visit_backup_cells(|cell| {
            cell.write_framed(&mut backup_bytes)
                .expect("write backup frame");
            cells.push((cell.data_key().clone(), cell.value().to_vec()));
            Ok(())
        })
        .expect("collect backup");
    assert_eq!(cells.len(), 1);
    assert!(matches!(cells[0].0.kind, DataCellKind::Node));

    let restored = TreeStore::memory();
    restored
        .write_node(&cells[0].0.store, &cells[0].0.identity)
        .expect("restore node");
    let mut restored_cells = Vec::new();
    let mut restored_backup_bytes = Vec::new();
    restored
        .visit_backup_cells(|cell| {
            cell.write_framed(&mut restored_backup_bytes)
                .expect("write restored backup frame");
            restored_cells.push((cell.data_key().clone(), cell.value().to_vec()));
            Ok(())
        })
        .expect("collect restored backup");

    assert_eq!(restored_cells, cells);
    assert_eq!(restored_backup_bytes, backup_bytes);
    assert!(
        restored
            .data_subtree_exists(&books, &[SavedKey::Int(1)], &[])
            .expect("restored presence")
    );
    assert_eq!(
        restored
            .record_child_count(&books, &[])
            .expect("restored count"),
        1
    );
}

#[test]
fn exact_index_tuple_delete_removes_only_the_exact_identity() {
    let by_shelf = catalog_id("4444444444444444");
    let identity = [SavedKey::Int(7)];
    let extended_identity = [SavedKey::Int(7), SavedKey::Bool(false)];
    let store = TreeStore::memory();

    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &identity,
            b"present".to_vec(),
        )
        .expect("write index");
    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &extended_identity,
            b"extended".to_vec(),
        )
        .expect("write prefix-related identity");
    store
        .delete_index_entry(&by_shelf, &[SavedKey::Str("fiction".into())], &identity)
        .expect("delete index");
    let remaining = store
        .scan_index_tuple(&by_shelf, &[SavedKey::Str("fiction".into())], 10)
        .expect("scan after delete");
    assert_eq!(
        index_rows(remaining),
        vec![(extended_identity.to_vec(), b"extended".to_vec())],
        "only the exact identity is removed; the prefix-related identity survives"
    );
}

#[test]
fn visit_backup_cells_streams_data_only_in_encoded_order() {
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let by_title = catalog_id("3333333333333333");
    let store = TreeStore::memory();

    // Two records, written out of order, plus an index entry derived from them.
    for id in [3, 1, 2] {
        store
            .write_data_value(
                &books,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(title.clone())],
                format!("title-{id}").into_bytes(),
            )
            .expect("write data value");
    }
    store
        .write_index_entry(
            &by_title,
            &[SavedKey::Str("title-1".into())],
            &[SavedKey::Int(1)],
            b"present".to_vec(),
        )
        .expect("write index entry");

    let mut cells = Vec::new();
    store
        .visit_backup_cells(|cell| {
            cells.push(cell.data_key().clone());
            Ok(())
        })
        .expect("visit backup cells");

    assert_eq!(cells.len(), 3, "backup excludes the generated index entry");
    assert!(
        cells
            .iter()
            .all(|cell| cell.store.as_str() == books.as_str()),
        "backup stream carries only data cells for the seeded store: {cells:?}"
    );
    assert_eq!(
        cells
            .iter()
            .map(|cell| cell.identity.clone())
            .collect::<Vec<_>>(),
        vec![
            vec![SavedKey::Int(1)],
            vec![SavedKey::Int(2)],
            vec![SavedKey::Int(3)],
        ],
        "backup traversal follows deterministic encoded identity order"
    );
    for cell in &cells {
        assert_eq!(
            cell.kind,
            DataCellKind::Value {
                path: vec![DataPathSegment::Member(title.clone())],
            },
            "backup traversal reports typed value targets, not physical keys"
        );
    }
}

#[test]
fn is_empty_sees_data_and_index_families() {
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let by_title = catalog_id("3333333333333333");
    let store = TreeStore::memory();
    assert!(store.is_empty().expect("fresh store is empty"));

    // A data-only store is not empty: is_empty checks the data family.
    store
        .write_data_value(
            &books,
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(title)],
            b"Mort".to_vec(),
        )
        .expect("write data value");
    assert!(
        !store.is_empty().expect("data-only store is not empty"),
        "is_empty checks the data family so a leftover record still blocks restore"
    );
    store
        .delete_record_subtree(&books, &[])
        .expect("clear data records");
    assert!(
        store
            .is_empty()
            .expect("store is empty after clearing data")
    );

    // An index-only store is not empty even though a backup carries no cells for it.
    store
        .write_index_entry(
            &by_title,
            &[SavedKey::Str("title-1".into())],
            &[SavedKey::Int(1)],
            b"present".to_vec(),
        )
        .expect("write index entry");
    assert!(
        !store.is_empty().expect("index-only store is not empty"),
        "is_empty checks the index family so a leftover entry still blocks restore"
    );

    let mut backup_cells = 0usize;
    store
        .visit_backup_cells(|_cell| {
            backup_cells += 1;
            Ok(())
        })
        .expect("visit backup cells");
    assert_eq!(
        backup_cells, 0,
        "the backup stream is empty even though the store is not"
    );

    store
        .delete_index_subtree(&by_title, &[])
        .expect("clear index");
    assert!(store.is_empty().expect("store is empty again"));
}

#[test]
fn malformed_tree_values_are_store_corruption() {
    let error = decode_tree_enum_member(&[0xff]).expect_err("malformed enum member is corruption");
    assert_eq!(error.code(), "store.corruption");
}

#[test]
fn facade_transactions_roll_back_data_index_and_metadata_atomically() {
    let store_id = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let by_shelf = catalog_id("3333333333333333");
    let profile = EngineProfile::new(1);
    let identity = [SavedKey::Int(1)];
    let store = TreeStore::memory();

    let path = [DataPathSegment::Member(title.clone())];
    store.write_catalog_epoch(1).expect("seed catalog epoch");
    store.write_engine_profile(&profile).expect("seed profile");
    store.begin().expect("begin");
    store
        .write_data_value(&store_id, &identity, &path, b"Dune".to_vec())
        .expect("write data value");
    store
        .write_index_entry(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &identity,
            b"present".to_vec(),
        )
        .expect("write index");
    store.write_catalog_epoch(2).expect("write catalog epoch");
    store.rollback().expect("rollback");

    assert_eq!(
        store
            .read_data_value(&store_id, &identity, &path)
            .expect("data value"),
        None
    );
    assert_eq!(
        store
            .scan_index_tuple(&by_shelf, &[SavedKey::Str("fiction".into())], 10)
            .expect("index")
            .entries,
        Vec::new()
    );
    assert_eq!(store.read_catalog_epoch().expect("catalog epoch"), Some(1));
    assert_eq!(
        store
            .read_engine_profile_digest()
            .expect("engine profile digest"),
        Some(profile.digest_bytes())
    );
}

#[cfg(feature = "native")]
#[test]
fn metadata_survives_native_redb_reopen() {
    let dir = common::TempDir::new("marrow-store-test").expect("create temp dir");
    let path = dir.path().join("tree-store.redb");
    let profile = EngineProfile::new(3);
    let root = catalog_id("aaaaaaaaaaaaaaaa");
    let index = catalog_id("bbbbbbbbbbbbbbbb");
    {
        let store = TreeStore::open(&path).expect("open native store");
        store.write_catalog_epoch(8).expect("write catalog epoch");
        store
            .write_engine_profile(&profile)
            .expect("write engine profile");
        store
            .write_commit_metadata(&sample_commit_metadata(
                9,
                8,
                profile.layout_epoch(),
                "sha256:0000000000000000000000000000000000000000000000000000000000000008",
                profile.digest_bytes(),
                vec![root.clone()],
                vec![index.clone()],
            ))
            .expect("write commit metadata");
    }

    let store = TreeStore::open_read_only(&path).expect("reopen native store");
    assert_eq!(
        store.read_catalog_epoch().expect("read catalog epoch"),
        Some(8)
    );
    assert_eq!(
        store
            .read_engine_profile_digest()
            .expect("read engine profile digest"),
        Some(profile.digest_bytes())
    );
    assert_eq!(
        store.read_commit_metadata().expect("read commit metadata"),
        Some(sample_commit_metadata(
            9,
            8,
            profile.layout_epoch(),
            "sha256:0000000000000000000000000000000000000000000000000000000000000008",
            profile.digest_bytes(),
            vec![root],
            vec![index],
        ))
    );
}
