//! Tree-cell storage IDs and the private physical key substrate.
//!
//! Engines order opaque bytes; this module owns the v0 Marrow key profile above
//! them so callers work in catalog IDs, typed key values, and tree-cell terms.

use crate::key::{
    SavedKey, decode_escaped_bytes, decode_key_value, encode_escaped_bytes, encode_key_value,
};

const EMPTY_PLACEMENT_PREFIX: u8 = 0x00;

const TREE_CELL_PROFILE_V0: u8 = 0x01;

const FAMILY_META: u8 = 0x10;
const FAMILY_DATA: u8 = 0x20;
const FAMILY_INDEX: u8 = 0x30;
const FAMILY_CATALOG: u8 = 0x40;

const NODE_END: u8 = 0x00;
const LEAF_CELL: u8 = 0x10;
const SEQUENCE_CELL: u8 = 0x20;
const DATA_MEMBER_SEGMENT: u8 = 0x30;
const DATA_KEY_SEGMENT: u8 = 0x40;
const DATA_VALUE_END: u8 = 0x00;
const INDEX_IDENTITY: u8 = 0x00;
const INDEX_ENTRY_END: u8 = 0x00;

pub(crate) const NODE_MARKER: &[u8] = b"node";

/// A stable catalog identifier, spelled `cat_<32 lowercase hex>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogId(String);

impl CatalogId {
    pub fn new(id: impl Into<String>) -> Result<Self, CellIdError> {
        validate_opaque_id(id.into()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellIdError;

impl std::fmt::Display for CellIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tree-cell IDs must match cat_<32 lowercase hex>")
    }
}

impl std::error::Error for CellIdError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetaCell {
    Commit,
}

/// A sequence position encoded in unsigned numeric order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SequencePosition(u64);

impl SequencePosition {
    pub fn new(position: u64) -> Self {
        Self(position)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// An encoded physical tree-cell key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct CellKey(Vec<u8>);

/// A stable member/key segment below a record node in the tree-cell data family.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DataPathSegment {
    Member(CatalogId),
    Key(SavedKey),
}

impl CellKey {
    pub(crate) fn meta(cell: MetaCell) -> Self {
        let tag = match cell {
            MetaCell::Commit => 0x04,
        };
        let mut bytes = family(FAMILY_META);
        bytes.push(tag);
        Self(bytes)
    }

    pub(crate) fn node(store: &CatalogId, identity: &[SavedKey]) -> Self {
        let mut bytes = data_node_stem(store, identity);
        bytes.push(NODE_END);
        Self(bytes)
    }

    pub(crate) fn leaf(store: &CatalogId, identity: &[SavedKey], member: &CatalogId) -> Self {
        let mut bytes = data_node_stem(store, identity);
        bytes.push(NODE_END);
        bytes.push(LEAF_CELL);
        encode_id(member.as_str(), &mut bytes);
        Self(bytes)
    }

    pub(crate) fn sequence(
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
    ) -> Self {
        let mut bytes = data_node_stem(store, identity);
        bytes.push(NODE_END);
        bytes.push(SEQUENCE_CELL);
        encode_id(member.as_str(), &mut bytes);
        bytes.extend_from_slice(&position.get().to_be_bytes());
        Self(bytes)
    }

    pub(crate) fn record_prefix(store: &CatalogId, identity_prefix: &[SavedKey]) -> Self {
        Self(data_node_stem(store, identity_prefix))
    }

    pub(crate) fn record_child_tag_upper_bound(
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        upper_key_tag: u8,
    ) -> Self {
        let mut bytes = data_node_stem(store, identity_prefix);
        bytes.push(upper_key_tag);
        Self(bytes)
    }

