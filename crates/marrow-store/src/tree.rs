//! Typed tree-cell store facade over the ordered-byte backend.

use crate::backend::{Backend, StoreError};
use crate::cell::{CatalogId, CellKey, MetaCell, SequencePosition};
use crate::key::{SavedKey, decode_key_value};

const NODE_MARKER: &[u8] = b"node";
const ENGINE_PROFILE_KEY_VERSION_V0: u8 = 0;
const TREE_VALUE_VERSION_V0: u8 = 0;
const ENGINE_PROFILE_DIGEST_BYTES: usize = 8;
const MIN_ENCODED_CATALOG_ID_BYTES: usize = 4 + "cat_0000000000000000".len();
const MIN_LENGTH_PREFIX_BYTES: usize = 4;

pub type EngineProfileDigest = [u8; ENGINE_PROFILE_DIGEST_BYTES];

/// The engine profile recorded with tree-cell metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineProfile {
    layout_epoch: u64,
}

impl EngineProfile {
    pub fn new(layout_epoch: u64) -> Self {
        Self { layout_epoch }
    }

    pub fn layout_epoch(&self) -> u64 {
        self.layout_epoch
    }

    pub fn key_profile_version(&self) -> u8 {
        ENGINE_PROFILE_KEY_VERSION_V0
    }

    pub fn digest_bytes(&self) -> EngineProfileDigest {
        fnv1a64(&self.digest_preimage()).to_be_bytes()
    }

    pub fn digest_hex(&self) -> String {
        let digest = u64::from_be_bytes(self.digest_bytes());
        format!("{digest:016x}")
    }

    fn digest_preimage(&self) -> Vec<u8> {
        let mut bytes = b"marrow-tree-cell-engine-profile-v0".to_vec();
        bytes.push(0);
        bytes.push(self.key_profile_version());
        bytes.extend_from_slice(&self.layout_epoch.to_be_bytes());
        bytes
    }
}

/// Metadata recorded for the latest tree-cell commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMetadata {
    pub commit_id: u64,
    pub catalog_epoch: u64,
    pub layout_epoch: u64,
    pub engine_profile_digest: EngineProfileDigest,
    pub changed_root_catalog_ids: Vec<CatalogId>,
    pub changed_index_catalog_ids: Vec<CatalogId>,
}

/// One index row from an exact index tuple scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub identity: Vec<SavedKey>,
    pub value: Vec<u8>,
}

/// Opaque cursor for resuming an exact index tuple scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexCursor {
    prefix: Vec<u8>,
    last_key: Vec<u8>,
}

/// One bounded page from an exact index tuple scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPage {
    pub entries: Vec<IndexEntry>,
    pub cursor: Option<IndexCursor>,
    pub truncated: bool,
}

/// A typed reference to a stored identity in another catalog-backed store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeReference {
    store: CatalogId,
    identity: Vec<SavedKey>,
}

impl TreeReference {
    pub fn new(store: CatalogId, identity: Vec<SavedKey>) -> Self {
        Self { store, identity }
    }

    pub fn store(&self) -> &CatalogId {
        &self.store
    }

    pub fn identity(&self) -> &[SavedKey] {
        &self.identity
    }
}

/// A catalog-backed enum member value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEnumMember {
    enum_id: CatalogId,
    member_id: CatalogId,
}

impl TreeEnumMember {
    pub fn new(enum_id: CatalogId, member_id: CatalogId) -> Self {
        Self { enum_id, member_id }
    }

    pub fn enum_id(&self) -> &CatalogId {
        &self.enum_id
    }

    pub fn member_id(&self) -> &CatalogId {
        &self.member_id
    }
}

/// A typed tree-cell facade that constructs physical keys from [`CellKey`].
pub struct TreeCellStore<'a, B: Backend + ?Sized> {
    backend: &'a mut B,
}

impl<'a, B: Backend + ?Sized> TreeCellStore<'a, B> {
    pub fn new(backend: &'a mut B) -> Self {
        Self { backend }
    }

    pub fn begin(&mut self) -> Result<(), StoreError> {
        self.backend.begin()
    }

    pub fn commit(&mut self) -> Result<(), StoreError> {
        self.backend.commit()
    }

    pub fn rollback(&mut self) -> Result<(), StoreError> {
        self.backend.rollback()
    }

    pub fn write_catalog_epoch(&mut self, epoch: u64) -> Result<(), StoreError> {
        self.write_u64_meta(MetaCell::CatalogEpoch, epoch)
    }

