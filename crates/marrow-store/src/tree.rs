//! Typed tree-cell store facade over the private ordered-byte engine.

use std::cell::RefCell;

use crate::backend::{Backend, ScanPage, StoreError};
use crate::cell::{CatalogId, CellKey, MetaCell, SequencePosition, is_data_cell_key};
use crate::key::{SavedKey, decode_key_value};

pub use crate::cell::DataPathSegment;

const NODE_MARKER: &[u8] = b"node";
/// How many cells a backup traversal pages at a time, so the whole store is
/// streamed in bounded chunks rather than materialized at once.
const BACKUP_SCAN_PAGE: usize = 1024;
const ENGINE_PROFILE_KEY_VERSION_V0: u8 = 0;
const TREE_VALUE_VERSION_V0: u8 = 0;
const ENGINE_PROFILE_DIGEST_BYTES: usize = 8;
const MIN_ENCODED_CATALOG_ID_BYTES: usize = 4 + "cat_00000000000000000000000000000000".len();
const MIN_LENGTH_PREFIX_BYTES: usize = 4;
const CHILD_SCAN_PAGE_LIMIT: usize = 128;

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
    /// The analyzed-source digest the commit activated, in the `sha256:<hex>` form the
    /// evolution witness records. It binds the schema shape (member types, identity key
    /// types, index uniqueness and columns) the store was last written against, so the
    /// activation fence can reject a structurally different schema even at the same
    /// catalog epoch.
    pub source_digest: String,
    pub engine_profile_digest: EngineProfileDigest,
    pub changed_root_catalog_ids: Vec<CatalogId>,
    pub changed_index_catalog_ids: Vec<CatalogId>,
    pub activation_evolution_digest: String,
    pub activation_proposal_catalog_digest: Option<String>,
    pub activation_records_backfilled: u64,
    pub activation_default_records_by_id: Vec<ActivationDefaultRecordCount>,
    pub activation_indexes_rebuilt: u64,
    pub activation_records_retired: u64,
    pub activation_retire_evidence_digest: String,
    pub activation_records_retired_by_id: Vec<(CatalogId, u64)>,
    pub activation_records_transformed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationDefaultRecordCount {
    pub catalog_id: CatalogId,
    pub records_backfilled: u64,
    pub target_records: u64,
    pub evidence_digest: String,
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

struct TreeCellStore<'a, B: Backend + ?Sized> {
    backend: &'a mut B,
}

/// An owning tree-cell store handle for runtime and tooling callers.
pub struct TreeStore {
    backend: RefCell<Box<dyn Backend>>,
}

impl TreeStore {
    pub fn memory() -> Self {
        Self::from_backend(Box::new(crate::mem::MemStore::default()))
    }