    pub(crate) fn data_path_prefix(
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Self {
        let mut bytes = Self::node(store, identity).into_bytes();
        encode_data_path(path, &mut bytes);
        Self(bytes)
    }

    pub(crate) fn data_path_child_tag_upper_bound(
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        upper_key_tag: u8,
    ) -> Self {
        let mut bytes = Self::data_path_prefix(store, identity, path).into_bytes();
        bytes.push(DATA_KEY_SEGMENT);
        bytes.push(upper_key_tag);
        Self(bytes)
    }

    pub(crate) fn data_path_value(
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Self {
        if path.is_empty() {
            return Self::node(store, identity);
        }
        let mut bytes = Self::data_path_prefix(store, identity, path).into_bytes();
        bytes.push(DATA_VALUE_END);
        Self(bytes)
    }

    pub(crate) fn index(index: &CatalogId, index_keys: &[SavedKey], identity: &[SavedKey]) -> Self {
        let mut bytes = family(FAMILY_INDEX);
        encode_id(index.as_str(), &mut bytes);
        encode_keys(index_keys, &mut bytes);
        bytes.push(INDEX_IDENTITY);
        encode_keys(identity, &mut bytes);
        bytes.push(INDEX_ENTRY_END);
        Self(bytes)
    }

    pub(crate) fn index_key_prefix(index: &CatalogId, index_keys: &[SavedKey]) -> Self {
        let mut bytes = family(FAMILY_INDEX);
        encode_id(index.as_str(), &mut bytes);
        encode_keys(index_keys, &mut bytes);
        Self(bytes)
    }

    pub(crate) fn index_tuple_prefix(index: &CatalogId, index_keys: &[SavedKey]) -> Self {
        let mut bytes = family(FAMILY_INDEX);
        encode_id(index.as_str(), &mut bytes);
        encode_keys(index_keys, &mut bytes);
        bytes.push(INDEX_IDENTITY);
        Self(bytes)
    }

    /// The prefix covering every data-family cell (nodes, leaves, sequences).
    pub(crate) fn data_family() -> Self {
        Self(family(FAMILY_DATA))
    }

    /// The prefix covering every index-family cell.
    pub(crate) fn index_family() -> Self {
        Self(family(FAMILY_INDEX))
    }

    /// The prefix covering every catalog-family cell. The accepted-catalog table
    /// lives under its own family, disjoint from the data, index, and meta keys, so
    /// no data/index/meta API can read, write, or scan a catalog row.
    pub(crate) fn catalog_family() -> Self {
        Self(family(FAMILY_CATALOG))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub(crate) fn range(&self) -> CellRange {
        CellRange::for_prefix(self.as_bytes())
    }
}

/// A key-grammar byte run that did not parse under the v0 tree-cell profile. The
/// caller, which still holds the full physical key, turns this into a store error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MalformedKey;

/// Decodes the immediate data-family child key after a data-path prefix; `Ok(None)`
/// once the bytes sit past the keyed-child run.
pub(crate) fn decode_data_child_key(bytes: &[u8]) -> Result<Option<SavedKey>, MalformedKey> {
    if bytes.first().copied() != Some(DATA_KEY_SEGMENT) {
        return Ok(None);
    }
    let body = bytes.get(1..).ok_or(MalformedKey)?;
    let (key, _) = decode_key_value(body).ok_or(MalformedKey)?;
    Ok(Some(key))
}

/// Decodes the next index-family child key after an index-key prefix: the next index
/// key, or the first identity key past the `INDEX_IDENTITY` separator.
pub(crate) fn decode_index_child_key(bytes: &[u8]) -> Result<Option<SavedKey>, MalformedKey> {
    let body = match bytes.split_first() {
        None => return Ok(None),
        Some((&INDEX_IDENTITY, [])) => return Ok(None),
        Some((&INDEX_IDENTITY, rest)) => rest,
        Some(_) => bytes,
    };
    let (key, _) = decode_key_value(body).ok_or(MalformedKey)?;
    Ok(Some(key))
}

/// Splits an index-entry tail into its stored index-key tuple and record identity,
/// which the `INDEX_IDENTITY` separator divides.
pub(crate) fn decode_index_entry_key(
    bytes: &[u8],
) -> Result<(Vec<SavedKey>, Vec<SavedKey>), MalformedKey> {
    let mut rest = bytes;
    let mut index_keys = Vec::new();
    loop {
        match rest.first().copied() {
            None => return Err(MalformedKey),
            Some(INDEX_IDENTITY) => {
                rest = rest.get(1..).ok_or(MalformedKey)?;
                break;
            }
            Some(_) => {
                let (key, consumed) = decode_key_value(rest).ok_or(MalformedKey)?;
                index_keys.push(key);
                rest = rest.get(consumed..).ok_or(MalformedKey)?;
            }
        }
    }
    Ok((index_keys, decode_index_identity(rest)?))
}

/// Decodes a stored index identity: a run of keys closed by the `INDEX_ENTRY_END` terminator.
pub(crate) fn decode_index_identity(bytes: &[u8]) -> Result<Vec<SavedKey>, MalformedKey> {
    let (&terminator, identity) = bytes.split_last().ok_or(MalformedKey)?;
    if terminator != INDEX_ENTRY_END {
        return Err(MalformedKey);
    }
    decode_saved_keys(identity)
}

/// Decodes a run of scalar key-values that fills the whole slice.
fn decode_saved_keys(mut bytes: &[u8]) -> Result<Vec<SavedKey>, MalformedKey> {
    let mut keys = Vec::new();
    while !bytes.is_empty() {
        let (key, consumed) = decode_key_value(bytes).ok_or(MalformedKey)?;
        keys.push(key);
        bytes = bytes.get(consumed..).ok_or(MalformedKey)?;
    }
    Ok(keys)
}

/// Which kind of data-family tree cell a typed backup cell carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataCellKind {
    Node,
    Leaf {
        member: CatalogId,
    },
    Sequence {
        member: CatalogId,
        position: SequencePosition,
    },
    Value {
        path: Vec<DataPathSegment>,
    },
}

/// A decoded data-family cell. A backup carries only data cells, so tooling
/// classifies these typed facts without touching physical key bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataCellKey {
    pub store: CatalogId,
    pub identity: Vec<SavedKey>,
    pub kind: DataCellKind,
}

impl DataCellKey {
    pub fn path(&self) -> Vec<DataPathSegment> {
        match &self.kind {
            DataCellKind::Node => Vec::new(),
            DataCellKind::Leaf { member } | DataCellKind::Sequence { member, .. } => {
                vec![DataPathSegment::Member(member.clone())]
            }
            DataCellKind::Value { path } => path.clone(),
        }
    }
}

/// Decodes a data-family cell key into its structural pieces. Returns `None` for a
/// non-data key (index cells are rebuilt and commit metadata is restamped, not backed
/// up) or for bytes that are malformed under the v0 grammar, which the caller treats
/// as corruption.
pub(crate) fn decode_data_cell_key(bytes: &[u8]) -> Option<DataCellKey> {
    let [
        EMPTY_PLACEMENT_PREFIX,
        TREE_CELL_PROFILE_V0,
        FAMILY_DATA,
        rest @ ..,
    ] = bytes
    else {
        return None;
    };
    let (store, after_store) = decode_escaped_id(rest)?;
    let store = CatalogId::new(store).ok()?;
    let (identity, after_identity) = decode_leading_keys(after_store)?;
    let after_node = after_identity.strip_prefix(&[NODE_END])?;
    let kind = decode_data_cell_kind(after_node)?;
    Some(DataCellKey {
        store,
        identity,
        kind,
    })
}

/// Decodes the identity key-values before the node terminator. A key tag is never
/// `NODE_END`, so the run ends unambiguously at the first `NODE_END`.
fn decode_leading_keys(mut bytes: &[u8]) -> Option<(Vec<SavedKey>, &[u8])> {
    let mut keys = Vec::new();
    while bytes.first().copied()? != NODE_END {
        let (key, used) = decode_key_value(bytes)?;
        keys.push(key);
        bytes = bytes.get(used..)?;
    }
    Some((keys, bytes))
}

/// Classifies the cell from the bytes after the node terminator. The decoded path
/// keeps only structural member/key segments; a sequence position is not part of it.
fn decode_data_cell_kind(mut bytes: &[u8]) -> Option<DataCellKind> {
    let mut path = Vec::new();
    loop {
        let Some(tag) = bytes.first().copied() else {
            return path.is_empty().then_some(DataCellKind::Node);
        };
        match tag {
            DATA_VALUE_END => {
                return bytes
                    .get(1..)
                    .filter(|rest| rest.is_empty())
                    .map(|_| DataCellKind::Value { path });
            }
            DATA_MEMBER_SEGMENT => {
                let (id, rest) = decode_escaped_id(bytes.get(1..)?)?;
                path.push(DataPathSegment::Member(CatalogId::new(id).ok()?));
                bytes = rest;
            }
            DATA_KEY_SEGMENT => {
                let (key, used) = decode_key_value(bytes.get(1..)?)?;
                path.push(DataPathSegment::Key(key));
                bytes = bytes.get(1 + used..)?;
            }
            LEAF_CELL => {
                let (id, rest) = decode_escaped_id(bytes.get(1..)?)?;
                if !rest.is_empty() {
                    return None;
                }
                return Some(DataCellKind::Leaf {
                    member: CatalogId::new(id).ok()?,
                });
            }
            SEQUENCE_CELL => {
                let (id, rest) = decode_escaped_id(bytes.get(1..)?)?;
                let position = rest.try_into().ok().map(u64::from_be_bytes)?;
                return Some(DataCellKind::Sequence {
                    member: CatalogId::new(id).ok()?,
                    position: SequencePosition::new(position),
                });
            }
            _ => return None,
        }
    }
}

/// Decodes one escaped id and returns it with the bytes after its terminator.
fn decode_escaped_id(bytes: &[u8]) -> Option<(String, &[u8])> {
    let (decoded, used) = decode_escaped_bytes(bytes)?;
    let id = String::from_utf8(decoded).ok()?;
    Some((id, bytes.get(used..)?))
}

fn encode_data_path(path: &[DataPathSegment], out: &mut Vec<u8>) {
    for segment in path {
        match segment {
            DataPathSegment::Member(member) => {
                out.push(DATA_MEMBER_SEGMENT);
                encode_id(member.as_str(), out);
            }
            DataPathSegment::Key(key) => {
                out.push(DATA_KEY_SEGMENT);
                out.extend_from_slice(&encode_key_value(key));
            }
        }
    }
}

/// A half-open byte range over a cell prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CellRange {
    start: Vec<u8>,
    end: Option<Vec<u8>>,
}

impl CellRange {
    fn for_prefix(prefix: &[u8]) -> Self {
        Self {
            start: prefix.to_vec(),
            end: prefix_successor(prefix),
        }
    }

