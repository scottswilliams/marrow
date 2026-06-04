use marrow_store::cell::{CatalogId, SequencePosition};
use marrow_store::key::SavedKey;
use marrow_store::tree::{
    CommitMetadata, DataPathSegment, EngineProfile, IndexPage, TreeEnumMember, TreeReference,
    TreeStore, decode_tree_enum_member, decode_tree_reference, encode_tree_enum_member,
    encode_tree_reference,
};

fn catalog_id(hex: &str) -> CatalogId {
    CatalogId::new(format!("cat_{hex:0>32}")).unwrap()
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
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

#[test]
fn reference_values_store_target_catalog_id_and_identity_keys() {
    let books = catalog_id("1111111111111111");
    let library_books = catalog_id("1111111111111111");
    let authors = catalog_id("2222222222222222");

    let value = TreeReference::new(
        books.clone(),
        vec![SavedKey::Str("isbn-9780441172719".into())],
    );
    let encoded = encode_tree_reference(&value).expect("encode reference");

    assert_eq!(
        decode_tree_reference(&encoded).expect("decode reference"),
        value
    );
    assert_eq!(
        encoded,
        encode_tree_reference(&TreeReference::new(
            library_books,
            vec![SavedKey::Str("isbn-9780441172719".into())],
        ))
        .expect("renamed source uses the same catalog-backed bytes")
    );
    assert_ne!(
        encoded,
        encode_tree_reference(&TreeReference::new(
            authors,
            vec![SavedKey::Str("isbn-9780441172719".into())],
        ))
        .expect("different store identity changes bytes")
    );
    for spelling in ["books", "libraryBooks", "authors"] {
        assert!(
            !contains_subslice(&encoded, spelling.as_bytes()),
            "reference bytes contain source spelling {spelling:?}: {encoded:?}"
        );
    }
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
        .write_commit_metadata(&CommitMetadata {
            commit_id: 55,
            catalog_epoch: 44,
            layout_epoch: profile.layout_epoch(),
            source_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000044"
                    .to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: vec![root.clone()],
            changed_index_catalog_ids: vec![index.clone()],
            activation_evolution_digest: String::new(),
            activation_proposal_catalog_digest: None,
            activation_records_backfilled: 0,
            activation_default_records_by_id: Vec::new(),
            activation_indexes_rebuilt: 0,
            activation_records_retired: 0,
            activation_retire_evidence_digest: String::new(),
            activation_records_retired_by_id: Vec::new(),
            activation_records_transformed: 0,
        })
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
        Some(CommitMetadata {
            commit_id: 55,
            catalog_epoch: 44,
            layout_epoch: profile.layout_epoch(),
            source_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000044"
                    .to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: vec![root],
            changed_index_catalog_ids: vec![index],
            activation_evolution_digest: String::new(),
            activation_proposal_catalog_digest: None,
            activation_records_backfilled: 0,
            activation_default_records_by_id: Vec::new(),
            activation_indexes_rebuilt: 0,
            activation_records_retired: 0,
            activation_retire_evidence_digest: String::new(),
            activation_records_retired_by_id: Vec::new(),
            activation_records_transformed: 0,
        })
    );
}

#[test]
fn node_and_leaf_operations_are_typed() {
    let store_id = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let identity = [SavedKey::Int(7)];
    let store = TreeStore::memory();

    store.write_node(&store_id, &identity).expect("write node");
    assert!(
        store
            .node_exists(&store_id, &identity)
            .expect("node exists")
    );
    assert_eq!(
        store
            .read_leaf(&store_id, &identity, &title)
            .expect("absent leaf"),
        None
    );

    store
        .write_leaf(&store_id, &identity, &title, b"Dune".to_vec())
        .expect("write leaf");
    assert_eq!(
        store
            .read_leaf(&store_id, &identity, &title)
            .expect("read leaf"),
        Some(b"Dune".to_vec())
    );
    store
        .delete_leaf(&store_id, &identity, &title)
        .expect("delete leaf");
    assert_eq!(
        store
            .read_leaf(&store_id, &identity, &title)
            .expect("read leaf"),
        None
    );
}