    #[cfg(feature = "native")]
    pub fn open(path: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self::from_backend(Box::new(crate::redb::RedbStore::open(
            path,
        )?)))
    }

    #[cfg(feature = "native")]
    pub fn open_read_only(path: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self::from_backend(Box::new(
            crate::redb::RedbStore::open_read_only(path)?,
        )))
    }

    fn from_backend(backend: Box<dyn Backend>) -> Self {
        Self {
            backend: RefCell::new(backend),
        }
    }

    pub fn begin(&self) -> Result<(), StoreError> {
        self.backend.borrow_mut().begin()
    }

    pub fn commit(&self) -> Result<(), StoreError> {
        self.backend.borrow_mut().commit()
    }

    pub fn rollback(&self) -> Result<(), StoreError> {
        self.backend.borrow_mut().rollback()
    }

    pub fn write_catalog_epoch(&self, epoch: u64) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.write_catalog_epoch(epoch))
    }

    pub fn read_catalog_epoch(&self) -> Result<Option<u64>, StoreError> {
        self.with_cell(|cell| cell.read_catalog_epoch())
    }

    pub fn write_layout_epoch(&self, epoch: u64) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.write_layout_epoch(epoch))
    }

    pub fn read_layout_epoch(&self) -> Result<Option<u64>, StoreError> {
        self.with_cell(|cell| cell.read_layout_epoch())
    }

    pub fn write_engine_profile(&self, profile: &EngineProfile) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.write_engine_profile(profile))
    }

    pub fn read_engine_profile_digest(&self) -> Result<Option<EngineProfileDigest>, StoreError> {
        self.with_cell(|cell| cell.read_engine_profile_digest())
    }

    pub fn write_commit_metadata(&self, metadata: &CommitMetadata) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.write_commit_metadata(metadata))
    }

    pub fn read_commit_metadata(&self) -> Result<Option<CommitMetadata>, StoreError> {
        self.with_cell(|cell| cell.read_commit_metadata())
    }

    pub fn write_node(&self, store: &CatalogId, identity: &[SavedKey]) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.write_node(store, identity))
    }

    pub fn node_exists(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
    ) -> Result<bool, StoreError> {
        self.with_cell(|cell| cell.node_exists(store, identity))
    }

    pub fn write_leaf(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.write_leaf(store, identity, member, value))
    }

    pub fn read_leaf(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.with_cell(|cell| cell.read_leaf(store, identity, member))
    }

    pub fn delete_leaf(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.delete_leaf(store, identity, member))
    }

    pub fn write_sequence_position(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| {
            cell.write_sequence_position(store, identity, member, position, value)
        })
    }

    pub fn read_sequence_position(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.with_cell(|cell| cell.read_sequence_position(store, identity, member, position))
    }

    pub fn delete_sequence_position(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.delete_sequence_position(store, identity, member, position))
    }

    pub fn write_data_value(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.write_data_value(store, identity, path, value))
    }

    pub fn read_data_value(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.with_cell(|cell| cell.read_data_value(store, identity, path))
    }

    pub fn delete_data_subtree(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.delete_data_subtree(store, identity, path))
    }

    pub fn data_subtree_exists(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<bool, StoreError> {
        self.with_cell(|cell| cell.data_subtree_exists(store, identity, path))
    }

    pub fn data_child_keys(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Vec<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.data_child_keys(store, identity, path))
    }

    pub fn data_child_keys_rev(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Vec<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.data_child_keys_rev(store, identity, path))
    }

    pub fn data_next_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.data_next_child(store, identity, path, after))
    }

    pub fn data_first_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.data_first_child(store, identity, path))
    }

    pub fn data_last_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.data_last_child(store, identity, path))
    }

    pub fn data_prev_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.data_prev_child(store, identity, path, before))
    }

    pub fn data_child_count(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<usize, StoreError> {
        self.with_cell(|cell| cell.data_child_count(store, identity, path))
    }

    pub fn max_int_data_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<i64>, StoreError> {
        self.with_cell(|cell| cell.max_int_data_child(store, identity, path))
    }

    pub fn record_child_keys(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Vec<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.record_child_keys(store, identity_prefix))
    }

    pub fn record_child_count(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<usize, StoreError> {
        self.with_cell(|cell| cell.record_child_count(store, identity_prefix))
    }

    pub fn delete_record_subtree(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.delete_record_subtree(store, identity_prefix))
    }

    pub fn record_child_keys_rev(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Vec<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.record_child_keys_rev(store, identity_prefix))
    }

    pub fn record_next_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.record_next_child(store, identity_prefix, after))
    }

    pub fn record_first_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.record_first_child(store, identity_prefix))
    }

    pub fn record_last_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.record_last_child(store, identity_prefix))
    }

    pub fn record_prev_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.record_prev_child(store, identity_prefix, before))
    }

    pub fn max_int_record_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<i64>, StoreError> {
        self.with_cell(|cell| cell.max_int_record_child(store, identity_prefix))
    }

    /// Visit every record identity under `store_id`, descending `arity` key levels and
    /// invoking `visit` with each full identity tuple. The descent reads one key at a
    /// time through the paged record cursor, so the scan never materializes the whole
    /// store; only the current identity path is held. A store always has at least one
    /// identity level, so an `arity` of zero is treated as one.
    pub fn for_each_record(
        &self,
        store_id: &CatalogId,
        arity: usize,
        visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let mut identity = Vec::new();
        self.descend_records(store_id, arity.max(1), &mut identity, visit)
    }

    fn descend_records(
        &self,
        store_id: &CatalogId,
        remaining: usize,
        identity: &mut Vec<SavedKey>,
        visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let mut next = self.record_first_child(store_id, identity)?;
        while let Some(key) = next {
            identity.push(key.clone());
            if remaining == 1 {
                visit(identity)?;
            } else {
                self.descend_records(store_id, remaining - 1, identity, visit)?;
            }
            identity.pop();
            next = self.record_next_child(store_id, identity, &key)?;
        }
        Ok(())
    }

    pub fn write_index_entry(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.write_index_entry(index, index_keys, identity, value))
    }

    pub fn read_index_entry(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.with_cell(|cell| cell.read_index_entry(index, index_keys, identity))
    }

    pub fn delete_index_entry(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.delete_index_entry(index, index_keys, identity))
    }

    pub fn index_child_keys(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Vec<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.index_child_keys(index, key_prefix))
    }

    pub fn delete_index_subtree(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.delete_index_subtree(index, key_prefix))
    }

    pub fn index_child_keys_rev(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Vec<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.index_child_keys_rev(index, key_prefix))
    }

    pub fn index_next_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.index_next_child(index, key_prefix, after))
    }

    pub fn index_first_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.index_first_child(index, key_prefix))
    }

    pub fn index_last_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.index_last_child(index, key_prefix))
    }

    pub fn index_prev_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.with_cell(|cell| cell.index_prev_child(index, key_prefix, before))
    }

    pub fn scan_index_tuple(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        self.with_cell(|cell| cell.scan_index_tuple(index, index_keys, limit))
    }

    pub fn scan_index_tuple_after(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        cursor: &IndexCursor,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        self.with_cell(|cell| cell.scan_index_tuple_after(index, index_keys, cursor, limit))
    }

    /// Pin a consistent read snapshot for the lifetime of the returned guard, so a
    /// multi-call traversal — a backup, or a long-lived inspection — reads one
    /// coherent version of saved data even while a writer commits.
    pub fn read_snapshot(&self) -> Result<ReadSnapshot<'_>, StoreError> {
        self.backend.borrow_mut().begin_snapshot()?;
        Ok(ReadSnapshot { store: self })
    }

    /// Whether the store holds no saved data: no data or index cells. A freshly
    /// created store is empty, and restore refuses a non-empty target.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.first_cell(&CellKey::data_family())?.is_none()
            && self.first_cell(&CellKey::index_family())?.is_none())
    }

    /// Visit every data-family cell in encoded order — the canonical `(key, value)`
    /// stream a backup carries. Index-family cells are derived from data and are
    /// rebuilt on restore, so a backup carries data only. Cells page internally so
    /// the whole store streams in bounded chunks; wrap the call in a
    /// [`read_snapshot`] to read one coherent version.
    ///
    /// [`read_snapshot`]: TreeStore::read_snapshot
    pub fn visit_backup_cells(
        &self,
        mut visit: impl FnMut(&[u8], &[u8]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        self.visit_family(CellKey::data_family().as_bytes(), &mut visit)
    }

    /// Replay one backup cell, validating that its key addresses a data-family
    /// cell. A backup carries data only — index cells are derived and rebuilt on
    /// restore — so an index or meta key is a malformed backup, not a cell to
    /// write. Restore writes cells inside one transaction.
    pub fn restore_cell(&self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.with_cell(|cell| cell.restore_cell(key, value))
    }

    fn first_cell(&self, prefix: &CellKey) -> Result<Option<Vec<u8>>, StoreError> {
        let page = self.with_cell(|cell| cell.scan_cells(prefix.as_bytes(), 1))?;
        Ok(page.entries.into_iter().next().map(|(key, _)| key))
    }

    fn visit_family(
        &self,
        prefix: &[u8],
        visit: &mut impl FnMut(&[u8], &[u8]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let mut page = self.with_cell(|cell| cell.scan_cells(prefix, BACKUP_SCAN_PAGE))?;
        loop {
            for (key, value) in &page.entries {
                visit(key, value)?;
            }
            if !page.truncated {
                return Ok(());
            }
            let resume = page
                .entries
                .last()
                .map(|(key, _)| key.clone())
                .expect("a truncated page has a last entry");
            page =
                self.with_cell(|cell| cell.scan_cells_after(prefix, &resume, BACKUP_SCAN_PAGE))?;
        }
    }

    fn with_cell<R>(
        &self,
        f: impl for<'b> FnOnce(&mut TreeCellStore<'b, dyn Backend>) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        let mut backend = self.backend.borrow_mut();
        let mut cell = TreeCellStore::new(&mut **backend);
        f(&mut cell)
    }
}