    pub(crate) fn contains(&self, key: &[u8]) -> bool {
        if key < self.start.as_slice() {
            return false;
        }
        match &self.end {
            Some(end) => key < end.as_slice(),
            None => true,
        }
    }
}

fn validate_opaque_id(id: String) -> Result<String, CellIdError> {
    if is_valid_opaque_id(&id) {
        Ok(id)
    } else {
        Err(CellIdError)
    }
}

fn is_valid_opaque_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("cat_") else {
        return false;
    };
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn family(tag: u8) -> Vec<u8> {
    vec![EMPTY_PLACEMENT_PREFIX, TREE_CELL_PROFILE_V0, tag]
}

fn data_node_stem(store: &CatalogId, identity: &[SavedKey]) -> Vec<u8> {
    let mut bytes = family(FAMILY_DATA);
    encode_id(store.as_str(), &mut bytes);
    encode_keys(identity, &mut bytes);
    bytes
}

fn encode_keys(keys: &[SavedKey], out: &mut Vec<u8>) {
    for key in keys {
        out.extend_from_slice(&encode_key_value(key));
    }
}

fn encode_id(id: &str, out: &mut Vec<u8>) {
    encode_escaped_bytes(id.as_bytes(), out);
}

pub(crate) fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.last_mut() {
        if *last < 0xff {
            *last += 1;
            return Some(end);
        }
        end.pop();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        CatalogId, CellKey, DataCellKey, DataCellKind, DataPathSegment, FAMILY_DATA,
        SequencePosition, decode_data_cell_key, decode_index_child_key, decode_index_entry_key,
        decode_index_identity, encode_id, family,
    };
    use crate::key::SavedKey;