#[test]
fn sequence_positions_are_typed_cells() {
    let store_id = catalog_id("1111111111111111");
    let tags = catalog_id("3333333333333333");
    let identity = [SavedKey::Int(7)];
    let store = TreeStore::memory();

    store
        .write_sequence_position(
            &store_id,
            &identity,
            &tags,
            SequencePosition::new(3),
            b"classic".to_vec(),
        )
        .expect("write sequence");
    assert_eq!(
        store
            .read_sequence_position(&store_id, &identity, &tags, SequencePosition::new(3))
            .expect("read sequence"),
        Some(b"classic".to_vec())
    );
    store
        .delete_sequence_position(&store_id, &identity, &tags, SequencePosition::new(3))
        .expect("delete sequence");
    assert_eq!(
        store
            .read_sequence_position(&store_id, &identity, &tags, SequencePosition::new(3))
            .expect("read sequence"),
        None
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
    assert_eq!(
        store
            .read_index_entry(&by_shelf, &[SavedKey::Str("fiction".into())], &identity)
            .expect("read index"),
        Some(b"present".to_vec())
    );

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
        store
            .data_child_keys(&books, &identity, &[DataPathSegment::Member(tags.clone())],)
            .expect("ascending children"),
        vec![SavedKey::Int(1), SavedKey::Int(2), SavedKey::Int(3)]
    );
    assert_eq!(
        store
            .data_child_keys_rev(&books, &identity, &[DataPathSegment::Member(tags.clone())])
            .expect("descending children"),
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
        store.record_child_keys(&books, &[]).expect("record keys"),
        vec![SavedKey::Int(1), SavedKey::Int(2), SavedKey::Int(3)]
    );
    assert_eq!(
        store
            .index_child_keys(&by_shelf, &[SavedKey::Str("fiction".into())])
            .expect("index identity keys"),
        vec![SavedKey::Int(1), SavedKey::Int(2), SavedKey::Int(3)]
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
    assert_eq!(
        store
            .read_index_entry(&by_shelf, &[SavedKey::Str("fiction".into())], &identity)
            .expect("read index"),
        None
    );
    assert_eq!(
        store
            .read_index_entry(
                &by_shelf,
                &[SavedKey::Str("fiction".into())],
                &extended_identity,
            )
            .expect("read prefix-related identity"),
        Some(b"extended".to_vec())
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

    let mut keys: Vec<Vec<u8>> = Vec::new();
    store
        .visit_backup_cells(|key, _value| {
            keys.push(key.to_vec());
            Ok(())
        })
        .expect("visit backup cells");

    // Every visited cell is a data-family cell; the index entry is excluded.
    assert!(
        keys.iter().all(|key| key.starts_with(&[0x00, 0x01, 0x20])),
        "backup stream carries only data-family cells: {keys:?}"
    );
    assert!(
        keys.iter().all(|key| !key.starts_with(&[0x00, 0x01, 0x30])),
        "backup stream excludes the index-family entry: {keys:?}"
    );
    // The stream is in ascending encoded-key order.
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "backup cells stream in encoded order");
}

#[test]
fn is_empty_sees_data_and_index_families() {
    let books = catalog_id("1111111111111111");
    let by_title = catalog_id("3333333333333333");
    let store = TreeStore::memory();
    assert!(store.is_empty().expect("fresh store is empty"));

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
        .visit_backup_cells(|_key, _value| {
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
    let _ = books;
}

#[test]
fn restore_cell_rejects_a_non_data_family_key() {
    let store = TreeStore::memory();
    // An index-family key (the `00 01 30` tree-cell index prefix) is derived data a
    // backup never carries; replaying it is a malformed backup.
    let error = store
        .restore_cell(&[0x00, 0x01, 0x30, b'x'], b"entry".to_vec())
        .expect_err("an index-family key is not a restorable backup cell");
    assert_eq!(error.code(), "store.corruption");
}

#[test]
fn malformed_tree_values_are_store_corruption() {
    let error = decode_tree_reference(&[0xff]).expect_err("malformed reference is corruption");
    assert_eq!(error.code(), "store.corruption");

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

    store.write_catalog_epoch(1).expect("seed catalog epoch");
    store.write_engine_profile(&profile).expect("seed profile");
    store.begin().expect("begin");
    store
        .write_leaf(&store_id, &identity, &title, b"Dune".to_vec())
        .expect("write leaf");
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
        store.read_leaf(&store_id, &identity, &title).expect("leaf"),
        None
    );
    assert_eq!(
        store
            .read_index_entry(&by_shelf, &[SavedKey::Str("fiction".into())], &identity)
            .expect("index"),
        None
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
    let dir = tempfile::tempdir().expect("create temp dir");
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
            .write_commit_metadata(&CommitMetadata {
                commit_id: 9,
                catalog_epoch: 8,
                layout_epoch: profile.layout_epoch(),
                source_digest:
                    "sha256:0000000000000000000000000000000000000000000000000000000000000008"
                        .to_string(),
                engine_profile_digest: profile.digest_bytes(),
                changed_root_catalog_ids: vec![root.clone()],
                changed_index_catalog_ids: vec![index.clone()],
                activation_evolution_digest: String::new(),
                activation_proposal_catalog_digest: None,
                activation_records_backfilled: 0,
                activation_default_records_by_id: Vec::new(),
                activation_indexes_rebuilt: 0,
                activation_records_retired: 0,
                activation_retire_evidence_digest: String::new(),
                activation_records_retired_by_id: Vec::new(),
                activation_records_transformed: 0,
            })
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
        Some(CommitMetadata {
            commit_id: 9,
            catalog_epoch: 8,
            layout_epoch: profile.layout_epoch(),
            source_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000008"
                    .to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: vec![root],
            changed_index_catalog_ids: vec![index],
            activation_evolution_digest: String::new(),
            activation_proposal_catalog_digest: None,
            activation_records_backfilled: 0,
            activation_default_records_by_id: Vec::new(),
            activation_indexes_rebuilt: 0,
            activation_records_retired: 0,
            activation_retire_evidence_digest: String::new(),
            activation_records_retired_by_id: Vec::new(),
            activation_records_transformed: 0,
        })
    );
}