    pub fn read_catalog_epoch(&self) -> Result<Option<u64>, StoreError> {
        self.read_u64_meta(MetaCell::CatalogEpoch)
    }

    pub fn write_layout_epoch(&mut self, epoch: u64) -> Result<(), StoreError> {
        self.write_u64_meta(MetaCell::LayoutEpoch, epoch)
    }

    pub fn read_layout_epoch(&self) -> Result<Option<u64>, StoreError> {
        self.read_u64_meta(MetaCell::LayoutEpoch)
    }

    pub fn write_engine_profile(&mut self, profile: &EngineProfile) -> Result<(), StoreError> {
        self.write_layout_epoch(profile.layout_epoch())?;
        self.backend.write(
            CellKey::meta(MetaCell::EngineProfile).as_bytes(),
            profile.digest_bytes().to_vec(),
        )
    }

    pub fn read_engine_profile_digest(&self) -> Result<Option<EngineProfileDigest>, StoreError> {
        self.backend
            .read(CellKey::meta(MetaCell::EngineProfile).as_bytes())?
            .map(|bytes| decode_digest(&bytes))
            .transpose()
    }

    pub fn write_commit_metadata(&mut self, metadata: &CommitMetadata) -> Result<(), StoreError> {
        self.backend.write(
            CellKey::meta(MetaCell::Commit).as_bytes(),
            encode_commit_metadata(metadata)?,
        )
    }

    pub fn read_commit_metadata(&self) -> Result<Option<CommitMetadata>, StoreError> {
        self.backend
            .read(CellKey::meta(MetaCell::Commit).as_bytes())?
            .map(|bytes| decode_commit_metadata(&bytes))
            .transpose()
    }

    pub fn write_node(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.backend.write(
            CellKey::node(store, identity).as_bytes(),
            NODE_MARKER.to_vec(),
        )
    }

    pub fn node_exists(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
    ) -> Result<bool, StoreError> {
        self.backend
            .read(CellKey::node(store, identity).as_bytes())
            .map(|value| value.is_some())
    }

    pub fn write_leaf(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.backend
            .write(CellKey::leaf(store, identity, member).as_bytes(), value)
    }

    pub fn read_leaf(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.backend
            .read(CellKey::leaf(store, identity, member).as_bytes())
    }

    pub fn delete_leaf(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::leaf(store, identity, member).as_bytes())
    }

    pub fn write_sequence_position(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.backend.write(
            CellKey::sequence(store, identity, member, position).as_bytes(),
            value,
        )
    }

    pub fn read_sequence_position(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.backend
            .read(CellKey::sequence(store, identity, member, position).as_bytes())
    }

    pub fn delete_sequence_position(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::sequence(store, identity, member, position).as_bytes())
    }

    pub fn write_index_entry(
        &mut self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.backend.write(
            CellKey::index(index, index_keys, identity).as_bytes(),
            value,
        )
    }

    pub fn read_index_entry(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.backend
            .read(CellKey::index(index, index_keys, identity).as_bytes())
    }

    pub fn delete_index_entry(
        &mut self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::index(index, index_keys, identity).as_bytes())
    }

    pub fn scan_index_tuple(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        self.scan_index_tuple_from(index, index_keys, None, limit)
    }

    pub fn scan_index_tuple_after(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        cursor: &IndexCursor,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        self.scan_index_tuple_from(index, index_keys, Some(cursor), limit)
    }

    fn scan_index_tuple_from(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        cursor: Option<&IndexCursor>,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        if limit == 0 {
            return Ok(IndexPage {
                entries: Vec::new(),
                cursor: None,
                truncated: false,
            });
        }
        let prefix = CellKey::index_tuple_prefix(index, index_keys);
        let page = match cursor {
            Some(cursor) => {
                if cursor.prefix != prefix.as_bytes() {
                    return Err(StoreError::InvalidCursor {
                        message: "index cursor does not match exact index tuple".into(),
                    });
                }
                self.backend
                    .scan_after(prefix.as_bytes(), cursor.last_key.as_slice(), limit)?
            }
            None => self.backend.scan(prefix.as_bytes(), limit)?,
        };
        let range = prefix.range();
        let mut entries = Vec::new();
        let mut last_key = None;
        for (key, value) in page.entries {
            if !range.contains(&key) {
                continue;
            }
            last_key = Some(key.clone());
            let identity = decode_index_identity(&key[prefix.as_bytes().len()..], &key)?;
            entries.push(IndexEntry { identity, value });
        }
        let cursor = if page.truncated {
            last_key.map(|last_key| IndexCursor {
                prefix: prefix.as_bytes().to_vec(),
                last_key,
            })
        } else {
            None
        };
        Ok(IndexPage {
            entries,
            cursor,
            truncated: page.truncated,
        })
    }

    fn write_u64_meta(&mut self, cell: MetaCell, value: u64) -> Result<(), StoreError> {
        self.backend
            .write(CellKey::meta(cell).as_bytes(), value.to_be_bytes().to_vec())
    }

    fn read_u64_meta(&self, cell: MetaCell) -> Result<Option<u64>, StoreError> {
        self.backend
            .read(CellKey::meta(cell).as_bytes())?
            .map(|bytes| decode_u64(&bytes))
            .transpose()
    }
}