    fn store_id(suffix: &str) -> CatalogId {
        CatalogId::new(format!("cat_0123456789abcdef0123456789abcdef{suffix}")).expect("catalog id")
    }

    fn member_id() -> CatalogId {
        CatalogId::new("cat_fedcba9876543210fedcba9876543210").expect("member id")
    }

    #[test]
    fn catalog_ids_use_128_bit_lowercase_hex() {
        assert!(CatalogId::new("cat_0123456789abcdef0123456789abcdef").is_ok());
        assert!(CatalogId::new("cat_0123456789abcdef").is_err());
        assert!(CatalogId::new("cat_0123456789abcdef0123456789abcdeF").is_err());
        assert!(CatalogId::new("cat_0123456789abcdef0123456789abcdef_1").is_err());
    }

    fn assert_data(key: &CellKey, expected_path: &[DataPathSegment]) {
        let store = store_id("");
        let cell = decode_data_cell_key(key.as_bytes()).expect("a data cell decodes");
        assert_eq!(cell.store, store);
        assert_eq!(cell.path(), expected_path);
    }

    #[test]
    fn decodes_a_value_cell_into_its_store_identity_and_member_path() {
        let store = store_id("");
        let member = member_id();
        let identity = vec![SavedKey::Int(1)];
        let path = vec![DataPathSegment::Member(member.clone())];
        let key = CellKey::data_path_value(&store, &identity, &path);
        let cell = decode_data_cell_key(key.as_bytes()).expect("decode");
        assert_eq!(cell.store, store);
        assert_eq!(cell.identity, identity);
        assert_eq!(cell.kind, DataCellKind::Value { path });
    }

