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
        f.write_str("tree-cell IDs must match cat_<16 lowercase hex>[_suffix]")
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

pub(crate) fn decode_data_child_key(bytes: &[u8]) -> Result<Option<SavedKey>, ()> {
    if bytes.first().copied() != Some(DATA_KEY_SEGMENT) {
        return Ok(None);
    }
    decode_key_value(bytes.get(1..).ok_or(())?)
        .map(|(key, _)| Some(key))
        .ok_or(())
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
    let Some(rest) = id.strip_prefix("cat_") else {
        return false;
    };
    let (hex, suffix) = match rest.split_once('_') {
        Some((hex, suffix)) => (hex, Some(suffix)),
        None => (rest, None),
    };
    if hex.len() != 16
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return false;
    }
    suffix.is_none_or(is_valid_suffix)
}

fn is_valid_suffix(suffix: &str) -> bool {
    if suffix.is_empty() || suffix == "0" {
        return false;
    }
    if suffix.len() > 1 && suffix.starts_with('0') {
        return false;
    }
    suffix.bytes().all(|byte| byte.is_ascii_digit())
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