pub fn encode_tree_reference(value: &TreeReference) -> Result<Vec<u8>, StoreError> {
    let mut bytes = vec![TREE_VALUE_VERSION_V0];
    put_catalog_id(&value.store, &mut bytes)?;
    put_saved_keys(&value.identity, &mut bytes)?;
    Ok(bytes)
}

pub fn decode_tree_reference(bytes: &[u8]) -> Result<TreeReference, StoreError> {
    let mut cursor = Cursor::new(bytes);
    cursor.take_version()?;
    let store = cursor.take_catalog_id()?;
    let identity = cursor.take_saved_keys()?;
    if !cursor.is_empty() {
        return Err(corrupt_cell(bytes));
    }
    Ok(TreeReference { store, identity })
}

pub fn encode_tree_enum_member(value: &TreeEnumMember) -> Result<Vec<u8>, StoreError> {
    let mut bytes = vec![TREE_VALUE_VERSION_V0];
    put_catalog_id(&value.enum_id, &mut bytes)?;
    put_catalog_id(&value.member_id, &mut bytes)?;
    Ok(bytes)
}

pub fn decode_tree_enum_member(bytes: &[u8]) -> Result<TreeEnumMember, StoreError> {
    let mut cursor = Cursor::new(bytes);
    cursor.take_version()?;
    let enum_id = cursor.take_catalog_id()?;
    let member_id = cursor.take_catalog_id()?;
    if !cursor.is_empty() {
        return Err(corrupt_cell(bytes));
    }
    Ok(TreeEnumMember { enum_id, member_id })
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn encode_commit_metadata(metadata: &CommitMetadata) -> Result<Vec<u8>, StoreError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&metadata.commit_id.to_be_bytes());
    bytes.extend_from_slice(&metadata.catalog_epoch.to_be_bytes());
    bytes.extend_from_slice(&metadata.layout_epoch.to_be_bytes());
    put_bytes(&metadata.engine_profile_digest, &mut bytes)?;
    put_catalog_ids(&metadata.changed_root_catalog_ids, &mut bytes)?;
    put_catalog_ids(&metadata.changed_index_catalog_ids, &mut bytes)?;
    Ok(bytes)
}

fn decode_commit_metadata(bytes: &[u8]) -> Result<CommitMetadata, StoreError> {
    let mut cursor = Cursor::new(bytes);
    let commit_id = cursor.take_u64()?;
    let catalog_epoch = cursor.take_u64()?;
    let layout_epoch = cursor.take_u64()?;
    let engine_profile_digest = cursor.take_digest()?;
    let changed_root_catalog_ids = cursor.take_catalog_ids()?;
    let changed_index_catalog_ids = cursor.take_catalog_ids()?;
    if !cursor.is_empty() {
        return Err(corrupt_cell(bytes));
    }
    Ok(CommitMetadata {
        commit_id,
        catalog_epoch,
        layout_epoch,
        engine_profile_digest,
        changed_root_catalog_ids,
        changed_index_catalog_ids,
    })
}