/// A pinned read snapshot over a [`TreeStore`]. While it is held, every read and
/// scan observes one consistent version of saved data; dropping it resumes
/// reading the latest committed data.
#[must_use = "a read snapshot is released as soon as it is dropped"]
pub struct ReadSnapshot<'a> {
    store: &'a TreeStore,
}

impl Drop for ReadSnapshot<'_> {
    fn drop(&mut self) {
        self.store.backend.borrow_mut().end_snapshot();
    }
}

impl<'a, B: Backend + ?Sized> TreeCellStore<'a, B> {
    fn new(backend: &'a mut B) -> Self {
        Self { backend }
    }

    fn scan_cells(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        self.backend.scan(prefix, limit)
    }

    fn scan_cells_after(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        self.backend.scan_after(prefix, cursor, limit)
    }

    fn restore_cell(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        if !is_data_cell_key(key) {
            return Err(StoreError::Corruption {
                message: "backup cell key is not a data cell".into(),
            });
        }
        self.backend.write(key, value)
    }

    fn write_catalog_epoch(&mut self, epoch: u64) -> Result<(), StoreError> {
        self.write_u64_meta(MetaCell::CatalogEpoch, epoch)
    }

    fn read_catalog_epoch(&self) -> Result<Option<u64>, StoreError> {
        self.read_u64_meta(MetaCell::CatalogEpoch)
    }

    fn write_layout_epoch(&mut self, epoch: u64) -> Result<(), StoreError> {
        self.write_u64_meta(MetaCell::LayoutEpoch, epoch)
    }

    fn read_layout_epoch(&self) -> Result<Option<u64>, StoreError> {
        self.read_u64_meta(MetaCell::LayoutEpoch)
    }

    fn write_engine_profile(&mut self, profile: &EngineProfile) -> Result<(), StoreError> {
        self.write_layout_epoch(profile.layout_epoch())?;
        self.backend.write(
            CellKey::meta(MetaCell::EngineProfile).as_bytes(),
            profile.digest_bytes().to_vec(),
        )
    }

    fn read_engine_profile_digest(&self) -> Result<Option<EngineProfileDigest>, StoreError> {
        self.backend
            .read(CellKey::meta(MetaCell::EngineProfile).as_bytes())?
            .map(|bytes| decode_digest(&bytes))
            .transpose()
    }

    fn write_commit_metadata(&mut self, metadata: &CommitMetadata) -> Result<(), StoreError> {
        self.backend.write(
            CellKey::meta(MetaCell::Commit).as_bytes(),
            encode_commit_metadata(metadata)?,
        )
    }

    fn read_commit_metadata(&self) -> Result<Option<CommitMetadata>, StoreError> {
        self.backend
            .read(CellKey::meta(MetaCell::Commit).as_bytes())?
            .map(|bytes| decode_commit_metadata(&bytes))
            .transpose()
    }

    fn write_node(&mut self, store: &CatalogId, identity: &[SavedKey]) -> Result<(), StoreError> {
        self.backend.write(
            CellKey::node(store, identity).as_bytes(),
            NODE_MARKER.to_vec(),
        )
    }

    fn node_exists(&self, store: &CatalogId, identity: &[SavedKey]) -> Result<bool, StoreError> {
        match self
            .backend
            .read(CellKey::node(store, identity).as_bytes())?
        {
            Some(value) if value == NODE_MARKER => Ok(true),
            Some(value) => Err(corrupt_cell(&value)),
            None => Ok(false),
        }
    }

    fn write_leaf(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.backend
            .write(CellKey::leaf(store, identity, member).as_bytes(), value)
    }

    fn read_leaf(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.backend
            .read(CellKey::leaf(store, identity, member).as_bytes())
    }

