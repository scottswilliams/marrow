//! Tree-cell physical keys are stable-ID storage addresses, not source paths.

use marrow_store::cell::{BlobId, CatalogId, CellKey, MetaCell, SequencePosition};
use marrow_store::key::SavedKey;

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn catalog_id(hex: &str) -> CatalogId {
    CatalogId::new(format!("cat_{hex}")).unwrap()
}

fn encoded_id(hex: &str) -> Vec<u8> {
    let mut bytes = b"cat_".to_vec();
    bytes.extend_from_slice(hex.as_bytes());
    bytes.extend_from_slice(&[0x00, 0x00]);
    bytes
}

#[test]
fn catalog_and_blob_ids_accept_opaque_shape() {
    assert_eq!(
        CatalogId::new("cat_0123456789abcdef").unwrap().as_str(),
        "cat_0123456789abcdef"
    );
    assert_eq!(
        CatalogId::new("cat_0123456789abcdef_1").unwrap().as_str(),
        "cat_0123456789abcdef_1"
    );
    assert_eq!(
        BlobId::new("cat_fedcba9876543210").unwrap().as_str(),
        "cat_fedcba9876543210"
    );
}

#[test]
fn catalog_ids_reject_source_like_spellings_and_malformed_ids() {
    for id in [
        "",
        "books",
        "title",
        "byShelf",
        "cat_title",
        "cat_0123456789abcdeg",
        "cat_0123456789ABCDEF",
        "cat_0123456789abcdef_",
        "cat_0123456789abcdef_0",
        "cat_0123456789abcdef_01",
    ] {
        assert!(CatalogId::new(id).is_err(), "accepted malformed ID {id:?}");
        assert!(
            BlobId::new(id).is_err(),
            "accepted malformed blob ID {id:?}"
        );
    }
}

#[test]
fn v0_layout_bytes_match_the_documented_profile() {
    let store = catalog_id("0123456789abcdef");
    let member = catalog_id("1111111111111111");
    let index_id = catalog_id("2222222222222222");
    let blob = BlobId::new("cat_3333333333333333").unwrap();

    let int_one = [0x02, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
    let str_a = [0x07, b'a', 0x00, 0x00];

    assert_eq!(
        CellKey::meta(MetaCell::EngineProfile).as_bytes(),
        &[0x00, 0x01, 0x10, 0x03]
    );

    let mut catalog = vec![0x00, 0x01, 0x11];
    catalog.extend_from_slice(&encoded_id("0123456789abcdef"));
    assert_eq!(CellKey::catalog(&store).as_bytes(), catalog.as_slice());

    let mut node = vec![0x00, 0x01, 0x20];
    node.extend_from_slice(&encoded_id("0123456789abcdef"));
    node.extend_from_slice(&int_one);
    node.push(0x00);
    assert_eq!(
        CellKey::node(&store, &[SavedKey::Int(1)]).as_bytes(),
        node.as_slice()
    );

    let mut leaf = node.clone();
    leaf.push(0x10);
    leaf.extend_from_slice(&encoded_id("1111111111111111"));
    assert_eq!(
        CellKey::leaf(&store, &[SavedKey::Int(1)], &member).as_bytes(),
        leaf.as_slice()
    );

    let mut sequence = node.clone();
    sequence.push(0x20);
    sequence.extend_from_slice(&encoded_id("1111111111111111"));
    sequence.extend_from_slice(&2u64.to_be_bytes());
    assert_eq!(
        CellKey::sequence(
            &store,
            &[SavedKey::Int(1)],
            &member,
            SequencePosition::new(2)
        )
        .as_bytes(),
        sequence.as_slice()
    );

    let mut index = vec![0x00, 0x01, 0x30];
    index.extend_from_slice(&encoded_id("2222222222222222"));
    index.extend_from_slice(&str_a);
    index.push(0x00);
    index.extend_from_slice(&int_one);
    assert_eq!(
        CellKey::index(&index_id, &[SavedKey::Str("a".into())], &[SavedKey::Int(1)]).as_bytes(),
        index.as_slice()
    );

    let mut blob_chunk = vec![0x00, 0x01, 0x40];
    blob_chunk.extend_from_slice(&encoded_id("3333333333333333"));
    blob_chunk.extend_from_slice(&3u64.to_be_bytes());
    assert_eq!(
        CellKey::blob_chunk(&blob, 3).as_bytes(),
        blob_chunk.as_slice()
    );

    let node_range = CellKey::node(&store, &[SavedKey::Int(1)]).range();
    let mut node_end = node.clone();
    *node_end.last_mut().unwrap() += 1;
    assert_eq!(node_range.start(), node.as_slice());
    assert_eq!(node_range.end(), Some(node_end.as_slice()));
}

#[test]
fn tree_cell_keys_encode_catalog_ids_not_source_spellings() {
    let store = catalog_id("0123456789abcdef");
    let title = catalog_id("1111111111111111");
    let by_shelf = catalog_id("2222222222222222");

    let keys = [
        CellKey::node(&store, &[SavedKey::Int(42)]),
        CellKey::leaf(&store, &[SavedKey::Int(42)], &title),
        CellKey::index(
            &by_shelf,
            &[SavedKey::Str("fiction".into())],
            &[SavedKey::Int(42)],
        ),
    ];
    let spellings = ["books", "title", "byShelf", "libraryItems", "displayTitle"];

    for key in keys {
        for spelling in spellings {
            assert!(
                !contains_subslice(key.as_bytes(), spelling.as_bytes()),
                "physical key contains source spelling {spelling:?}: {key:?}"
            );
        }
    }
}

#[test]
fn typed_key_values_sort_in_marrow_order_inside_node_keys() {
    let store = catalog_id("aaaaaaaaaaaaaaaa");
    let mut encoded = vec![
        (CellKey::node(&store, &[SavedKey::Str("a".into())]), "str a"),
        (CellKey::node(&store, &[SavedKey::Int(2)]), "int 2"),
        (CellKey::node(&store, &[SavedKey::Bool(true)]), "bool true"),
        (CellKey::node(&store, &[SavedKey::Int(-1)]), "int -1"),
        (
            CellKey::node(&store, &[SavedKey::Bool(false)]),
            "bool false",
        ),
        (
            CellKey::node(&store, &[SavedKey::Bytes(vec![0x00])]),
            "bytes",
        ),
    ];

    encoded.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));

    let labels: Vec<&str> = encoded.into_iter().map(|(_, label)| label).collect();
    assert_eq!(
        labels,
        [
            "bool false",
            "bool true",
            "int -1",
            "int 2",
            "str a",
            "bytes"
        ]
    );
}