fn put_bytes(value: &[u8], out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(value.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

fn put_catalog_ids(ids: &[CatalogId], out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(ids.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for id in ids {
        put_bytes(id.as_str().as_bytes(), out)?;
    }
    Ok(())
}

fn put_catalog_id(id: &CatalogId, out: &mut Vec<u8>) -> Result<(), StoreError> {
    put_bytes(id.as_str().as_bytes(), out)
}

fn put_saved_keys(keys: &[SavedKey], out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(keys.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell value key count",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for key in keys {
        put_bytes(&crate::key::encode_key_value(key), out)?;
    }
    Ok(())
}

fn decode_u64(bytes: &[u8]) -> Result<u64, StoreError> {
    let raw: [u8; 8] = bytes.try_into().map_err(|_| corrupt_cell(bytes))?;
    Ok(u64::from_be_bytes(raw))
}

fn decode_digest(bytes: &[u8]) -> Result<EngineProfileDigest, StoreError> {
    bytes.try_into().map_err(|_| corrupt_cell(bytes))
}

fn decode_index_identity(bytes: &[u8], full_key: &[u8]) -> Result<Vec<SavedKey>, StoreError> {
    let Some((&terminator, identity)) = bytes.split_last() else {
        return Err(corrupt_cell(full_key));
    };
    if terminator != 0x00 {
        return Err(corrupt_cell(full_key));
    }
    decode_saved_keys(identity, full_key)
}

fn decode_saved_keys(mut bytes: &[u8], full_key: &[u8]) -> Result<Vec<SavedKey>, StoreError> {
    let mut keys = Vec::new();
    while !bytes.is_empty() {
        let (key, consumed) = decode_key_value(bytes).ok_or_else(|| corrupt_cell(full_key))?;
        keys.push(key);
        bytes = &bytes[consumed..];
    }
    Ok(keys)
}

struct Cursor<'a> {
    bytes: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn take_u64(&mut self) -> Result<u64, StoreError> {
        let bytes = self.take(8)?;
        let raw: [u8; 8] = bytes.try_into().map_err(|_| corrupt_cell(bytes))?;
        Ok(u64::from_be_bytes(raw))
    }

    fn take_version(&mut self) -> Result<(), StoreError> {
        let version = self.take(1)?[0];
        if version == TREE_VALUE_VERSION_V0 {
            Ok(())
        } else {
            Err(corrupt_cell(&[version]))
        }
    }

    fn take_bytes(&mut self) -> Result<&'a [u8], StoreError> {
        let len = self.take_u32()? as usize;
        self.take(len)
    }

    fn take_digest(&mut self) -> Result<EngineProfileDigest, StoreError> {
        decode_digest(self.take_bytes()?)
    }

    fn take_catalog_id(&mut self) -> Result<CatalogId, StoreError> {
        let raw = self.take_bytes()?;
        let id = std::str::from_utf8(raw).map_err(|_| corrupt_cell(raw))?;
        CatalogId::new(id).map_err(|_| corrupt_cell(raw))
    }

    fn take_catalog_ids(&mut self) -> Result<Vec<CatalogId>, StoreError> {
        let len = self.take_u32()? as usize;
        if len > self.bytes.len() / MIN_ENCODED_CATALOG_ID_BYTES {
            return Err(corrupt_cell(self.bytes));
        }
        let mut ids = Vec::new();
        for _ in 0..len {
            let raw = self.take_bytes()?;
            let id = std::str::from_utf8(raw).map_err(|_| corrupt_cell(raw))?;
            ids.push(CatalogId::new(id).map_err(|_| corrupt_cell(raw))?);
        }
        Ok(ids)
    }

    fn take_saved_keys(&mut self) -> Result<Vec<SavedKey>, StoreError> {
        let len = self.take_u32()? as usize;
        if len > self.bytes.len() / MIN_LENGTH_PREFIX_BYTES {
            return Err(corrupt_cell(self.bytes));
        }
        let mut keys = Vec::new();
        for _ in 0..len {
            let raw = self.take_bytes()?;
            let (key, consumed) = decode_key_value(raw).ok_or_else(|| corrupt_cell(raw))?;
            if consumed != raw.len() {
                return Err(corrupt_cell(raw));
            }
            keys.push(key);
        }
        Ok(keys)
    }

    fn take_u32(&mut self) -> Result<u32, StoreError> {
        let bytes = self.take(4)?;
        let raw: [u8; 4] = bytes.try_into().map_err(|_| corrupt_cell(bytes))?;
        Ok(u32::from_be_bytes(raw))
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], StoreError> {
        let Some((head, tail)) = self.bytes.split_at_checked(len) else {
            return Err(corrupt_cell(self.bytes));
        };
        self.bytes = tail;
        Ok(head)
    }
}

fn corrupt_cell(bytes: &[u8]) -> StoreError {
    StoreError::Corruption {
        message: format!("tree-cell data is malformed ({} bytes)", bytes.len()),
    }
}
