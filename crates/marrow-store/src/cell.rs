//! Tree-cell storage IDs and the private physical key substrate.
//!
//! The in-memory and redb engines order opaque bytes. This module owns the v0
//! Marrow key profile above those engines while callers use stable catalog IDs,
//! typed key values, and tree-cell operations.

use crate::key::{SavedKey, decode_key_value, encode_escaped_bytes, encode_key_value};

const EMPTY_PLACEMENT_PREFIX: u8 = 0x00;

const TREE_CELL_PROFILE_V0: u8 = 0x01;

const FAMILY_META: u8 = 0x10;
const FAMILY_DATA: u8 = 0x20;
const FAMILY_INDEX: u8 = 0x30;

const NODE_END: u8 = 0x00;
const LEAF_CELL: u8 = 0x10;
const SEQUENCE_CELL: u8 = 0x20;
const DATA_MEMBER_SEGMENT: u8 = 0x30;
const DATA_KEY_SEGMENT: u8 = 0x40;
const DATA_VALUE_END: u8 = 0x00;
const INDEX_IDENTITY: u8 = 0x00;
const INDEX_ENTRY_END: u8 = 0x00;

/// An opaque catalog ID in the tree-cell storage key shape.
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

/// A rejected stable ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellIdError;

impl std::fmt::Display for CellIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tree-cell IDs must match cat_<32 lowercase hex>")
    }
}

impl std::error::Error for CellIdError {}

/// Store-level metadata cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetaCell {
    CatalogEpoch,
    LayoutEpoch,
    EngineProfile,
    Commit,
}

impl MetaCell {
    fn tag(self) -> u8 {
        match self {
            Self::CatalogEpoch => 0x01,
            Self::LayoutEpoch => 0x02,
            Self::EngineProfile => 0x03,
            Self::Commit => 0x04,
        }
    }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataPathSegment {
    Member(CatalogId),
    Key(SavedKey),
}

impl CellKey {
    pub(crate) fn meta(cell: MetaCell) -> Self {
        let mut bytes = family(FAMILY_META);
        bytes.push(cell.tag());
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

    pub(crate) fn data_path_prefix(
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Self {
        let mut bytes = Self::node(store, identity).into_bytes();
        encode_data_path(path, &mut bytes);
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

/// Whether `bytes` is a well-formed data-family cell key — the only family a
/// backup carries and restore replays. Index cells are derived from data and are
/// rebuilt on restore; meta cells are reconstructed from the manifest. A key
/// outside the data family is a malformed backup rather than a cell to write.
pub(crate) fn is_data_cell_key(bytes: &[u8]) -> bool {
    matches!(
        bytes,
        [EMPTY_PLACEMENT_PREFIX, TREE_CELL_PROFILE_V0, family, ..]
            if *family == FAMILY_DATA
    )
}

pub(crate) fn decode_data_child_key(bytes: &[u8]) -> Result<Option<SavedKey>, ()> {
    if bytes.first().copied() != Some(DATA_KEY_SEGMENT) {
        return Ok(None);
    }
    decode_key_value(bytes.get(1..).ok_or(())?)
        .map(|(key, _)| Some(key))
        .ok_or(())
}

/// A decoded data-family cell key: its store catalog id, record identity, and the
/// member/key path below the identity. A backup carries only data cells, so this is
/// what the integrity orphan scan classifies against the checked schema.
pub struct DataCellKey {
    pub store: CatalogId,
    pub identity: Vec<SavedKey>,
    pub path: Vec<DataPathSegment>,
}

/// Decode a data-family cell key into its structural pieces, or `None` when the
/// bytes are not a well-formed data cell under the v0 tree-cell key grammar — the
/// caller reports that as store corruption. Index and meta cells are derived or
/// reconstructed, not carried in a backup, so a non-data key also decodes to `None`.
pub fn decode_data_cell_key(bytes: &[u8]) -> Option<DataCellKey> {
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
    let path = decode_data_cell_path(after_node)?;
    Some(DataCellKey {
        store,
        identity,
        path,
    })
}

/// Decode the run of identity key-values that precede the node terminator. A
/// key-value tag is never `NODE_END`, so the run ends at the first `NODE_END`.
fn decode_leading_keys(mut bytes: &[u8]) -> Option<(Vec<SavedKey>, &[u8])> {
    let mut keys = Vec::new();
    while bytes.first().copied()? != NODE_END {
        let (key, used) = decode_key_value(bytes)?;
        keys.push(key);
        bytes = bytes.get(used..)?;
    }
    Some((keys, bytes))
}

/// Decode the member/key path below a record node from the bytes after the node
/// terminator. An empty tail is a bare record node (path `[]`). A value cell ends
/// at the value marker; a leaf or sequence cell carries one member id after its
/// cell tag. The decoded path keeps only structural member/key segments — a
/// sequence position is not part of the schema path.
fn decode_data_cell_path(mut bytes: &[u8]) -> Option<Vec<DataPathSegment>> {
    let mut path = Vec::new();
    loop {
        let Some(tag) = bytes.first().copied() else {
            return Some(path);
        };
        match tag {
            DATA_VALUE_END => return bytes.get(1..).filter(|rest| rest.is_empty()).map(|_| path),
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
                path.push(DataPathSegment::Member(CatalogId::new(id).ok()?));
                return Some(path);
            }
            SEQUENCE_CELL => {
                let (id, rest) = decode_escaped_id(bytes.get(1..)?)?;
                // The trailing 8-byte sequence position is not a schema path
                // segment; the member id is enough to classify the cell.
                if rest.len() != 8 {
                    return None;
                }
                path.push(DataPathSegment::Member(CatalogId::new(id).ok()?));
                return Some(path);
            }
            _ => return None,
        }
    }
}

/// Decode one escaped id (the `encode_escaped_bytes` form) and return it with the
/// bytes after its `0x00 0x00` terminator.
fn decode_escaped_id(bytes: &[u8]) -> Option<(String, &[u8])> {
    let mut decoded = Vec::new();
    let mut index = 0;
    loop {
        match *bytes.get(index)? {
            0x00 => match *bytes.get(index + 1)? {
                0x00 => {
                    let id = String::from_utf8(decoded).ok()?;
                    return Some((id, bytes.get(index + 2..)?));
                }
                0x01 => {
                    decoded.push(0x00);
                    index += 2;
                }
                _ => return None,
            },
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
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

fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
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
        CatalogId, CellKey, DataPathSegment, FAMILY_DATA, SequencePosition, decode_data_cell_key,
        encode_id, family,
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
        assert_eq!(cell.path, expected_path);
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
        assert_eq!(cell.path, path);
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
        assert_data(&key, &[DataPathSegment::Member(member)]);
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

    // The int key tag begins a fixed 9-byte key; a tag with no body is truncated.
    const KEY_INT_TAG_PROBE: u8 = 0x02;
}