#[test]
fn leaf_keys_live_under_their_node_key() {
    let store = catalog_id("bbbbbbbbbbbbbbbb");
    let member = catalog_id("cccccccccccccccc");
    let identity = [SavedKey::Int(9)];

    let node = CellKey::node(&store, &identity);
    let leaf = CellKey::leaf(&store, &identity, &member);
    let range = node.range();

    assert!(leaf.as_bytes().starts_with(node.as_bytes()));
    assert!(range.contains(leaf.as_bytes()));
}

#[test]
fn index_cells_sort_by_index_key_then_identity_tie_breaker() {
    let index = catalog_id("dddddddddddddddd");
    let mut encoded = vec![
        (
            CellKey::index(&index, &[SavedKey::Str("b".into())], &[SavedKey::Int(0)]),
            "b/0",
        ),
        (
            CellKey::index(&index, &[SavedKey::Str("a".into())], &[SavedKey::Int(2)]),
            "a/2",
        ),
        (
            CellKey::index(&index, &[SavedKey::Str("a".into())], &[SavedKey::Int(1)]),
            "a/1",
        ),
    ];

    encoded.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));

    let labels: Vec<&str> = encoded.into_iter().map(|(_, label)| label).collect();
    assert_eq!(labels, ["a/1", "a/2", "b/0"]);
}

#[test]
fn exact_index_tuple_range_excludes_longer_tuples_with_the_same_prefix() {
    let index = catalog_id("eeeeeeeeeeeeeeee");
    let exact_a = CellKey::index_tuple_prefix(&index, &[SavedKey::Str("a".into())]).range();

    let a_identity = CellKey::index(&index, &[SavedKey::Str("a".into())], &[SavedKey::Int(1)]);
    let longer_tuple = CellKey::index(
        &index,
        &[SavedKey::Str("a".into()), SavedKey::Bool(false)],
        &[SavedKey::Int(1)],
    );

    assert!(exact_a.contains(a_identity.as_bytes()));
    assert!(
        !exact_a.contains(longer_tuple.as_bytes()),
        "exact index tuple range must not include longer tuples"
    );
}

#[test]
fn sequence_cells_sort_by_position() {
    let store = catalog_id("ffffffffffffffff");
    let member = catalog_id("0000000000000000");
    let identity = [SavedKey::Int(5)];
    let mut encoded = vec![
        (
            CellKey::sequence(&store, &identity, &member, SequencePosition::new(10)),
            10,
        ),
        (
            CellKey::sequence(&store, &identity, &member, SequencePosition::new(2)),
            2,
        ),
        (
            CellKey::sequence(&store, &identity, &member, SequencePosition::new(0)),
            0,
        ),
    ];

    encoded.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));

    let positions: Vec<u64> = encoded.into_iter().map(|(_, position)| position).collect();
    assert_eq!(positions, [0, 2, 10]);
}

#[test]
fn catalog_meta_blob_and_data_cells_do_not_collide() {
    let store = catalog_id("1234567890abcdef");
    let member = catalog_id("fedcba0987654321");
    let blob = BlobId::new("cat_13579bdf2468ace0").unwrap();

    let cells = [
        CellKey::meta(MetaCell::EngineProfile),
        CellKey::catalog(&store),
        CellKey::node(&store, &[SavedKey::Int(1)]),
        CellKey::leaf(&store, &[SavedKey::Int(1)], &member),
        CellKey::blob_chunk(&blob, 0),
    ];

    for (left_index, left) in cells.iter().enumerate() {
        for right in cells.iter().skip(left_index + 1) {
            assert_ne!(left.as_bytes(), right.as_bytes());
        }
    }
}