    #[test]
    fn decodes_a_keyed_member_value_cell() {
        let store = store_id("");
        let member = member_id();
        let path = vec![
            DataPathSegment::Member(member.clone()),
            DataPathSegment::Key(SavedKey::Int(10)),
        ];
        let key = CellKey::data_path_value(&store, &[], &path);
        assert_data(&key, &path);
    }

    #[test]
    fn decodes_a_leaf_cell_and_a_bare_node_cell() {
        let store = store_id("");
        let member = member_id();
        let leaf = CellKey::leaf(&store, &[SavedKey::Int(1)], &member);
        assert_data(&leaf, &[DataPathSegment::Member(member)]);

        let node = CellKey::node(&store, &[SavedKey::Int(1)]);
        assert_data(&node, &[]);
    }

    #[test]
    fn decodes_a_sequence_cell_to_its_member_without_the_position() {
        let store = store_id("");
        let member = member_id();
        let key = CellKey::sequence(&store, &[], &member, SequencePosition::new(7));
        let cell = decode_data_cell_key(key.as_bytes()).expect("decode sequence");
        assert_eq!(
            cell.kind,
            DataCellKind::Sequence {
                member,
                position: SequencePosition::new(7)
            }
        );
    }

    #[test]
    fn data_cell_key_codec_round_trips_each_data_cell_kind() {
        let store = store_id("");
        let member = member_id();
        let identity = vec![SavedKey::Int(7), SavedKey::Str("a\u{0}b".into())];
        let path = vec![
            DataPathSegment::Member(member.clone()),
            DataPathSegment::Key(SavedKey::Bytes(vec![0x00, 0xff])),
        ];
        let cases = [
            (
                CellKey::node(&store, &identity),
                DataCellKey {
                    store: store.clone(),
                    identity: identity.clone(),
                    kind: DataCellKind::Node,
                },
            ),
            (
                CellKey::leaf(&store, &identity, &member),
                DataCellKey {
                    store: store.clone(),
                    identity: identity.clone(),
                    kind: DataCellKind::Leaf {
                        member: member.clone(),
                    },
                },
            ),
            (
                CellKey::sequence(&store, &identity, &member, SequencePosition::new(42)),
                DataCellKey {
                    store: store.clone(),
                    identity: identity.clone(),
                    kind: DataCellKind::Sequence {
                        member: member.clone(),
                        position: SequencePosition::new(42),
                    },
                },
            ),
            (
                CellKey::data_path_value(&store, &identity, &path),
                DataCellKey {
                    store: store.clone(),
                    identity: identity.clone(),
                    kind: DataCellKind::Value { path },
                },
            ),
        ];

        for (encoded, expected) in cases {
            assert_eq!(decode_data_cell_key(encoded.as_bytes()), Some(expected));
        }
    }