    fn delete_leaf(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::leaf(store, identity, member).as_bytes())
    }

    fn write_sequence_position(
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

    fn read_sequence_position(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.backend
            .read(CellKey::sequence(store, identity, member, position).as_bytes())
    }

    fn delete_sequence_position(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::sequence(store, identity, member, position).as_bytes())
    }

    fn write_data_value(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.backend.write(
            CellKey::data_path_value(store, identity, path).as_bytes(),
            value,
        )
    }

    fn read_data_value(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.backend
            .read(CellKey::data_path_value(store, identity, path).as_bytes())
    }

    fn delete_data_subtree(
        &mut self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::data_path_prefix(store, identity, path).as_bytes())
    }

    fn data_subtree_exists(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<bool, StoreError> {
        if self.read_data_value(store, identity, path)?.is_some() {
            return Ok(true);
        }
        let prefix = CellKey::data_path_prefix(store, identity, path);
        Ok(!self.backend.scan(prefix.as_bytes(), 1)?.entries.is_empty())
    }

    fn data_child_keys(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Vec<SavedKey>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.child_keys(prefix.as_bytes(), decode_data_child)
    }

    fn data_child_keys_rev(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Vec<SavedKey>, StoreError> {
        let mut keys = self.data_child_keys(store, identity, path)?;
        keys.reverse();
        Ok(keys)
    }

    fn data_next_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        let mut cursor_path = path.to_vec();
        cursor_path.push(DataPathSegment::Key(after.clone()));
        let cursor = CellKey::data_path_prefix(store, identity, &cursor_path);
        self.next_child_after_cursor(
            prefix.as_bytes(),
            cursor.as_bytes(),
            after,
            decode_data_child,
        )
    }

    fn data_prev_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.prev_child_before(prefix.as_bytes(), before, decode_data_child)
    }

    fn data_first_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.first_child(prefix.as_bytes(), decode_data_child)
    }

    fn data_last_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.last_child(prefix.as_bytes(), decode_data_child)
    }

    fn data_child_count(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<usize, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.child_count(prefix.as_bytes(), decode_data_child)
    }

    fn record_child_keys(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Vec<SavedKey>, StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        self.child_keys(prefix.as_bytes(), decode_record_child)
    }

    fn record_child_count(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<usize, StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        self.child_count(prefix.as_bytes(), decode_record_child)
    }

    fn delete_record_subtree(
        &mut self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::record_prefix(store, identity_prefix).as_bytes())
    }

    fn record_child_keys_rev(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Vec<SavedKey>, StoreError> {
        let mut keys = self.record_child_keys(store, identity_prefix)?;
        keys.reverse();
        Ok(keys)
    }

    fn record_next_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        self.next_child_after(prefix.as_bytes(), after, decode_record_child)
    }

    fn record_prev_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        self.prev_child_before(prefix.as_bytes(), before, decode_record_child)
    }

    fn record_first_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        self.first_child(prefix.as_bytes(), decode_record_child)
    }

    fn record_last_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        self.last_child(prefix.as_bytes(), decode_record_child)
    }

    fn max_int_record_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<i64>, StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        self.max_int_child(prefix.as_bytes(), decode_record_child)
    }

    fn write_index_entry(
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

    fn read_index_entry(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.backend
            .read(CellKey::index(index, index_keys, identity).as_bytes())
    }

    fn delete_index_entry(
        &mut self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::index(index, index_keys, identity).as_bytes())
    }

    fn index_child_keys(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Vec<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        self.child_keys(prefix.as_bytes(), decode_index_child)
    }

    fn delete_index_subtree(
        &mut self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.backend
            .delete(CellKey::index_key_prefix(index, key_prefix).as_bytes())
    }

    fn index_child_keys_rev(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Vec<SavedKey>, StoreError> {
        let mut keys = self.index_child_keys(index, key_prefix)?;
        keys.reverse();
        Ok(keys)
    }

    fn index_next_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        self.next_child_after(prefix.as_bytes(), after, decode_index_child)
    }

    fn index_prev_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        self.prev_child_before(prefix.as_bytes(), before, decode_index_child)
    }

    fn index_first_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        self.first_child(prefix.as_bytes(), decode_index_child)
    }

    fn index_last_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        self.last_child(prefix.as_bytes(), decode_index_child)
    }

    fn max_int_data_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<i64>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.max_int_child(prefix.as_bytes(), decode_data_child)
    }

    fn scan_index_tuple(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        self.scan_index_tuple_from(index, index_keys, None, limit)
    }

    fn scan_index_tuple_after(
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

    fn child_keys(
        &self,
        prefix: &[u8],
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Vec<SavedKey>, StoreError> {
        let mut keys = Vec::new();
        self.scan_children(prefix, |child| keys.push(child), decode)?;
        Ok(keys)
    }

    fn child_count(
        &self,
        prefix: &[u8],
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<usize, StoreError> {
        let mut count = 0;
        self.scan_children(
            prefix,
            |child| {
                let _ = child;
                count += 1;
            },
            decode,
        )?;
        Ok(count)
    }

    fn scan_children(
        &self,
        prefix: &[u8],
        mut visit: impl FnMut(SavedKey),
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<(), StoreError> {
        self.scan_children_until(
            prefix,
            |child| {
                visit(child);
                std::ops::ControlFlow::Continue(())
            },
            decode,
        )
    }

    fn scan_children_until(
        &self,
        prefix: &[u8],
        mut visit: impl FnMut(SavedKey) -> std::ops::ControlFlow<()>,
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<(), StoreError> {
        let mut cursor: Option<Vec<u8>> = None;
        let mut last_child: Option<SavedKey> = None;
        loop {
            let page = match cursor.as_ref() {
                Some(cursor) => self
                    .backend
                    .scan_after(prefix, cursor, CHILD_SCAN_PAGE_LIMIT)?,
                None => self.backend.scan(prefix, CHILD_SCAN_PAGE_LIMIT)?,
            };
            cursor = page.entries.last().map(|(key, _)| key.clone());
            for (key, _) in page.entries {
                let rest = key.get(prefix.len()..).unwrap_or_default();
                if let Some(child) = decode(rest)? {
                    if last_child.as_ref() == Some(&child) {
                        continue;
                    }
                    last_child = Some(child.clone());
                    if visit(child).is_break() {
                        return Ok(());
                    }
                }
            }
            if !page.truncated {
                break;
            }
            if cursor.is_none() {
                return Err(StoreError::InvalidCursor {
                    message: "child scan page was truncated without a cursor".into(),
                });
            }
        }
        Ok(())
    }

    fn next_child_after(
        &self,
        prefix: &[u8],
        after: &SavedKey,
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut seen_anchor = false;
        let mut result = None;
        self.scan_children_until(
            prefix,
            |child| {
                if seen_anchor {
                    result = Some(child);
                    return std::ops::ControlFlow::Break(());
                }
                seen_anchor = &child == after;
                std::ops::ControlFlow::Continue(())
            },
            decode,
        )?;
        Ok(result)
    }

    fn next_child_after_cursor(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        after: &SavedKey,
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut cursor = cursor.to_vec();
        loop {
            let page = self
                .backend
                .scan_after(prefix, &cursor, CHILD_SCAN_PAGE_LIMIT)?;
            let next_cursor = page.entries.last().map(|(key, _)| key.clone());
            for (key, _) in page.entries {
                let rest = key.get(prefix.len()..).unwrap_or_default();
                let Some(child) = decode(rest)? else {
                    continue;
                };
                if &child == after {
                    continue;
                }
                return Ok(Some(child));
            }
            if !page.truncated {
                break;
            }
            cursor = next_cursor.ok_or_else(|| StoreError::InvalidCursor {
                message: "child scan page was truncated without a cursor".into(),
            })?;
        }
        Ok(None)
    }

    fn prev_child_before(
        &self,
        prefix: &[u8],
        before: &SavedKey,
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut previous = None;
        let mut result = None;
        self.scan_children_until(
            prefix,
            |child| {
                if &child == before {
                    result = previous.take();
                    return std::ops::ControlFlow::Break(());
                }
                previous = Some(child);
                std::ops::ControlFlow::Continue(())
            },
            decode,
        )?;
        Ok(result)
    }

    fn first_child(
        &self,
        prefix: &[u8],
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut result = None;
        self.scan_children_until(
            prefix,
            |child| {
                result = Some(child);
                std::ops::ControlFlow::Break(())
            },
            decode,
        )?;
        Ok(result)
    }

    fn last_child(
        &self,
        prefix: &[u8],
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut result = None;
        self.scan_children(prefix, |child| result = Some(child), decode)?;
        Ok(result)
    }

    fn max_int_child(
        &self,
        prefix: &[u8],
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<i64>, StoreError> {
        let mut result = None;
        self.scan_children(
            prefix,
            |child| {
                if let SavedKey::Int(value) = child {
                    result = Some(result.map_or(value, |max: i64| max.max(value)));
                }
            },
            decode,
        )?;
        Ok(result)
    }
}

fn decode_record_child(bytes: &[u8]) -> Result<Option<SavedKey>, StoreError> {
    if bytes.is_empty() || bytes.first().copied() == Some(0) {
        return Ok(None);
    }
    decode_key_value(bytes)
        .map(|(key, _)| Some(key))
        .ok_or_else(|| corrupt_cell(bytes))
}

fn decode_data_child(bytes: &[u8]) -> Result<Option<SavedKey>, StoreError> {
    crate::cell::decode_data_child_key(bytes).map_err(|_| corrupt_cell(bytes))
}

fn decode_index_child(bytes: &[u8]) -> Result<Option<SavedKey>, StoreError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.first().copied() == Some(0) {
        let Some(rest) = bytes.get(1..) else {
            return Ok(None);
        };
        if rest.is_empty() {
            return Ok(None);
        }
        return decode_key_value(rest)
            .map(|(key, _)| Some(key))
            .ok_or_else(|| corrupt_cell(bytes));
    }
    decode_key_value(bytes)
        .map(|(key, _)| Some(key))
        .ok_or_else(|| corrupt_cell(bytes))
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
    put_bytes(metadata.source_digest.as_bytes(), &mut bytes)?;
    put_bytes(&metadata.engine_profile_digest, &mut bytes)?;
    put_catalog_ids(&metadata.changed_root_catalog_ids, &mut bytes)?;
    put_catalog_ids(&metadata.changed_index_catalog_ids, &mut bytes)?;
    put_bytes(metadata.activation_evolution_digest.as_bytes(), &mut bytes)?;
    put_bytes(
        metadata
            .activation_proposal_catalog_digest
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
        &mut bytes,
    )?;
    bytes.extend_from_slice(&metadata.activation_records_backfilled.to_be_bytes());
    bytes.extend_from_slice(&metadata.activation_indexes_rebuilt.to_be_bytes());
    bytes.extend_from_slice(&metadata.activation_records_retired.to_be_bytes());
    bytes.extend_from_slice(&metadata.activation_records_transformed.to_be_bytes());
    put_bytes(
        metadata.activation_retire_evidence_digest.as_bytes(),
        &mut bytes,
    )?;
    put_retire_counts(&metadata.activation_records_retired_by_id, &mut bytes)?;
    put_default_counts(&metadata.activation_default_records_by_id, &mut bytes)?;
    Ok(bytes)
}

fn decode_commit_metadata(bytes: &[u8]) -> Result<CommitMetadata, StoreError> {
    let mut cursor = Cursor::new(bytes);
    let commit_id = cursor.take_u64()?;
    let catalog_epoch = cursor.take_u64()?;
    let layout_epoch = cursor.take_u64()?;
    let source_digest = cursor.take_string()?;
    let engine_profile_digest = cursor.take_digest()?;
    let changed_root_catalog_ids = cursor.take_catalog_ids()?;
    let changed_index_catalog_ids = cursor.take_catalog_ids()?;
    let (
        activation_evolution_digest,
        activation_proposal_catalog_digest,
        activation_records_backfilled,
        activation_default_records_by_id,
        activation_indexes_rebuilt,
        activation_records_retired,
        activation_records_transformed,
        activation_retire_evidence_digest,
        activation_records_retired_by_id,
    ) = if cursor.is_empty() {
        (
            String::new(),
            None,
            0,
            Vec::new(),
            0,
            0,
            0,
            String::new(),
            Vec::new(),
        )
    } else {
        let evolution_digest = cursor.take_string()?;
        let proposal_digest = cursor.take_string()?;
        let activation_records_backfilled = cursor.take_u64()?;
        let activation_indexes_rebuilt = cursor.take_u64()?;
        let activation_records_retired = cursor.take_u64()?;
        let activation_records_transformed = cursor.take_u64()?;
        let activation_retire_evidence_digest = cursor.take_string()?;
        let activation_records_retired_by_id = cursor.take_retire_counts()?;
        let activation_default_records_by_id = cursor.take_default_counts()?;
        (
            evolution_digest,
            (!proposal_digest.is_empty()).then_some(proposal_digest),
            activation_records_backfilled,
            activation_default_records_by_id,
            activation_indexes_rebuilt,
            activation_records_retired,
            activation_records_transformed,
            activation_retire_evidence_digest,
            activation_records_retired_by_id,
        )
    };
    if !cursor.is_empty() {
        return Err(corrupt_cell(bytes));
    }
    Ok(CommitMetadata {
        commit_id,
        catalog_epoch,
        layout_epoch,
        source_digest,
        engine_profile_digest,
        changed_root_catalog_ids,
        changed_index_catalog_ids,
        activation_evolution_digest,
        activation_proposal_catalog_digest,
        activation_records_backfilled,
        activation_default_records_by_id,
        activation_indexes_rebuilt,
        activation_records_retired,
        activation_retire_evidence_digest,
        activation_records_retired_by_id,
        activation_records_transformed,
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

fn put_retire_counts(counts: &[(CatalogId, u64)], out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(counts.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for (id, count) in counts {
        put_catalog_id(id, out)?;
        out.extend_from_slice(&count.to_be_bytes());
    }
    Ok(())
}

fn put_default_counts(
    counts: &[ActivationDefaultRecordCount],
    out: &mut Vec<u8>,
) -> Result<(), StoreError> {
    let len = u32::try_from(counts.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell metadata length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    for count in counts {
        put_catalog_id(&count.catalog_id, out)?;
        out.extend_from_slice(&count.records_backfilled.to_be_bytes());
        out.extend_from_slice(&count.target_records.to_be_bytes());
        put_bytes(count.evidence_digest.as_bytes(), out)?;
    }
    Ok(())
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

    fn take_string(&mut self) -> Result<String, StoreError> {
        let raw = self.take_bytes()?;
        std::str::from_utf8(raw)
            .map(str::to_string)
            .map_err(|_| corrupt_cell(raw))
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

    fn take_retire_counts(&mut self) -> Result<Vec<(CatalogId, u64)>, StoreError> {
        let len = self.take_u32()? as usize;
        if len > self.bytes.len() / (MIN_ENCODED_CATALOG_ID_BYTES + 8) {
            return Err(corrupt_cell(self.bytes));
        }
        let mut counts = Vec::new();
        for _ in 0..len {
            counts.push((self.take_catalog_id()?, self.take_u64()?));
        }
        Ok(counts)
    }

    fn take_default_counts(&mut self) -> Result<Vec<ActivationDefaultRecordCount>, StoreError> {
        let len = self.take_u32()? as usize;
        if len > self.bytes.len() / (MIN_ENCODED_CATALOG_ID_BYTES + 16 + MIN_LENGTH_PREFIX_BYTES) {
            return Err(corrupt_cell(self.bytes));
        }
        let mut counts = Vec::new();
        for _ in 0..len {
            counts.push(ActivationDefaultRecordCount {
                catalog_id: self.take_catalog_id()?,
                records_backfilled: self.take_u64()?,
                target_records: self.take_u64()?,
                evidence_digest: self.take_string()?,
            });
        }
        Ok(counts)
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{
        ActivationDefaultRecordCount, CellKey, CommitMetadata, DataPathSegment, NODE_MARKER,
        TreeStore, decode_commit_metadata, encode_commit_metadata, is_data_cell_key,
    };
    use crate::StoreError;
    use crate::backend::{Backend, ScanPage};
    use crate::cell::CatalogId;
    use crate::key::SavedKey;
    use crate::mem::MemStore;

    /// A backend that delegates to an in-memory store but fails the Nth write, after
    /// the earlier writes have already mutated the transaction buffer. It models a
    /// storage fault that strikes part-way through a staged plan, so a test can prove
    /// the transaction bracket rolls the whole plan back rather than leaving a partial
    /// write behind.
    struct FailOnNthWrite {
        inner: MemStore,
        writes_until_fault: Cell<usize>,
    }

    impl FailOnNthWrite {
        fn new(writes_before_fault: usize) -> Self {
            Self {
                inner: MemStore::default(),
                writes_until_fault: Cell::new(writes_before_fault),
            }
        }
    }

    impl Backend for FailOnNthWrite {
        fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
            self.inner.read(key)
        }

        fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
            let remaining = self.writes_until_fault.get();
            if remaining == 0 {
                return Err(StoreError::Corruption {
                    message: "injected write fault".into(),
                });
            }
            self.writes_until_fault.set(remaining - 1);
            self.inner.write(key, value)
        }

        fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError> {
            self.inner.delete(prefix)
        }

        fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
            self.inner.scan(prefix, limit)
        }

        fn scan_after(
            &self,
            prefix: &[u8],
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.inner.scan_after(prefix, cursor, limit)
        }

        fn begin(&mut self) -> Result<(), StoreError> {
            self.inner.begin()
        }

        fn commit(&mut self) -> Result<(), StoreError> {
            self.inner.commit()
        }

        fn rollback(&mut self) -> Result<(), StoreError> {
            self.inner.rollback()
        }

        fn begin_snapshot(&mut self) -> Result<(), StoreError> {
            self.inner.begin_snapshot()
        }

        fn end_snapshot(&mut self) {
            self.inner.end_snapshot();
        }
    }

    /// A storage fault part-way through a staged transaction rolls the whole bracket
    /// back: a write that succeeded before the fault must not survive, and no metadata
    /// stamp may land. This is the atomic guarantee evolution apply relies on when it
    /// commits backfills and the catalog-epoch stamp together; a read-only store fails
    /// at `begin`, so only a mid-plan fault proves the rollback covers committed writes.
    #[test]
    fn a_mid_transaction_write_fault_rolls_the_whole_bracket_back() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let path = [DataPathSegment::Member(member)];
        // The fault strikes on the second write, so the first data write lands in the
        // buffer before the bracket aborts.
        let store = TreeStore::from_backend(Box::new(FailOnNthWrite::new(1)));

        let before = store.read_catalog_epoch().expect("read epoch");
        assert_eq!(before, None, "the store starts unstamped");

        store.begin().expect("begin");
        store
            .write_data_value(&store_id, &[SavedKey::Int(1)], &path, b"first".to_vec())
            .expect("first write lands in the buffer");
        let second =
            store.write_data_value(&store_id, &[SavedKey::Int(2)], &path, b"second".to_vec());
        assert!(matches!(second, Err(StoreError::Corruption { .. })));
        // A real bracket rolls back on the staged error rather than committing.
        store.rollback().expect("rollback");

        assert_eq!(
            store
                .read_data_value(&store_id, &[SavedKey::Int(1)], &path)
                .expect("read"),
            None,
            "the pre-fault write must not survive the rollback"
        );
        assert_eq!(
            store
                .read_data_value(&store_id, &[SavedKey::Int(2)], &path)
                .expect("read"),
            None,
            "the faulted write left nothing behind"
        );
        assert_eq!(
            store.read_catalog_epoch().expect("read epoch"),
            None,
            "no metadata stamp may land when the plan aborts"
        );
    }

    #[test]
    fn node_exists_reports_a_malformed_node_marker_as_corruption() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let mut backend = MemStore::default();
        Backend::write(
            &mut backend,
            CellKey::node(&store_id, &[SavedKey::Int(1)]).as_bytes(),
            b"not-a-node-marker".to_vec(),
        )
        .expect("seed malformed marker");
        let store = TreeStore::from_backend(Box::new(backend));

        assert_corruption(store.node_exists(&store_id, &[SavedKey::Int(1)]));
    }

    #[test]
    fn record_child_scans_report_malformed_child_keys_as_corruption() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let mut backend = MemStore::default();
        let mut key = CellKey::record_prefix(&store_id, &[]).into_bytes();
        key.push(0xff);
        Backend::write(&mut backend, &key, NODE_MARKER.to_vec()).expect("seed malformed child");
        let store = TreeStore::from_backend(Box::new(backend));

        assert_corruption(store.record_first_child(&store_id, &[]));
    }

    #[test]
    fn data_child_scans_report_malformed_key_segments_as_corruption() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let identity = [SavedKey::Int(1)];
        let path = [DataPathSegment::Member(member)];
        let mut backend = MemStore::default();
        let mut key = CellKey::data_path_prefix(&store_id, &identity, &path).into_bytes();
        key.push(0x40);
        key.push(0xff);
        Backend::write(&mut backend, &key, b"value".to_vec()).expect("seed malformed child");
        let store = TreeStore::from_backend(Box::new(backend));

        assert_corruption(store.data_first_child(&store_id, &identity, &path));
    }

    /// Commit metadata round-trips through its encoding, including the schema-bearing
    /// source digest the activation fence binds. A different source digest produces a
    /// distinct round-trip, so the fence can detect a structurally different schema at
    /// the same catalog epoch.
    #[test]
    fn commit_metadata_round_trips_with_source_digest() {
        let metadata = CommitMetadata {
            commit_id: 7,
            catalog_epoch: 3,
            layout_epoch: 0,
            source_digest:
                "sha256:00000000000000000000000000000000000000000000000000000000deadbeef"
                    .to_string(),
            engine_profile_digest: [1, 2, 3, 4, 5, 6, 7, 8],
            changed_root_catalog_ids: vec![catalog("cat_00000000000000000000000000000001")],
            changed_index_catalog_ids: vec![catalog("cat_00000000000000000000000000000002")],
            activation_evolution_digest:
                "sha256:00000000000000000000000000000000000000000000000000000000feedface"
                    .to_string(),
            activation_proposal_catalog_digest: Some(
                "sha256:00000000000000000000000000000000000000000000000000000000c001d00d"
                    .to_string(),
            ),
            activation_records_backfilled: 2,
            activation_default_records_by_id: vec![ActivationDefaultRecordCount {
                catalog_id: catalog("cat_00000000000000000000000000000005"),
                records_backfilled: 2,
                target_records: 3,
                evidence_digest: "fnv1a64:0000000000000005".to_string(),
            }],
            activation_indexes_rebuilt: 1,
            activation_records_retired: 4,
            activation_retire_evidence_digest: "fnv1a64:0000000000000006".to_string(),
            activation_records_retired_by_id: vec![
                (catalog("cat_00000000000000000000000000000003"), 3),
                (catalog("cat_00000000000000000000000000000004"), 1),
            ],
            activation_records_transformed: 3,
        };

        let store = TreeStore::memory();
        store
            .write_commit_metadata(&metadata)
            .expect("write metadata");
        let read = store
            .read_commit_metadata()
            .expect("read metadata")
            .expect("metadata present");
        assert_eq!(read, metadata);

        let other = CommitMetadata {
            source_digest:
                "sha256:00000000000000000000000000000000000000000000000000000000cafef00d"
                    .to_string(),
            ..metadata.clone()
        };
        assert_ne!(other, metadata, "a distinct source digest is not equal");
    }

    #[test]
    fn commit_metadata_rejects_truncated_activation_receipt_lists() {
        let metadata = CommitMetadata {
            commit_id: 7,
            catalog_epoch: 3,
            layout_epoch: 0,
            source_digest: "fnv1a64:00000000deadbeef".to_string(),
            engine_profile_digest: [1, 2, 3, 4, 5, 6, 7, 8],
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
            activation_evolution_digest: "fnv1a64:00000000feedface".to_string(),
            activation_proposal_catalog_digest: Some("fnv1a64:00000000c001d00d".to_string()),
            activation_records_backfilled: 0,
            activation_default_records_by_id: Vec::new(),
            activation_indexes_rebuilt: 0,
            activation_records_retired: 0,
            activation_retire_evidence_digest: "fnv1a64:0000000000000000".to_string(),
            activation_records_retired_by_id: Vec::new(),
            activation_records_transformed: 0,
        };
        let encoded = encode_commit_metadata(&metadata).expect("encode metadata");

        assert_corruption(decode_commit_metadata(&encoded[..encoded.len() - 8]));
        assert_corruption(decode_commit_metadata(&encoded[..encoded.len() - 4]));
    }

    /// `for_each_record` visits exactly the seeded single-key record identities, in key
    /// order regardless of insertion order. Evolution apply walks every record through
    /// this primitive, so it must reach each one once and invent none.
    #[test]
    fn for_each_record_visits_every_single_key_identity() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let store = TreeStore::memory();
        for id in [3, 1, 2] {
            store
                .write_node(&store_id, &[SavedKey::Int(id)])
                .expect("seed record");
        }

        let mut visited = Vec::new();
        store
            .for_each_record(&store_id, 1, &mut |identity| {
                visited.push(identity.to_vec());
                Ok(())
            })
            .expect("traverse records");

        assert_eq!(
            visited,
            vec![
                vec![SavedKey::Int(1)],
                vec![SavedKey::Int(2)],
                vec![SavedKey::Int(3)],
            ]
        );
    }

    /// A composite key descends one level per identity key and yields the full tuple for
    /// each leaf record, never a partial prefix. The seed shares a first-level key across
    /// two records to prove the descent enumerates every second-level child under it.
    #[test]
    fn for_each_record_visits_every_composite_key_identity() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let store = TreeStore::memory();
        let identities = [
            vec![SavedKey::Str("fiction".into()), SavedKey::Int(2)],
            vec![SavedKey::Str("fiction".into()), SavedKey::Int(1)],
            vec![SavedKey::Str("history".into()), SavedKey::Int(5)],
        ];
        for identity in &identities {
            store.write_node(&store_id, identity).expect("seed record");
        }

        let mut visited = Vec::new();
        store
            .for_each_record(&store_id, 2, &mut |identity| {
                visited.push(identity.to_vec());
                Ok(())
            })
            .expect("traverse records");

        assert_eq!(
            visited,
            vec![
                vec![SavedKey::Str("fiction".into()), SavedKey::Int(1)],
                vec![SavedKey::Str("fiction".into()), SavedKey::Int(2)],
                vec![SavedKey::Str("history".into()), SavedKey::Int(5)],
            ]
        );
    }

    fn collect_backup_cells(store: &TreeStore) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut cells = Vec::new();
        store
            .visit_backup_cells(|key, value| {
                cells.push((key.to_vec(), value.to_vec()));
                Ok(())
            })
            .expect("visit backup cells");
        cells
    }

    #[test]
    fn is_empty_distinguishes_a_fresh_store_from_a_populated_one() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let store = TreeStore::memory();
        assert!(store.is_empty().expect("is_empty"));

        store
            .write_data_value(
                &store_id,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(member)],
                b"v".to_vec(),
            )
            .expect("write data");
        assert!(!store.is_empty().expect("is_empty"));
    }

    /// A backup carries every data-family cell and nothing else, and replaying that
    /// stream into a fresh store reproduces it byte-for-byte. Index cells are derived
    /// and rebuilt on restore, so they stay out of the stream; meta cells stay out
    /// because restore restamps them from the manifest.
    #[test]
    fn backup_cells_round_trip_and_exclude_index_and_meta() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let title = catalog("cat_00000000000000000000000000000002");
        let index = catalog("cat_00000000000000000000000000000003");
        let path = [DataPathSegment::Member(title.clone())];

        let source = TreeStore::memory();
        source
            .write_node(&store_id, &[SavedKey::Int(1)])
            .expect("write node");
        source
            .write_data_value(&store_id, &[SavedKey::Int(1)], &path, b"Mort".to_vec())
            .expect("write leaf");
        // An index cell the backup stream must not carry: it is derived from the data
        // above and is rebuilt, not replayed, on restore.
        source
            .write_index_entry(
                &index,
                &[SavedKey::Str("Mort".into())],
                &[SavedKey::Int(1)],
                Vec::new(),
            )
            .expect("write index");
        // A meta stamp that the backup stream must not carry.
        source.write_catalog_epoch(4).expect("stamp catalog epoch");

        let cells = collect_backup_cells(&source);
        assert!(!cells.is_empty(), "the populated store has backup cells");
        assert!(
            cells
                .iter()
                .all(|(key, _)| is_data_cell_key(key) && !is_index_family(key)),
            "the backup stream carries only data-family cells: {cells:?}"
        );

        let restored = TreeStore::memory();
        assert!(restored.is_empty().expect("fresh store is empty"));
        for (key, value) in &cells {
            restored
                .restore_cell(key, value.clone())
                .expect("restore cell");
        }
        // `is_empty` checks the index family too, and the data cells round-tripped.
        assert!(!restored.is_empty().expect("restored store is populated"));

        assert_eq!(collect_backup_cells(&restored), cells, "stream round-trips");
        assert_eq!(
            restored
                .read_data_value(&store_id, &[SavedKey::Int(1)], &path)
                .expect("read restored leaf"),
            Some(b"Mort".to_vec()),
        );
        // The catalog-epoch meta cell was never part of the stream.
        assert_eq!(restored.read_catalog_epoch().expect("read epoch"), None);
    }

    fn is_index_family(key: &[u8]) -> bool {
        key.starts_with(CellKey::index_family().as_bytes())
    }

    #[test]
    fn restore_cell_rejects_a_meta_key() {
        let store = TreeStore::memory();
        // A meta-family key (catalog-epoch cell) is not a restorable backup cell.
        let meta_key = CellKey::meta(super::MetaCell::CatalogEpoch);
        assert_corruption(store.restore_cell(meta_key.as_bytes(), b"4".to_vec()));
    }

    #[test]
    fn restore_cell_rejects_an_index_key() {
        let store = TreeStore::memory();
        // An index-family cell is derived and rebuilt on restore; a backup never
        // carries one, so replaying an index key is a malformed backup.
        let mut index_key = CellKey::index_family().as_bytes().to_vec();
        index_key.extend_from_slice(b"entry");
        assert_corruption(store.restore_cell(&index_key, b"1".to_vec()));
    }

    /// A pinned read snapshot keeps a backup traversal coherent: cells written after
    /// the snapshot opens are invisible until it is released.
    #[test]
    fn read_snapshot_keeps_a_backup_traversal_coherent() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let path = [DataPathSegment::Member(member)];
        let store = TreeStore::memory();
        store
            .write_data_value(&store_id, &[SavedKey::Int(1)], &path, b"first".to_vec())
            .expect("write first");

        let before = {
            let _snapshot = store.read_snapshot().expect("snapshot");
            store
                .write_data_value(&store_id, &[SavedKey::Int(2)], &path, b"second".to_vec())
                .expect("write second");
            collect_backup_cells(&store)
        };
        assert_eq!(before.len(), 1, "snapshot hid the concurrent write");

        // After the snapshot drops, the traversal sees both records.
        assert_eq!(collect_backup_cells(&store).len(), 2);
    }

    fn catalog(raw: &str) -> CatalogId {
        CatalogId::new(raw.to_string()).expect("valid catalog id")
    }

    fn assert_corruption<T>(result: Result<T, StoreError>) {
        assert!(matches!(result, Err(StoreError::Corruption { .. })));
    }
}
