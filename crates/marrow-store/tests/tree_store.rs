use marrow_store::backend::{Backend, StoreError};
use marrow_store::cell::{CatalogId, CellKey, MetaCell, SequencePosition};
use marrow_store::key::SavedKey;
use marrow_store::mem::MemStore;
use marrow_store::tree::{
    CommitMetadata, EngineProfile, IndexPage, TreeCellStore, TreeEnumMember, TreeReference,
    decode_tree_enum_member, decode_tree_reference, encode_tree_enum_member, encode_tree_reference,
};

fn catalog_id(hex: &str) -> CatalogId {
    CatalogId::new(format!("cat_{hex}")).unwrap()
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn raw_keys(store: &dyn Backend) -> Vec<Vec<u8>> {
    let page = store.scan(&[], 32).expect("scan raw backend");
    assert!(!page.truncated, "raw-key fixture exceeded its scan limit");
    page.entries.into_iter().map(|(key, _)| key).collect()
}

fn index_rows(page: IndexPage) -> Vec<(Vec<SavedKey>, Vec<u8>)> {
    page.entries
        .into_iter()
        .map(|entry| (entry.identity, entry.value))
        .collect()
}

#[test]
fn writes_tree_cells_without_source_spelling_keys() {
    let books = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let by_shelf = catalog_id("3333333333333333");
    let mut backend = MemStore::new();
    {
        let mut store = TreeCellStore::new(&mut backend);
        store
            .write_node(&books, &[SavedKey::Int(7)])
            .expect("write node");
        store
            .write_leaf(&books, &[SavedKey::Int(7)], &title, b"Dune".to_vec())
            .expect("write leaf");
        store
            .write_index_entry(
                &by_shelf,
                &[SavedKey::Str("fiction".into())],
                &[SavedKey::Int(7)],
                b"present".to_vec(),
            )
            .expect("write index");
    }

    let keys = raw_keys(&backend);
    assert!(
        keys.iter()
            .any(|key| key == CellKey::node(&books, &[SavedKey::Int(7)]).as_bytes())
    );
    assert!(
        keys.iter()
            .any(|key| key == CellKey::leaf(&books, &[SavedKey::Int(7)], &title).as_bytes())
    );
    assert!(keys.iter().any(|key| {
        key == CellKey::index(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &[SavedKey::Int(7)],
        )
        .as_bytes()
    }));
    for key in keys {
        for spelling in ["books", "title", "byShelf"] {
            assert!(
                !contains_subslice(&key, spelling.as_bytes()),
                "physical key contains source spelling {spelling:?}: {key:?}"
            );
        }
    }
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
    let mut backend = MemStore::new();
    {
        let mut store = TreeCellStore::new(&mut backend);
        store.write_catalog_epoch(44).expect("write catalog epoch");
        store
            .write_engine_profile(&profile)
            .expect("write engine profile");
        store
            .write_commit_metadata(&CommitMetadata {
                commit_id: 55,
                catalog_epoch: 44,
                layout_epoch: profile.layout_epoch(),
                engine_profile_digest: profile.digest_bytes(),
                changed_root_catalog_ids: vec![root.clone()],
                changed_index_catalog_ids: vec![index.clone()],
            })
            .expect("write commit");
    }
    let store = TreeCellStore::new(&mut backend);

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
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: vec![root],
            changed_index_catalog_ids: vec![index],
        })
    );

    assert_eq!(
        Backend::read(&backend, CellKey::meta(MetaCell::CatalogEpoch).as_bytes())
            .expect("read raw catalog epoch"),
        Some(44u64.to_be_bytes().to_vec())
    );
    assert!(
        Backend::read(&backend, CellKey::meta(MetaCell::Commit).as_bytes())
            .expect("read raw commit")
            .is_some()
    );
}

#[test]
fn node_and_leaf_operations_are_typed() {
    let store_id = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let identity = [SavedKey::Int(7)];
    let mut backend = MemStore::new();
    let mut store = TreeCellStore::new(&mut backend);

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
    let mut backend = MemStore::new();
    let mut store = TreeCellStore::new(&mut backend);

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
    let mut backend = MemStore::new();
    let mut store = TreeCellStore::new(&mut backend);

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
    let mut backend = MemStore::new();
    let mut store = TreeCellStore::new(&mut backend);

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
    let mut backend = MemStore::new();
    let store = TreeCellStore::new(&mut backend);

    let empty_page = store
        .scan_index_tuple(&by_shelf, &[SavedKey::Str("fiction".into())], 0)
        .expect("zero-limit scan");
    assert!(empty_page.entries.is_empty());
    assert!(empty_page.cursor.is_none());
    assert!(!empty_page.truncated);
}