    #[test]
    fn rejects_a_non_data_cell_and_a_corrupt_key() {
        // An index cell is not a data cell, so it does not decode as one.
        let index = member_id();
        let index_key = CellKey::index(&index, &[SavedKey::Int(1)], &[SavedKey::Int(2)]);
        assert!(decode_data_cell_key(index_key.as_bytes()).is_none());

        // A data-family prefix with a truncated key body does not decode.
        let mut corrupt = family(FAMILY_DATA);
        encode_id(store_id("").as_str(), &mut corrupt);
        corrupt.push(KEY_INT_TAG_PROBE);
        assert!(decode_data_cell_key(&corrupt).is_none());
        // A non-cell key (wrong placement/profile) does not decode.
        assert!(decode_data_cell_key(&[0x01, 0x02, 0x03]).is_none());
    }

    fn after_prefix<'a>(prefix: &CellKey, key: &'a CellKey) -> &'a [u8] {
        key.as_bytes()
            .strip_prefix(prefix.as_bytes())
            .expect("key extends the prefix")
    }

    #[test]
    fn index_child_decode_reads_the_next_index_key_then_the_identity() {
        let index = member_id();
        let index_keys = [SavedKey::Str("title".into())];
        let identity = [SavedKey::Int(7)];
        let key = CellKey::index(&index, &index_keys, &identity);

        let key_prefix = CellKey::index_key_prefix(&index, &[]);
        assert_eq!(
            decode_index_child_key(after_prefix(&key_prefix, &key)),
            Ok(Some(index_keys[0].clone())),
            "the child below the index id is the first index key",
        );

        let tuple_prefix = CellKey::index_key_prefix(&index, &index_keys);
        assert_eq!(
            decode_index_child_key(after_prefix(&tuple_prefix, &key)),
            Ok(Some(identity[0].clone())),
            "the child below the full key tuple is the first identity key",
        );
    }

    #[test]
    fn index_entry_decode_round_trips_the_stored_key_tuple_and_identity() {
        let index = member_id();
        let index_keys = vec![SavedKey::Str("title".into()), SavedKey::Int(2)];
        let identity = vec![SavedKey::Int(7), SavedKey::Str("x".into())];
        let key = CellKey::index(&index, &index_keys, &identity);

        let prefix = CellKey::index_key_prefix(&index, &[]);
        let decoded = decode_index_entry_key(after_prefix(&prefix, &key)).expect("entry decodes");
        assert_eq!(decoded, (index_keys.clone(), identity.clone()));

        let tuple_prefix = CellKey::index_tuple_prefix(&index, &index_keys);
        let identity_bytes = after_prefix(&tuple_prefix, &key);
        assert_eq!(
            decode_index_identity(identity_bytes),
            Ok(identity),
            "the identity decodes from the bytes after the key-tuple separator",
        );
    }

    #[test]
    fn child_decoders_stop_cleanly_past_their_run_and_reject_malformed_bytes() {
        // A separator and an empty tail sit past the index child run.
        assert_eq!(decode_index_child_key(&[0x00]), Ok(None));
        assert_eq!(decode_index_child_key(&[]), Ok(None));

        // A child tag with an unparseable key body is malformed, not end-of-run.
        assert!(decode_index_child_key(&[0xff]).is_err());
    }

    // The int key tag begins a fixed 9-byte key; a tag with no body is truncated.
    const KEY_INT_TAG_PROBE: u8 = 0x02;
}