#[test]
fn exact_index_tuple_delete_removes_only_the_exact_identity() {
    let by_shelf = catalog_id("4444444444444444");
    let identity = [SavedKey::Int(7)];
    let extended_identity = [SavedKey::Int(7), SavedKey::Bool(false)];
    let mut backend = MemStore::new();
    let mut store = TreeCellStore::new(&mut backend);

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
fn malformed_tree_cell_data_is_store_corruption_not_corrupt_saved_path() {
    let by_shelf = catalog_id("3333333333333333");
    let mut backend = MemStore::new();

    Backend::write(
        &mut backend,
        CellKey::meta(MetaCell::Commit).as_bytes(),
        vec![0x01, 0x02, 0x03],
    )
    .expect("write malformed commit metadata");
    let store = TreeCellStore::new(&mut backend);
    let error = store
        .read_commit_metadata()
        .expect_err("malformed metadata is corruption");
    assert_eq!(error.code(), "store.corruption");
    assert!(!matches!(error, StoreError::CorruptPath { .. }));

    Backend::write(
        &mut backend,
        CellKey::meta(MetaCell::EngineProfile).as_bytes(),
        vec![0x01, 0x02, 0x03],
    )
    .expect("write malformed engine profile digest");
    let store = TreeCellStore::new(&mut backend);
    let error = store
        .read_engine_profile_digest()
        .expect_err("malformed engine profile digest is corruption");
    assert_eq!(error.code(), "store.corruption");
    assert!(!matches!(error, StoreError::CorruptPath { .. }));

    let error = decode_tree_reference(&[0xff]).expect_err("malformed reference is corruption");
    assert_eq!(error.code(), "store.corruption");
    assert!(!matches!(error, StoreError::CorruptPath { .. }));

    let error = decode_tree_enum_member(&[0xff]).expect_err("malformed enum member is corruption");
    assert_eq!(error.code(), "store.corruption");
    assert!(!matches!(error, StoreError::CorruptPath { .. }));

    let mut corrupt_index_key =
        CellKey::index_tuple_prefix(&by_shelf, &[SavedKey::Str("fiction".into())]).into_bytes();
    corrupt_index_key.extend_from_slice(&[0x01, 0x02, 0x00]);
    Backend::write(&mut backend, &corrupt_index_key, b"bad".to_vec())
        .expect("write malformed index cell");
    let store = TreeCellStore::new(&mut backend);
    let error = store
        .scan_index_tuple(&by_shelf, &[SavedKey::Str("fiction".into())], 10)
        .expect_err("malformed index identity is corruption");
    assert_eq!(error.code(), "store.corruption");
    assert!(!matches!(error, StoreError::CorruptPath { .. }));
}

#[test]
fn facade_transactions_roll_back_data_index_and_metadata_atomically() {
    let store_id = catalog_id("1111111111111111");
    let title = catalog_id("2222222222222222");
    let by_shelf = catalog_id("3333333333333333");
    let profile = EngineProfile::new(1);
    let identity = [SavedKey::Int(1)];
    let mut backend = MemStore::new();
    let mut store = TreeCellStore::new(&mut backend);

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
    use marrow_store::redb::RedbStore;

    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("tree-store.redb");
    let profile = EngineProfile::new(3);
    let root = catalog_id("aaaaaaaaaaaaaaaa");
    let index = catalog_id("bbbbbbbbbbbbbbbb");
    {
        let mut backend = RedbStore::open(&path).expect("open redb");
        let mut store = TreeCellStore::new(&mut backend);
        store.write_catalog_epoch(8).expect("write catalog epoch");
        store
            .write_engine_profile(&profile)
            .expect("write engine profile");
        store
            .write_commit_metadata(&CommitMetadata {
                commit_id: 9,
                catalog_epoch: 8,
                layout_epoch: profile.layout_epoch(),
                engine_profile_digest: profile.digest_bytes(),
                changed_root_catalog_ids: vec![root.clone()],
                changed_index_catalog_ids: vec![index.clone()],
            })
            .expect("write commit metadata");
    }

    let mut backend = RedbStore::open(&path).expect("reopen redb");
    let store = TreeCellStore::new(&mut backend);
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
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: vec![root],
            changed_index_catalog_ids: vec![index],
        })
    );
}
