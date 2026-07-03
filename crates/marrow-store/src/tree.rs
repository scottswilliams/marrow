//! Typed tree-cell store facade over the private ordered-byte engine.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

use crate::backend::{Backend, ScanPage, StoreError, ValuePrefix};
use crate::cell::{
    CatalogId, CellKey, IndexCellKey, MetaCell, NODE_MARKER, SequencePosition,
    decode_data_cell_key, decode_data_child_key, decode_data_family_store, decode_index_cell_key,
    decode_index_child_key, decode_index_entry_key, decode_index_identity, prefix_successor,
};
use crate::codec::BoundedReader;
use crate::digest::RootDigest;
use crate::key::{
    INDEX_MARKER, KEY_INT_EXCLUSIVE_END, SavedKey, encode_identity_payload, encode_key_value,
};
use crate::metadata::{
    CommitRecord, decode_commit_metadata, decode_commit_record, decode_store_uid,
    encode_commit_metadata, encode_commit_record, encode_store_uid,
};

pub use crate::backup::{
    TREE_BACKUP_ARCHIVE_FORMAT_VERSION, TREE_BACKUP_ARCHIVE_MAGIC,
    TREE_BACKUP_MAX_CATALOG_SECTION_BYTES, TREE_BACKUP_MAX_CELL_BYTES,
    TREE_BACKUP_MAX_MANIFEST_BYTES, TreeBackupArchiveReadError, TreeBackupCell, TreeBackupCellBuf,
    TreeBackupCellFrameError, TreeBackupCellReadError, fold_checksum_bytes,
    read_tree_backup_archive_chunk, read_tree_backup_archive_header,
    write_tree_backup_archive_chunk, write_tree_backup_archive_header,
};
pub use crate::cell::DataPathSegment;
pub use crate::metadata::{CommitMetadata, EngineProfile, EngineProfileDigest, StoreUid};

/// How many cells a backup traversal pages at a time, so the whole store is
/// streamed in bounded chunks rather than materialized at once.
const BACKUP_SCAN_PAGE: usize = 1024;
const TREE_VALUE_VERSION_V0: u8 = 0;
const CHILD_SCAN_PAGE_LIMIT: usize = 128;
const INDEX_IDENTITY_SCAN_PAGE: usize = 1024;
type IndexEntryVisitor<'a> =
    dyn FnMut(&[SavedKey], &[SavedKey], &[u8]) -> Result<(), StoreError> + 'a;
type RawCellVisitor<'a> = dyn FnMut(&[u8], &[u8]) -> Result<(), StoreError> + 'a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub index_keys: Vec<SavedKey>,
    pub identity: Vec<SavedKey>,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataValuePrefix {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// Opaque cursor for resuming an index scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexCursor {
    prefix: Vec<u8>,
    last_key: Vec<u8>,
    scope: IndexCursorScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IndexCursorScope {
    Exact,
    Range {
        lower: Vec<u8>,
        upper: Option<Vec<u8>>,
    },
}

/// One bounded page from an exact index tuple scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPage {
    pub entries: Vec<IndexEntry>,
    pub cursor: Option<IndexCursor>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRangeBounds {
    pub lower: Option<SavedKey>,
    pub lower_inclusive: bool,
    pub upper: Option<SavedKey>,
    pub upper_inclusive: bool,
}

struct NormalizedIndexRange {
    lower: Vec<u8>,
    upper: Option<Vec<u8>>,
}

impl NormalizedIndexRange {
    fn cursor_scope(&self) -> IndexCursorScope {
        IndexCursorScope::Range {
            lower: self.lower.clone(),
            upper: self.upper.clone(),
        }
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

/// An owning tree-cell store handle for runtime and tooling callers.
pub struct TreeStore {
    backend: RefCell<Box<dyn Backend>>,
    /// The structural-digest change each touched root has accumulated since the open
    /// transaction began: every data cell written adds its hash and every cell overwritten
    /// or deleted subtracts the prior one, in constant time per cell. At the outermost
    /// commit each delta is folded into the durable per-root digest the same transaction
    /// persists, so the anchor always reflects exactly the cells the commit holds. Cleared
    /// whenever a transaction closes.
    digest_deltas: RefCell<HashMap<CatalogId, RootDigest>>,
    /// Roots whose live cells this handle has already reconciled against the sealed commit
    /// record. A run or serve enumerates a root by walking it, so the first walk verifies the
    /// root's digest — the subtree-granularity "verify what you touch" check — and later walks in
    /// the same session skip it, since backend damage cannot appear mid-session on an open handle.
    verified_roots: RefCell<HashSet<CatalogId>>,
}

#[derive(Clone, Copy)]
enum RecordChildScan {
    DescendantNode,
    ExactNode,
}

#[derive(Clone, Copy)]
enum ScanStep {
    Next,
    Prev,
}

/// The child scan to use when descending toward records of total `arity`, or `None`
/// when the prefix already reaches the record level and has no further children.
fn arity_child_scan(identity_prefix: &[SavedKey], arity: usize) -> Option<RecordChildScan> {
    if identity_prefix.len() >= arity {
        return None;
    }
    Some(if identity_prefix.len() + 1 == arity {
        RecordChildScan::ExactNode
    } else {
        RecordChildScan::DescendantNode
    })
}

/// Decide whether a decoded cell is a record child under `identity_prefix` for the
/// given scan, returning the child key one level below the prefix. A child is a node
/// marker in `store` whose identity extends the prefix; `ExactNode` requires the
/// immediate child level, `DescendantNode` accepts any deeper node.
fn record_child_of(
    decoded: &crate::cell::DataCellKey,
    value: &[u8],
    store: &CatalogId,
    identity_prefix: &[SavedKey],
    scan: RecordChildScan,
) -> Option<SavedKey> {
    if decoded.store != *store
        || !matches!(decoded.kind, crate::cell::DataCellKind::Node)
        || !decoded.identity.starts_with(identity_prefix)
        || value != NODE_MARKER
    {
        return None;
    }
    match scan {
        RecordChildScan::DescendantNode if decoded.identity.len() <= identity_prefix.len() => {
            return None;
        }
        RecordChildScan::ExactNode if decoded.identity.len() != identity_prefix.len() + 1 => {
            return None;
        }
        _ => {}
    }
    Some(decoded.identity[identity_prefix.len()].clone())
}

impl TreeStore {
    pub fn memory() -> Self {
        Self::from_backend(Box::new(crate::mem::MemStore::default()))
    }

    #[cfg(feature = "native")]
    pub(crate) fn open(path: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self::from_backend(Box::new(crate::redb::RedbStore::open(
            path,
        )?)))
    }

    #[cfg(feature = "native")]
    pub(crate) fn open_existing(path: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self::from_backend(Box::new(
            crate::redb::RedbStore::open_existing(path)?,
        )))
    }

    #[cfg(feature = "native")]
    pub(crate) fn open_read_only(path: &std::path::Path) -> Result<Self, StoreError> {
        Ok(Self::from_backend(Box::new(
            crate::redb::RedbStore::open_read_only(path)?,
        )))
    }

    fn from_backend(backend: Box<dyn Backend>) -> Self {
        Self {
            backend: RefCell::new(backend),
            digest_deltas: RefCell::new(HashMap::new()),
            verified_roots: RefCell::new(HashSet::new()),
        }
    }

    /// Walk every cell through the production scan path, so damage below the table
    /// roots — which opens cleanly because redb reads btree pages lazily — surfaces
    /// as a typed [`StoreError`] rather than on a later read. Used by `recover`,
    /// which otherwise opens and replays without traversing the tree. The walk
    /// paginates rather than materializing the whole store.
    ///
    /// Two passes prove a store is traversable the way every read command traverses it.
    ///
    /// First, every data-family cell is decoded and value-validated through the same
    /// backup-cell grammar `data integrity`'s undeclared-cell pass uses, so a flipped key
    /// or value byte that leaves the file scannable but no longer well-formed is caught
    /// here. That linear pass also records each `(store, record arity)` seen at a leaf and
    /// checks decoded keys never regress: the encoding is order-preserving, so a strictly
    /// smaller identity is the damage that stalls a descent, caught rather than looped.
    ///
    /// Second, each recorded `(store, arity)` is descended through `for_each_record`, the
    /// seek-driven navigation `data integrity` runs. A clobbered interior page can sit off
    /// the linear scan's path yet on a record descent's re-seek, panicking deep in the
    /// engine; running the descent here surfaces it as corruption instead of leaving
    /// `recover` to bless a store a later read would fault on.
    ///
    /// The derived index family is verified the same two ways, since an index-driven
    /// read navigates that family's btree just as a record read navigates the data
    /// family's. A flipped index byte that orphans entries from an index scan would
    /// otherwise let `recover` bless, and an index lookup silently under-return, a store
    /// whose data records are intact.
    pub fn verify_readable(&self) -> Result<(), StoreError> {
        let mut previous: Option<(CatalogId, Vec<SavedKey>)> = None;
        let mut record_shapes: std::collections::BTreeSet<(CatalogId, usize)> =
            std::collections::BTreeSet::new();
        self.visit_backup_cells(|cell| {
            let key = cell.data_key();
            if matches!(
                key.kind,
                crate::cell::DataCellKind::Leaf { .. }
                    | crate::cell::DataCellKind::Sequence { .. }
                    | crate::cell::DataCellKind::Value { .. }
            ) {
                record_shapes.insert((key.store.clone(), key.identity.len()));
            }
            let current = (key.store.clone(), key.identity.clone());
            if let Some(previous) = &previous
                && current < *previous
            {
                return Err(StoreError::Corruption {
                    message: "data cell keys are out of order".into(),
                });
            }
            previous = Some(current);
            Ok(())
        })?;
        for (store, arity) in record_shapes {
            self.for_each_record(&store, arity, &mut |_| Ok(()))?;
        }
        self.verify_structural_digests()?;
        self.verify_index_readable()
    }

    /// The deep re-derivation behind the sealed commit record: re-scan every committed cell and
    /// prove the live per-root digests equal the ones the record sealed.
    ///
    /// The store-open path validates the record alone. That catches a flip of any sealed field
    /// but not a localized data-page fault that drops a run of cells or rewrites a torn-but-
    /// decodable value while every traversal still reads cleanly past it: the data family is its
    /// own derivation, so any expectation drawn from the live cells shifts with them. The record's
    /// per-root digests are the independent oracle — a content sum over every cell, sealed ahead of
    /// the data and surviving a localized data-page flip. Re-deriving each root's digest from a
    /// full data-family scan and comparing it to the record catches a dropped cell, a torn value,
    /// and a moved field alike: each changes the per-cell hash that feeds the sum. A root that
    /// holds data the record's digest does not cover, or whose live digest disagrees, is backend
    /// damage, failed closed as corruption.
    ///
    /// [`validate_commit_record`] runs first, so a store no digest comparison can condemn still
    /// fails closed on a broken uid, commit stamp, or catalog snapshot. And redb positions a range
    /// scan by walking leaf pages while a point lookup navigates interior branch separators, so a
    /// flipped separator can misroute a lookup past a committed cell the range scan — and therefore
    /// the digest — still covers: `verify_data_cells_seek_reachable` reconciles the two and fails a
    /// store holding a committed cell no read can reach.
    ///
    /// `data integrity`, `recover`, and `backup` run this directly: their record and orphan passes
    /// traverse the data family but would otherwise bless a store whose cells were silently
    /// truncated or rewritten below the sealed digest.
    ///
    /// [`validate_commit_record`]: TreeStore::validate_commit_record
    pub fn verify_structural_digests(&self) -> Result<(), StoreError> {
        self.validate_commit_record()?;
        self.verify_data_cells_seek_reachable()?;
        let live = self.live_root_digests()?;
        let mut stamped = self.stamped_root_digests()?;
        for (store, live_digest) in &live {
            let stamped_digest = stamped.remove(store).unwrap_or_default();
            if *live_digest != stamped_digest {
                return Err(StoreError::Corruption {
                    message: "a root holds different data than its commit digest recorded".into(),
                });
            }
        }
        // A sealed digest with no live data is a root whose cells were dropped wholesale while
        // the record survived; only a zero digest (an empty root) is consistent.
        if stamped.values().any(|digest| !digest.is_zero()) {
            return Err(StoreError::Corruption {
                message: "a root holds different data than its commit digest recorded".into(),
            });
        }
        Ok(())
    }

    /// Re-derive each root's structural digest from a full data-family scan: the
    /// independent recomputation the stamped digest is checked against. Roots are keyed by
    /// the store id their cells encode, so a root with no data cells contributes nothing.
    fn live_root_digests(&self) -> Result<HashMap<CatalogId, RootDigest>, StoreError> {
        let mut digests: HashMap<CatalogId, RootDigest> = HashMap::new();
        self.for_each_data_cell_raw(&mut |key, value| {
            if let Some(store) = decode_data_family_store(key) {
                digests.entry(store).or_default().add_cell(key, value);
            }
            Ok(())
        })?;
        Ok(digests)
    }

    /// Every committed data cell the linear scan yields must also be reachable by the
    /// point-lookup descent typed reads use. A range scan iterates leaf pages while a point
    /// lookup navigates interior branch separators, so a flipped separator can misroute a
    /// lookup past a committed cell the range scan still yields: the cell reads absent though
    /// its bytes — and the structural digest derived from the scan — are intact. Re-reading each
    /// scanned cell through the production point read fails such a store closed rather than
    /// blessing a committed cell no `read_data_value` or record descent can reach.
    fn verify_data_cells_seek_reachable(&self) -> Result<(), StoreError> {
        self.for_each_data_cell_raw(&mut |key, _value| {
            if self.read_cell(key)?.is_none() {
                return Err(StoreError::Corruption {
                    message: "a committed data cell is unreachable by point lookup".into(),
                });
            }
            Ok(())
        })
    }

    /// The digest each root will carry once the current state is committed: the digests the
    /// sealed record holds folded with any delta accumulated by writes still staged in an open
    /// transaction. The record sorts ahead of the data, so this enumeration is unaffected by a
    /// localized data-page corruption. Folding the pending delta lets a mid-transaction verify —
    /// the schema check a restore replay runs before its commit re-seals the record — compare
    /// against the cells the same transaction already staged, rather than a record that has not
    /// caught up yet.
    fn stamped_root_digests(&self) -> Result<HashMap<CatalogId, RootDigest>, StoreError> {
        let mut digests: HashMap<CatalogId, RootDigest> = self
            .read_commit_record()?
            .map(|record| record.root_digests.into_iter().collect())
            .unwrap_or_default();
        for (store, delta) in self.digest_deltas.borrow().iter() {
            digests.entry(store.clone()).or_default().add(*delta);
        }
        Ok(digests)
    }

    /// Verify the index family the way [`verify_readable`] verifies the data family:
    /// a linear structural decode of every index cell, in encoded order, then a
    /// seek-driven re-descent of every index it observes, then a reconciliation of the
    /// bounded seek path against that linear scan. The passes traverse different btree
    /// paths, so a clobbered index node fails closed as corruption rather than silently
    /// dropping rows an index lookup would skip.
    ///
    /// The linear decode also reconciles each entry's two identity copies. A production
    /// writer stores a record's identity twice per index entry: after the `INDEX_IDENTITY`
    /// separator, and — redundantly — as the trailing keys of the ordered tuple for a
    /// non-unique index or in the cell value for a unique one. A flip diverging the stored
    /// identity from that redundant copy is always corruption, so failing it here condemns
    /// such an entry at store open rather than letting a read surface it as a typed program
    /// fault at an innocent source span.
    ///
    /// `data integrity` runs this directly, because its record and orphan passes only
    /// touch the data family and would otherwise bless an index-corrupt store.
    ///
    /// [`verify_readable`]: TreeStore::verify_readable
    pub fn verify_index_readable(&self) -> Result<(), StoreError> {
        let mut previous: Option<Vec<u8>> = None;
        let mut index_shapes: std::collections::BTreeSet<(CatalogId, usize)> =
            std::collections::BTreeSet::new();
        self.for_each_page_entry(CellKey::index_family().as_bytes(), |key, value| {
            let decoded = decode_index_cell_key(key).ok_or_else(|| corrupt_cell(key))?;
            if !index_entry_identity_is_consistent(&decoded, value) {
                return Err(StoreError::Corruption {
                    message: "index entry identity does not match its record identity".into(),
                });
            }
            index_shapes.insert((decoded.index, decoded.index_keys.len()));
            if let Some(previous) = &previous
                && key <= previous.as_slice()
            {
                return Err(StoreError::Corruption {
                    message: "index cell keys are out of order".into(),
                });
            }
            previous = Some(key.to_vec());
            Ok(ControlFlow::Continue(()))
        })?;
        for (index, arity) in index_shapes {
            self.descend_index_entries(&index, arity, &mut Vec::new())?;
        }
        self.verify_index_cells_seek_reachable()
    }

    /// Every committed index cell the linear scan yields must also be reachable by the
    /// point-lookup descent an index read navigates. A range scan iterates leaf pages while
    /// a bounded index seek navigates interior branch separators, so a flipped separator can
    /// misroute a seek past a committed entry the linear scan — and the schema-driven
    /// completeness count — still yield: the entry reads absent though its bytes are intact,
    /// and an index range read silently under-returns a contiguous subtree. Re-reading each
    /// scanned index cell through the point read fails such a store closed rather than
    /// blessing a committed index entry no seek can reach. This mirrors
    /// [`verify_data_cells_seek_reachable`] for the derived index family.
    ///
    /// [`verify_data_cells_seek_reachable`]: TreeStore::verify_data_cells_seek_reachable
    fn verify_index_cells_seek_reachable(&self) -> Result<(), StoreError> {
        self.for_each_page_entry(CellKey::index_family().as_bytes(), |key, _value| {
            if self.read_cell(key)?.is_none() {
                return Err(StoreError::Corruption {
                    message: "a committed index cell is unreachable by point lookup".into(),
                });
            }
            Ok(ControlFlow::Continue(()))
        })
    }

    /// Seek-descend every entry under one index, mirroring how an index lookup walks
    /// it: each of the `arity` index-key levels then the identity tuple are stepped
    /// through `index_first_child`/`index_next_child`, the same cursor guards a read
    /// uses. A non-advancing or malformed child fails closed rather than looping.
    fn descend_index_entries(
        &self,
        index: &CatalogId,
        arity: usize,
        key_prefix: &mut Vec<SavedKey>,
    ) -> Result<(), StoreError> {
        if key_prefix.len() == arity {
            return self.for_each_index_identity(index, key_prefix, &mut |_| Ok(()));
        }
        let mut next = self.index_first_child(index, key_prefix)?;
        while let Some(child) = next {
            key_prefix.push(child.clone());
            self.descend_index_entries(index, arity, key_prefix)?;
            key_prefix.pop();
            next = self.index_next_child(index, key_prefix, &child)?;
        }
        Ok(())
    }

    /// Step every identity stored under one full index-key tuple, the leaf level an
    /// index descent reaches. Identities order strictly, so a non-advancing step is
    /// corruption.
    fn for_each_index_identity(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let mut page = self.scan_index_tuple(index, index_keys, INDEX_IDENTITY_SCAN_PAGE)?;
        let mut previous_cursor: Option<Vec<u8>> = None;
        loop {
            for entry in &page.entries {
                visit(&entry.identity)?;
            }
            let Some(cursor) = page.cursor.clone() else {
                return Ok(());
            };
            if let Some(previous) = &previous_cursor {
                guard_page_cursor_advances(
                    &cursor.last_key,
                    previous,
                    std::cmp::Ordering::Greater,
                )?;
            }
            previous_cursor = Some(cursor.last_key.clone());
            page =
                self.scan_index_tuple_after(index, index_keys, &cursor, INDEX_IDENTITY_SCAN_PAGE)?;
        }
    }

    pub fn begin(&self) -> Result<(), StoreError> {
        self.backend.borrow_mut().begin()
    }

    pub fn commit(&self) -> Result<(), StoreError> {
        // Fold each touched root's accumulated digest delta into its durable anchor before
        // the outermost commit persists it, so the stamped digest is always written in the
        // same transaction as the cells it covers. Nested commits leave the anchor to the
        // outer bracket; a no-op commit with no open transaction has nothing to stamp.
        if self.transaction_depth() == 1 {
            self.stamp_commit_record()?;
        }
        let result = self.backend.borrow_mut().commit();
        if self.transaction_depth() == 0 {
            self.digest_deltas.borrow_mut().clear();
        }
        result
    }

    pub fn rollback(&self) -> Result<(), StoreError> {
        let result = self.backend.borrow_mut().rollback();
        self.digest_deltas.borrow_mut().clear();
        result
    }

    /// Refresh the durable sealed commit record from the transaction's staged state before it
    /// commits. Each touched root's net digest delta folds into the record's per-root digests,
    /// and the record's sealed activation binding — store uid, accepted epoch, catalog digest,
    /// and active saved roots — is re-read from the store's own cells so it always describes the
    /// cells this commit now holds. Because the whole record is re-sealed here, the store-open
    /// path validates it alone instead of rescanning every cell.
    fn stamp_commit_record(&self) -> Result<(), StoreError> {
        let mut record = self.read_commit_record()?.unwrap_or_default();
        let deltas: Vec<(CatalogId, RootDigest)> = self
            .digest_deltas
            .borrow()
            .iter()
            .map(|(store, delta)| (store.clone(), *delta))
            .collect();
        for (store, delta) in deltas {
            fold_root_digest(&mut record.root_digests, &store, delta);
        }
        record.root_digests.retain(|(_, digest)| !digest.is_zero());
        record
            .root_digests
            .sort_by(|left, right| left.0.cmp(&right.0));
        record.store_uid = self.read_store_uid()?;
        record.catalog_epoch = self
            .read_commit_metadata()?
            .map(|commit| commit.catalog_epoch);
        let snapshot = self.read_catalog_snapshot()?;
        record.catalog_digest = snapshot.as_ref().map(|snapshot| snapshot.digest.clone());
        record.active_roots = active_store_roots(snapshot.as_ref())?;
        self.write_commit_record(&record)
    }

    fn read_commit_record(&self) -> Result<Option<CommitRecord>, StoreError> {
        self.read_cell(CellKey::meta(MetaCell::CommitRecord).as_bytes())?
            .map(|bytes| decode_commit_record(&bytes))
            .transpose()
    }

    fn write_commit_record(&self, record: &CommitRecord) -> Result<(), StoreError> {
        self.write_cell(
            CellKey::meta(MetaCell::CommitRecord).as_bytes(),
            encode_commit_record(record)?,
        )
    }

    /// The store-open integrity check: validate the single sealed commit record instead of
    /// scanning every cell. Decoding the record recomputes its content seal, so a flip of any bound
    /// field — uid, epoch, catalog digest, active roots, or a per-root digest — fails closed. The
    /// record then binds the mutable data-identity cells it seals: a commit epoch, catalog digest,
    /// or active-root set that both the record and the cell carry and that disagree is backend
    /// damage the seal alone could not see (a self-consistent swap of one cell), failed closed
    /// here — an active-root swap would otherwise let a run enumerate a phantom root, an epoch or
    /// digest swap misjudge evolution state. A field the record has not sealed yet — an unstamped
    /// store's absent epoch — is lag, not corruption, so a `None` on either side skips. The store
    /// uid is sealed but not held against its cell: the uid names no data (records key on catalog
    /// ids), so a torn uid cell is a cosmetic-identity fault, not a data-soundness one, and a
    /// process killed mid-write can legitimately leave it torn. The deep O(N) re-derivation of live
    /// cells against the record's digests is `verify_structural_digests`, which `data integrity`,
    /// `recover`, and `backup` run.
    pub fn validate_commit_record(&self) -> Result<(), StoreError> {
        self.read_store_uid()?;
        let commit = self.read_commit_metadata()?;
        let snapshot = self.read_catalog_snapshot()?;
        if let (Some(commit), Some(snapshot)) = (&commit, &snapshot)
            && commit.catalog_epoch != snapshot.epoch
        {
            return Err(StoreError::Corruption {
                message:
                    "commit metadata catalog epoch disagrees with the accepted catalog snapshot"
                        .into(),
            });
        }
        // The record is stamped and re-sealed at the outermost commit, so a handle mid-write does
        // not carry it for this transaction's staged state; the stamp proves it at commit, and the
        // deep pass reconciles the staged cells against the pending digests directly.
        if self.transaction_depth() > 0 {
            return Ok(());
        }
        let Some(record) = self.read_commit_record()? else {
            if commit.is_some() {
                return Err(StoreError::Corruption {
                    message: "the sealed commit record is missing from a stamped store".into(),
                });
            }
            return Ok(());
        };
        let disagree = |field: &'static str| StoreError::Corruption {
            message: format!("the store {field} disagrees with the sealed commit record"),
        };
        if let (Some(sealed), Some(commit)) = (record.catalog_epoch, &commit)
            && sealed != commit.catalog_epoch
        {
            return Err(disagree("commit epoch"));
        }
        if let (Some(sealed), Some(snapshot)) = (&record.catalog_digest, &snapshot)
            && *sealed != snapshot.digest
        {
            return Err(disagree("catalog digest"));
        }
        if let Some(snapshot) = &snapshot
            && record.active_roots != active_store_roots(Some(snapshot))?
        {
            return Err(disagree("active saved roots"));
        }
        Ok(())
    }

    /// Reconcile one root's live cells against the digest the sealed commit record holds, once per
    /// handle: the subtree-granularity witness a run or serve owes the root it enumerates. The open
    /// path validates the record in O(1) and drops the store-wide scan, so a localized backend flip
    /// that silently drops a cell, rewrites a torn-but-decodable value, or misroutes a point lookup
    /// in a root would otherwise let an enumeration return a truncated set with no fault. Walking the
    /// root the reader is about to enumerate, folding its digest, and confirming each cell is
    /// point-reachable fails such a root closed while leaving untouched roots unscanned.
    pub fn verify_root_digest_once(&self, store: &CatalogId) -> Result<(), StoreError> {
        if self.verified_roots.borrow().contains(store) {
            return Ok(());
        }
        self.verify_root_digest(store)?;
        self.verified_roots.borrow_mut().insert(store.clone());
        Ok(())
    }

    fn verify_root_digest(&self, store: &CatalogId) -> Result<(), StoreError> {
        let mut live = RootDigest::zero();
        let prefix = CellKey::record_prefix(store, &[]);
        self.for_each_cell_under(prefix.as_bytes(), &mut |key, value| {
            if decode_data_family_store(key).as_ref() != Some(store) {
                return Ok(());
            }
            if self.read_cell(key)?.is_none() {
                return Err(StoreError::Corruption {
                    message: "a committed data cell is unreachable by point lookup".into(),
                });
            }
            live.add_cell(key, value);
            Ok(())
        })?;
        let sealed = self
            .stamped_root_digests()?
            .get(store)
            .copied()
            .unwrap_or_default();
        if live != sealed {
            return Err(StoreError::Corruption {
                message: "a root holds different data than its commit digest recorded".into(),
            });
        }
        Ok(())
    }

    pub fn transaction_depth(&self) -> usize {
        self.backend.borrow().transaction_depth()
    }

    pub fn write_commit_metadata(&self, metadata: &CommitMetadata) -> Result<(), StoreError> {
        self.write_cell(
            CellKey::meta(MetaCell::Commit).as_bytes(),
            encode_commit_metadata(metadata)?,
        )
    }

    pub fn read_commit_metadata(&self) -> Result<Option<CommitMetadata>, StoreError> {
        self.read_cell(CellKey::meta(MetaCell::Commit).as_bytes())?
            .map(|bytes| decode_commit_metadata(&bytes))
            .transpose()
    }

    pub fn write_store_uid(&self, uid: &StoreUid) -> Result<(), StoreError> {
        self.write_cell(
            CellKey::meta(MetaCell::StoreUid).as_bytes(),
            encode_store_uid(uid),
        )
    }

    pub fn read_store_uid(&self) -> Result<Option<StoreUid>, StoreError> {
        self.read_cell(CellKey::meta(MetaCell::StoreUid).as_bytes())?
            .map(|bytes| decode_store_uid(&bytes))
            .transpose()
    }

    /// The accepted catalog the store holds, or `None` when none is published. The
    /// catalog rows live in their own physical family, invisible to every data,
    /// index, and meta access, and a read fails closed if any row was tampered.
    pub fn read_catalog_snapshot(
        &self,
    ) -> Result<Option<marrow_catalog::CatalogMetadata>, StoreError> {
        crate::catalog::read_catalog_snapshot(&**self.backend.borrow())
    }

    /// Replace the accepted catalog with `snapshot`, writing its rows under the
    /// caller's active transaction. The whole prior catalog is cleared first, so the
    /// stored rows are exactly this snapshot's.
    pub fn replace_catalog_snapshot(
        &self,
        snapshot: &marrow_catalog::CatalogMetadata,
    ) -> Result<(), StoreError> {
        crate::catalog::replace_catalog_snapshot(&mut **self.backend.borrow_mut(), snapshot)
    }

    /// The digest of the accepted catalog the store holds, or `None` when none is
    /// published. Reconstructed from the stored entries, so it reflects any tamper.
    pub fn catalog_snapshot_digest(&self) -> Result<Option<String>, StoreError> {
        crate::catalog::read_catalog_snapshot_digest(&**self.backend.borrow())
    }

    pub fn write_record_presence(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.write_cell(
            CellKey::node(store, identity).as_bytes(),
            NODE_MARKER.to_vec(),
        )
    }

    pub fn write_leaf(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.write_cell(CellKey::leaf(store, identity, member).as_bytes(), value)
    }

    pub fn write_sequence_position(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        member: &CatalogId,
        position: SequencePosition,
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.write_cell(
            CellKey::sequence(store, identity, member, position).as_bytes(),
            value,
        )
    }

    pub fn write_data_value(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        value: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.write_cell(
            CellKey::data_path_value(store, identity, path).as_bytes(),
            value,
        )
    }

    pub fn write_data_node(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<(), StoreError> {
        if path.is_empty() {
            return Err(StoreError::InvalidTransaction {
                message: "data path node writes require a non-empty path".to_string(),
            });
        }
        self.write_cell(
            CellKey::data_path_prefix(store, identity, path).as_bytes(),
            NODE_MARKER.to_vec(),
        )
    }

    pub fn read_data_value(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.read_cell(CellKey::data_path_value(store, identity, path).as_bytes())
    }

    pub fn read_data_value_prefix(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        limit: usize,
    ) -> Result<Option<DataValuePrefix>, StoreError> {
        self.read_cell_prefix(
            CellKey::data_path_value(store, identity, path).as_bytes(),
            limit,
        )
    }

    pub fn delete_data_subtree(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<(), StoreError> {
        self.delete_cells(CellKey::data_path_prefix(store, identity, path).as_bytes())
    }

    pub fn data_subtree_exists(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<bool, StoreError> {
        if path.is_empty() {
            return self
                .read_cell(CellKey::node(store, identity).as_bytes())
                .map(|node| node.is_some());
        }
        if self.read_data_value(store, identity, path)?.is_some() {
            return Ok(true);
        }
        let prefix = CellKey::data_path_prefix(store, identity, path);
        Ok(!self.scan(prefix.as_bytes(), 1)?.entries.is_empty())
    }

    /// Whether an identity node holds any cell beyond its bare existence marker. The node
    /// marker sorts first under the node prefix; every field, sequence, and nested-tree
    /// cell sorts strictly after it, so a scan past the marker that finds nothing proves
    /// the node exists with zero children. Distinguishes an emptied identity from one that
    /// genuinely has children.
    pub fn data_node_has_children(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
    ) -> Result<bool, StoreError> {
        let node = CellKey::node(store, identity);
        Ok(!self
            .scan_after(node.as_bytes(), node.as_bytes(), 1)?
            .entries
            .is_empty())
    }

    pub fn data_next_child(
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
        let Some(cursor) = prefix_successor(cursor.as_bytes()) else {
            return Ok(None);
        };
        let child = self.next_child_after_cursor(prefix.as_bytes(), &cursor, decode_data_child)?;
        guard_child_advances(child, after, std::cmp::Ordering::Greater)
    }

    pub fn data_first_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.first_child(prefix.as_bytes(), decode_data_child)
    }

    pub fn data_last_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.last_child(prefix.as_bytes(), decode_data_child)
    }

    pub fn data_prev_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        let mut cursor_path = path.to_vec();
        cursor_path.push(DataPathSegment::Key(before.clone()));
        let cursor = CellKey::data_path_prefix(store, identity, &cursor_path);
        let child = self.prev_child_before(
            prefix.as_bytes(),
            cursor.as_bytes(),
            before,
            decode_data_child,
        )?;
        guard_child_advances(child, before, std::cmp::Ordering::Less)
    }

    pub fn data_child_count(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<usize, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        self.child_count(prefix.as_bytes(), decode_data_child)
    }

    pub fn max_int_data_child(
        &self,
        store: &CatalogId,
        identity: &[SavedKey],
        path: &[DataPathSegment],
    ) -> Result<Option<i64>, StoreError> {
        let prefix = CellKey::data_path_prefix(store, identity, path);
        let cursor =
            CellKey::data_path_child_tag_upper_bound(store, identity, path, KEY_INT_EXCLUSIVE_END);
        self.max_int_child(prefix.as_bytes(), cursor.as_bytes(), decode_data_child)
    }

    pub fn record_child_count(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<usize, StoreError> {
        let mut count = 0;
        self.scan_record_children_until(
            store,
            identity_prefix,
            RecordChildScan::ExactNode,
            |_| {
                count += 1;
                std::ops::ControlFlow::Continue(())
            },
        )?;
        Ok(count)
    }

    pub fn delete_record_subtree(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.delete_cells(CellKey::record_prefix(store, identity_prefix).as_bytes())
    }

    pub fn record_next_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.record_next_child_with(
            store,
            identity_prefix,
            after,
            RecordChildScan::DescendantNode,
        )
    }

    pub fn record_exact_next_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.record_next_child_with(store, identity_prefix, after, RecordChildScan::ExactNode)
    }

    fn record_next_child_with(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        after: &SavedKey,
        scan: RecordChildScan,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut result = None;
        let mut cursor_identity = identity_prefix.to_vec();
        cursor_identity.push(after.clone());
        let cursor = CellKey::record_prefix(store, &cursor_identity);
        let Some(cursor) = prefix_successor(cursor.as_bytes()) else {
            return Ok(None);
        };
        self.scan_record_children_after_cursor(store, identity_prefix, &cursor, scan, |child| {
            result = Some(child);
            std::ops::ControlFlow::Break(())
        })?;
        guard_child_advances(result, after, std::cmp::Ordering::Greater)
    }

    pub fn record_first_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        self.record_first_child_with(store, identity_prefix, RecordChildScan::DescendantNode)
    }

    pub fn record_exact_first_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        self.record_first_child_with(store, identity_prefix, RecordChildScan::ExactNode)
    }

    fn record_first_child_with(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        scan: RecordChildScan,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut result = None;
        self.scan_record_children_until(store, identity_prefix, scan, |child| {
            result = Some(child);
            std::ops::ControlFlow::Break(())
        })?;
        Ok(result)
    }

    fn record_last_child_with(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        scan: RecordChildScan,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut result = None;
        let prefix = CellKey::record_prefix(store, identity_prefix);
        let cursor =
            prefix_successor(prefix.as_bytes()).ok_or_else(|| StoreError::InvalidCursor {
                message: "record prefix has no upper bound".into(),
            })?;
        self.scan_record_children_reverse_until(store, identity_prefix, &cursor, scan, |child| {
            result = Some(child);
            std::ops::ControlFlow::Break(())
        })?;
        Ok(result)
    }

    pub fn record_prev_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        self.record_prev_child_with(
            store,
            identity_prefix,
            before,
            RecordChildScan::DescendantNode,
        )
    }

    fn record_prev_child_with(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        before: &SavedKey,
        scan: RecordChildScan,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut result = None;
        let mut cursor_identity = identity_prefix.to_vec();
        cursor_identity.push(before.clone());
        let cursor = CellKey::record_prefix(store, &cursor_identity);
        self.scan_record_children_reverse_until(
            store,
            identity_prefix,
            cursor.as_bytes(),
            scan,
            |child| {
                result = Some(child);
                std::ops::ControlFlow::Break(())
            },
        )?;
        guard_child_advances(result, before, std::cmp::Ordering::Less)
    }

    pub fn max_int_record_child(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
    ) -> Result<Option<i64>, StoreError> {
        let mut result = None;
        let cursor =
            CellKey::record_child_tag_upper_bound(store, identity_prefix, KEY_INT_EXCLUSIVE_END);
        self.scan_record_children_reverse_until(
            store,
            identity_prefix,
            cursor.as_bytes(),
            RecordChildScan::ExactNode,
            |child| {
                if let SavedKey::Int(value) = child {
                    result = Some(value);
                }
                std::ops::ControlFlow::Break(())
            },
        )?;
        Ok(result)
    }

    pub fn record_identity_exists_under(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        arity: usize,
    ) -> Result<bool, StoreError> {
        if identity_prefix.len() == arity {
            return self.data_subtree_exists(store, identity_prefix, &[]);
        }
        if identity_prefix.len() > arity {
            return Ok(false);
        }
        self.record_first_child_at_arity(store, identity_prefix, arity)
            .map(|child| child.is_some())
    }

    pub fn record_first_child_at_arity(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        arity: usize,
    ) -> Result<Option<SavedKey>, StoreError> {
        let Some(scan) = arity_child_scan(identity_prefix, arity) else {
            return Ok(None);
        };
        let seed = self.record_first_child_with(store, identity_prefix, scan)?;
        self.first_existing_child_at_arity(
            store,
            identity_prefix,
            arity,
            scan,
            seed,
            ScanStep::Next,
        )
    }

    pub fn record_next_child_at_arity(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        arity: usize,
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let Some(scan) = arity_child_scan(identity_prefix, arity) else {
            return Ok(None);
        };
        let seed = self.record_next_child_with(store, identity_prefix, after, scan)?;
        self.first_existing_child_at_arity(
            store,
            identity_prefix,
            arity,
            scan,
            seed,
            ScanStep::Next,
        )
    }

    pub fn record_last_child_at_arity(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        arity: usize,
    ) -> Result<Option<SavedKey>, StoreError> {
        let Some(scan) = arity_child_scan(identity_prefix, arity) else {
            return Ok(None);
        };
        let seed = self.record_last_child_with(store, identity_prefix, scan)?;
        self.first_existing_child_at_arity(
            store,
            identity_prefix,
            arity,
            scan,
            seed,
            ScanStep::Prev,
        )
    }

    pub fn record_prev_child_at_arity(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        arity: usize,
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let Some(scan) = arity_child_scan(identity_prefix, arity) else {
            return Ok(None);
        };
        let seed = self.record_prev_child_with(store, identity_prefix, before, scan)?;
        self.first_existing_child_at_arity(
            store,
            identity_prefix,
            arity,
            scan,
            seed,
            ScanStep::Prev,
        )
    }

    /// Walk children from `seed` in `step` order, returning the first whose subtree
    /// actually carries a record at `arity`. Intermediate node markers can outlive their
    /// records, so each candidate is confirmed before it is yielded.
    fn first_existing_child_at_arity(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        arity: usize,
        scan: RecordChildScan,
        seed: Option<SavedKey>,
        step: ScanStep,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut child = seed;
        while let Some(candidate) = child {
            let mut next_prefix = identity_prefix.to_vec();
            next_prefix.push(candidate.clone());
            if self.record_identity_exists_under(store, &next_prefix, arity)? {
                return Ok(Some(candidate));
            }
            child = match step {
                ScanStep::Next => {
                    self.record_next_child_with(store, identity_prefix, &candidate, scan)?
                }
                ScanStep::Prev => {
                    self.record_prev_child_with(store, identity_prefix, &candidate, scan)?
                }
            };
        }
        Ok(None)
    }

    /// Visit every record identity under `store_id`, descending `arity` key levels and
    /// invoking `visit` with each full identity tuple. The descent is paged, so the
    /// whole store never materializes. A singleton store has arity zero and visits the
    /// empty identity only when its root node exists.
    pub fn for_each_record(
        &self,
        store_id: &CatalogId,
        arity: usize,
        visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        if arity == 0 {
            if self.data_subtree_exists(store_id, &[], &[])? {
                visit(&[])?;
            }
            return Ok(());
        }
        let mut identity = Vec::new();
        self.descend_records(store_id, arity, &mut identity, visit)
    }

    /// Count distinct record identities under `store_id` that carry any data-family cell.
    /// The scan is bounded to this store's subtree, where cells are ordered by identity,
    /// so it keeps only the previous identity rather than materializing the record set.
    pub fn data_record_count(&self, store_id: &CatalogId) -> Result<usize, StoreError> {
        let prefix = CellKey::record_prefix(store_id, &[]);
        let mut count = 0usize;
        let mut previous_identity: Option<Vec<SavedKey>> = None;
        self.visit_family(prefix.as_bytes(), &mut |cell| {
            let data_key = cell.data_key();
            if previous_identity.as_deref() != Some(data_key.identity.as_slice()) {
                count += 1;
                previous_identity = Some(data_key.identity.clone());
            }
            Ok(ControlFlow::Continue(()))
        })?;
        Ok(count)
    }

    fn descend_records(
        &self,
        store_id: &CatalogId,
        remaining: usize,
        identity: &mut Vec<SavedKey>,
        visit: &mut dyn FnMut(&[SavedKey]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let mut next = if remaining == 1 {
            self.record_exact_first_child(store_id, identity)?
        } else {
            self.record_first_child(store_id, identity)?
        };
        while let Some(key) = next {
            identity.push(key.clone());
            if remaining == 1 {
                visit(identity)?;
            } else {
                self.descend_records(store_id, remaining - 1, identity, visit)?;
            }
            identity.pop();
            next = if remaining == 1 {
                self.record_exact_next_child(store_id, identity, &key)?
            } else {
                self.record_next_child(store_id, identity, &key)?
            };
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
        self.write_cell(
            CellKey::index(index, index_keys, identity).as_bytes(),
            value,
        )
    }

    pub fn delete_index_entry(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        identity: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.delete_cells(CellKey::index(index, index_keys, identity).as_bytes())
    }

    pub fn delete_index_subtree(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<(), StoreError> {
        self.delete_cells(CellKey::index_key_prefix(index, key_prefix).as_bytes())
    }

    pub fn index_next_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
        after: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        let cursor = self.index_child_stem(index, key_prefix, after)?;
        let Some(cursor) = prefix_successor(&cursor) else {
            return Ok(None);
        };
        let child = self.next_child_after_cursor(prefix.as_bytes(), &cursor, decode_index_child)?;
        guard_child_advances(child, after, std::cmp::Ordering::Greater)
    }

    pub fn index_first_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        self.first_child(prefix.as_bytes(), decode_index_child)
    }

    pub fn index_last_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        self.last_child(prefix.as_bytes(), decode_index_child)
    }

    pub fn index_prev_child(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
        before: &SavedKey,
    ) -> Result<Option<SavedKey>, StoreError> {
        let prefix = CellKey::index_key_prefix(index, key_prefix);
        let cursor = self.index_child_stem(index, key_prefix, before)?;
        let child =
            self.prev_child_before(prefix.as_bytes(), &cursor, before, decode_index_child)?;
        guard_child_advances(child, before, std::cmp::Ordering::Less)
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

    pub fn scan_index_range(
        &self,
        index: &CatalogId,
        exact_prefix: &[SavedKey],
        range: &IndexRangeBounds,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        self.scan_index_range_from(index, exact_prefix, range, None, limit)
    }

    pub fn scan_index_range_after(
        &self,
        index: &CatalogId,
        exact_prefix: &[SavedKey],
        range: &IndexRangeBounds,
        cursor: &IndexCursor,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        self.scan_index_range_from(index, exact_prefix, range, Some(cursor), limit)
    }

    pub fn scan_index_range_after_entry(
        &self,
        index: &CatalogId,
        exact_prefix: &[SavedKey],
        range: &IndexRangeBounds,
        after_index_keys: &[SavedKey],
        after_identity: &[SavedKey],
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        if !after_index_keys.starts_with(exact_prefix) {
            return Err(StoreError::InvalidCursor {
                message: "index entry cursor does not match bounded index range".into(),
            });
        }
        let prefix = CellKey::index_key_prefix(index, exact_prefix);
        let Some(bounds) = normalized_index_range(prefix.as_bytes(), range) else {
            return Err(StoreError::InvalidCursor {
                message: "index entry cursor does not match bounded index range".into(),
            });
        };
        let last_key = CellKey::index(index, after_index_keys, after_identity).into_bytes();
        if last_key < bounds.lower
            || bounds
                .upper
                .as_ref()
                .is_some_and(|upper| last_key >= *upper)
        {
            return Err(StoreError::InvalidCursor {
                message: "index entry cursor does not match bounded index range".into(),
            });
        }
        let cursor = IndexCursor {
            prefix: prefix.as_bytes().to_vec(),
            last_key,
            scope: bounds.cursor_scope(),
        };
        self.scan_index_range_from(index, exact_prefix, range, Some(&cursor), limit)
    }

    pub fn scan_index_range_reverse(
        &self,
        index: &CatalogId,
        exact_prefix: &[SavedKey],
        range: &IndexRangeBounds,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        let prefix = CellKey::index_key_prefix(index, exact_prefix);
        let Some(bounds) = normalized_index_range(prefix.as_bytes(), range) else {
            return Ok(empty_index_page());
        };
        let upper = bounds
            .upper
            .clone()
            .or_else(|| prefix_successor(prefix.as_bytes()))
            .ok_or_else(|| StoreError::InvalidCursor {
                message: "bounded index range has no reverse cursor".into(),
            })?;
        let cursor = IndexCursor {
            prefix: prefix.as_bytes().to_vec(),
            last_key: upper,
            scope: bounds.cursor_scope(),
        };
        self.scan_index_range_before_from(index, exact_prefix, range, &cursor, limit)
    }

    pub fn scan_index_range_before(
        &self,
        index: &CatalogId,
        exact_prefix: &[SavedKey],
        range: &IndexRangeBounds,
        cursor: &IndexCursor,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        self.scan_index_range_before_from(index, exact_prefix, range, cursor, limit)
    }

    /// Visit every row under one index id. The callback sees the stored index-key tuple
    /// and identity exactly as encoded, so callers can detect stale rows whose key arity
    /// no longer matches the current source declaration.
    pub fn for_each_index_entry(
        &self,
        index: &CatalogId,
        visit: &mut IndexEntryVisitor<'_>,
    ) -> Result<(), StoreError> {
        let prefix = CellKey::index_key_prefix(index, &[]);
        self.for_each_page_entry(prefix.as_bytes(), |key, value| {
            let rest = key.get(prefix.as_bytes().len()..).unwrap_or_default();
            let (index_keys, identity) =
                decode_index_entry_key(rest).map_err(|_| corrupt_cell(key))?;
            visit(&index_keys, &identity, value)?;
            Ok(std::ops::ControlFlow::Continue(()))
        })
    }

    /// Pin a consistent read snapshot for the lifetime of the returned guard, so a
    /// multi-call traversal reads one coherent version of saved data. The handle
    /// rejects writes until the guard is dropped.
    pub fn read_snapshot(&self) -> Result<ReadSnapshot<'_>, StoreError> {
        self.backend.borrow_mut().begin_snapshot()?;
        Ok(ReadSnapshot { store: self })
    }

    /// Whether the store holds no saved data: no data or index cells. Empty-only
    /// restore rejects a non-empty target; counted replace mode checks the live
    /// record count before clearing inside the restore transaction.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.family_is_empty(&CellKey::data_family())?
            && self.family_is_empty(&CellKey::index_family())?)
    }

    /// Clear the durable families a restore replay owns. Callers run this inside
    /// their restore transaction so target data, derived indexes, accepted catalog,
    /// and replay metadata are replaced atomically by the backup stream.
    pub fn clear_restore_target(&self) -> Result<(), StoreError> {
        self.delete_cells(CellKey::data_family().as_bytes())?;
        self.delete_cells(CellKey::index_family().as_bytes())?;
        self.delete_cells(CellKey::catalog_family().as_bytes())?;
        self.delete_cells(CellKey::meta(MetaCell::CommitRecord).as_bytes())?;
        self.delete_cells(CellKey::meta(MetaCell::Commit).as_bytes())?;
        self.delete_cells(CellKey::meta(MetaCell::StoreUid).as_bytes())
    }

    fn family_is_empty(&self, prefix: &CellKey) -> Result<bool, StoreError> {
        Ok(self.scan(prefix.as_bytes(), 1)?.entries.is_empty())
    }

    /// Visit every data-family cell in encoded order. Index-family cells are derived
    /// from data and rebuilt on restore, so a backup carries data only. Wrap the call
    /// in a [`read_snapshot`] when every page must observe one coherent version.
    ///
    /// [`read_snapshot`]: TreeStore::read_snapshot
    pub fn visit_backup_cells(
        &self,
        mut visit: impl for<'cell> FnMut(TreeBackupCell<'cell>) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        self.visit_backup_cells_until(|cell| {
            visit(cell)?;
            Ok(ControlFlow::Continue(()))
        })
    }

    /// Visit data-family cells in encoded order until the visitor stops.
    pub fn visit_backup_cells_until(
        &self,
        mut visit: impl for<'cell> FnMut(TreeBackupCell<'cell>) -> Result<ControlFlow<()>, StoreError>,
    ) -> Result<(), StoreError> {
        self.visit_family(CellKey::data_family().as_bytes(), &mut visit)
    }

    fn visit_family(
        &self,
        prefix: &[u8],
        visit: &mut impl for<'cell> FnMut(TreeBackupCell<'cell>) -> Result<ControlFlow<()>, StoreError>,
    ) -> Result<(), StoreError> {
        let mut page = self.scan(prefix, BACKUP_SCAN_PAGE)?;
        let mut previous_resume: Option<Vec<u8>> = None;
        loop {
            for (key, value) in &page.entries {
                if visit(TreeBackupCell::from_raw(key, value)?)?.is_break() {
                    return Ok(());
                }
            }
            if !page.truncated {
                return Ok(());
            }
            let resume = page
                .entries
                .last()
                .map(|(key, _)| key.clone())
                .ok_or_else(|| StoreError::InvalidCursor {
                    message: "data-family scan page was truncated without a cursor".into(),
                })?;
            if let Some(previous) = &previous_resume {
                guard_page_cursor_advances(&resume, previous, std::cmp::Ordering::Greater)?;
            }
            page = self.scan_after(prefix, &resume, BACKUP_SCAN_PAGE)?;
            previous_resume = Some(resume);
        }
    }

    fn read_cell(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.backend.borrow().read(key)
    }

    fn read_cell_prefix(
        &self,
        key: &[u8],
        limit: usize,
    ) -> Result<Option<DataValuePrefix>, StoreError> {
        self.backend
            .borrow()
            .read_prefix(key, limit)
            .map(|prefix| prefix.map(DataValuePrefix::from))
    }

    fn write_cell(&self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.with_digest_commit(
            "write",
            |store| {
                // A data-family write maintains the touched root's digest delta in constant
                // time: subtract any value being overwritten and add the new one. The single
                // read here is the same key the write addresses, so a write stays one logical
                // operation.
                if let Some(store) = store {
                    if let Some(prior) = self.read_cell(key)? {
                        self.adjust_digest_delta(store, |delta| delta.remove_cell(key, &prior));
                    }
                    self.adjust_digest_delta(store, |delta| delta.add_cell(key, &value));
                }
                self.backend.borrow_mut().write(key, value)
            },
            decode_data_family_store(key),
        )
    }

    fn delete_cells(&self, prefix: &[u8]) -> Result<(), StoreError> {
        self.with_digest_commit(
            "delete",
            |store| {
                // A data-family delete removes a whole subtree, so the touched root's digest
                // delta subtracts every cell the delete drops. The scan is bounded by the
                // deleted cells, never the whole root, and routes through the single delete
                // primitive so a new data-delete path is covered without remembering to
                // maintain the digest.
                if let Some(store) = store {
                    let store = store.clone();
                    self.for_each_cell_under(prefix, &mut |key, value| {
                        self.adjust_digest_delta(&store, |delta| delta.remove_cell(key, value));
                        Ok(())
                    })?;
                }
                self.backend.borrow_mut().delete(prefix)
            },
            decode_data_family_store(prefix),
        )
    }

    /// Run one data mutation and its digest maintenance. Inside an open transaction the
    /// delta is accumulated and the outer commit stamps it. A bare mutation with no open
    /// transaction auto-commits at the backend, so this brackets the mutation and its
    /// digest stamp in one transaction, keeping the stamped digest atomic with the cells it
    /// covers even on the unbracketed path. When the mutation does not touch the data
    /// family there is no digest to maintain and the mutation runs directly.
    fn with_digest_commit(
        &self,
        op: &'static str,
        mutate: impl FnOnce(Option<&CatalogId>) -> Result<(), StoreError>,
        store: Option<CatalogId>,
    ) -> Result<(), StoreError> {
        if store.is_none() || self.transaction_depth() > 0 {
            return mutate(store.as_ref());
        }
        // Reject an unwritable handle with the mutation's own error before the
        // self-begin bracket opens, so a read-only data mutation surfaces its
        // contracted op rather than the bracket's, and leaves no half-open
        // transaction behind for teardown to trip over.
        self.backend.borrow().require_write_access(op)?;
        self.begin()?;
        let result = mutate(store.as_ref()).and_then(|()| self.stamp_commit_record());
        match result {
            Ok(()) => {
                self.backend.borrow_mut().commit()?;
                self.digest_deltas.borrow_mut().clear();
                Ok(())
            }
            Err(error) => {
                let _ = self.backend.borrow_mut().rollback();
                self.digest_deltas.borrow_mut().clear();
                Err(error)
            }
        }
    }

    fn adjust_digest_delta(&self, store: &CatalogId, adjust: impl FnOnce(&mut RootDigest)) {
        let mut deltas = self.digest_deltas.borrow_mut();
        adjust(deltas.entry(store.clone()).or_default());
    }

    /// Page over every cell under `prefix` in encoded order, raw key and value bytes, so a
    /// torn-but-decodable value still contributes the bytes the store actually holds. Used
    /// to re-derive the structural digest and to subtract a deleted subtree.
    fn for_each_data_cell_raw(&self, visit: &mut RawCellVisitor<'_>) -> Result<(), StoreError> {
        self.for_each_cell_under(CellKey::data_family().as_bytes(), visit)
    }

    fn for_each_cell_under(
        &self,
        prefix: &[u8],
        visit: &mut RawCellVisitor<'_>,
    ) -> Result<(), StoreError> {
        let mut page = self.scan(prefix, BACKUP_SCAN_PAGE)?;
        let mut previous_resume: Option<Vec<u8>> = None;
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
                .ok_or_else(|| StoreError::InvalidCursor {
                    message: "data cell scan page was truncated without a cursor".into(),
                })?;
            if let Some(previous) = &previous_resume {
                guard_page_cursor_advances(&resume, previous, std::cmp::Ordering::Greater)?;
            }
            page = self.scan_after(prefix, &resume, BACKUP_SCAN_PAGE)?;
            previous_resume = Some(resume);
        }
    }

    fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        self.backend.borrow().scan(prefix, limit)
    }

    fn scan_after(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        self.backend.borrow().scan_after(prefix, cursor, limit)
    }

    fn scan_before(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        self.backend.borrow().scan_before(prefix, cursor, limit)
    }

    fn scan_between(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        self.backend
            .borrow()
            .scan_between(prefix, lower, upper, limit)
    }

    fn scan_between_after(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        self.backend
            .borrow()
            .scan_between_after(prefix, lower, upper, cursor, limit)
    }

    fn scan_between_before(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        self.backend
            .borrow()
            .scan_between_before(prefix, lower, upper, cursor, limit)
    }
}

impl From<ValuePrefix> for DataValuePrefix {
    fn from(prefix: ValuePrefix) -> Self {
        Self {
            bytes: prefix.bytes,
            truncated: prefix.truncated,
        }
    }
}

/// A pinned read snapshot over a [`TreeStore`]. While it is held, every read and
/// scan observes one consistent version of saved data, and writes on the same
/// handle are rejected; dropping it resumes live reads and writes.
#[must_use = "a read snapshot is released as soon as it is dropped"]
pub struct ReadSnapshot<'a> {
    store: &'a TreeStore,
}

impl Drop for ReadSnapshot<'_> {
    fn drop(&mut self) {
        self.store.backend.borrow_mut().end_snapshot();
    }
}

impl TreeStore {
    fn scan_index_tuple_from(
        &self,
        index: &CatalogId,
        index_keys: &[SavedKey],
        cursor: Option<&IndexCursor>,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        if limit == 0 {
            return Ok(empty_index_page());
        }
        let prefix = CellKey::index_tuple_prefix(index, index_keys);
        let page = match cursor {
            Some(cursor) => {
                if cursor.prefix != prefix.as_bytes() || cursor.scope != IndexCursorScope::Exact {
                    return Err(StoreError::InvalidCursor {
                        message: "index cursor does not match exact index tuple".into(),
                    });
                }
                self.scan_after(prefix.as_bytes(), cursor.last_key.as_slice(), limit)?
            }
            None => self.scan(prefix.as_bytes(), limit)?,
        };
        let range = prefix.range();
        let mut entries = Vec::new();
        let mut last_key = None;
        for (key, value) in page.entries {
            if !range.contains(&key) {
                continue;
            }
            last_key = Some(key.clone());
            let identity = decode_index_identity(&key[prefix.as_bytes().len()..])
                .map_err(|_| corrupt_cell(&key))?;
            entries.push(IndexEntry {
                index_keys: index_keys.to_vec(),
                identity,
                value,
            });
        }
        let cursor = index_page_cursor(
            page.truncated,
            last_key,
            prefix.as_bytes(),
            IndexCursorScope::Exact,
        )?;
        Ok(IndexPage {
            entries,
            cursor,
            truncated: page.truncated,
        })
    }

    fn scan_index_range_from(
        &self,
        index: &CatalogId,
        exact_prefix: &[SavedKey],
        range: &IndexRangeBounds,
        cursor: Option<&IndexCursor>,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        if limit == 0 {
            return Ok(empty_index_page());
        }
        let prefix = CellKey::index_key_prefix(index, exact_prefix);
        let Some(bounds) = normalized_index_range(prefix.as_bytes(), range) else {
            if cursor.is_some() {
                return Err(StoreError::InvalidCursor {
                    message: "index cursor does not match bounded index range".into(),
                });
            }
            return Ok(empty_index_page());
        };
        let page = match cursor {
            Some(cursor) => {
                if cursor.prefix != prefix.as_bytes() {
                    return Err(StoreError::InvalidCursor {
                        message: "index cursor does not match bounded index range".into(),
                    });
                }
                if cursor.scope != bounds.cursor_scope() {
                    return Err(StoreError::InvalidCursor {
                        message: "index cursor does not match bounded index range".into(),
                    });
                }
                self.scan_between_after(
                    prefix.as_bytes(),
                    Some(bounds.lower.as_slice()),
                    bounds.upper.as_deref(),
                    cursor.last_key.as_slice(),
                    limit,
                )?
            }
            None => self.scan_between(
                prefix.as_bytes(),
                Some(bounds.lower.as_slice()),
                bounds.upper.as_deref(),
                limit,
            )?,
        };
        self.decode_index_range_page(index, prefix, page, bounds.cursor_scope())
    }

    fn scan_index_range_before_from(
        &self,
        index: &CatalogId,
        exact_prefix: &[SavedKey],
        range: &IndexRangeBounds,
        cursor: &IndexCursor,
        limit: usize,
    ) -> Result<IndexPage, StoreError> {
        if limit == 0 {
            return Ok(empty_index_page());
        }
        let prefix = CellKey::index_key_prefix(index, exact_prefix);
        if cursor.prefix != prefix.as_bytes() {
            return Err(StoreError::InvalidCursor {
                message: "index cursor does not match bounded index range".into(),
            });
        }
        let Some(bounds) = normalized_index_range(prefix.as_bytes(), range) else {
            return Err(StoreError::InvalidCursor {
                message: "index cursor does not match bounded index range".into(),
            });
        };
        if cursor.scope != bounds.cursor_scope() {
            return Err(StoreError::InvalidCursor {
                message: "index cursor does not match bounded index range".into(),
            });
        }
        let page = self.scan_between_before(
            prefix.as_bytes(),
            Some(bounds.lower.as_slice()),
            bounds.upper.as_deref(),
            cursor.last_key.as_slice(),
            limit,
        )?;
        self.decode_index_range_page(index, prefix, page, bounds.cursor_scope())
    }

    fn decode_index_range_page(
        &self,
        index: &CatalogId,
        prefix: CellKey,
        page: ScanPage,
        scope: IndexCursorScope,
    ) -> Result<IndexPage, StoreError> {
        let full_index_prefix = CellKey::index_key_prefix(index, &[]);
        let mut entries = Vec::new();
        let mut last_key = None;
        for (key, value) in page.entries {
            if !key.starts_with(prefix.as_bytes()) {
                continue;
            }
            last_key = Some(key.clone());
            let (index_keys, identity) =
                decode_index_entry_key(&key[full_index_prefix.as_bytes().len()..])
                    .map_err(|_| corrupt_cell(&key))?;
            entries.push(IndexEntry {
                index_keys,
                identity,
                value,
            });
        }
        let cursor = index_page_cursor(page.truncated, last_key, prefix.as_bytes(), scope)?;
        Ok(IndexPage {
            entries,
            cursor,
            truncated: page.truncated,
        })
    }

    fn child_count(
        &self,
        prefix: &[u8],
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<usize, StoreError> {
        let mut count = 0;
        self.scan_children_until(
            prefix,
            |_| {
                count += 1;
                std::ops::ControlFlow::Continue(())
            },
            decode,
        )?;
        Ok(count)
    }

    /// Drive a bounded prefix scan to exhaustion, paging by [`CHILD_SCAN_PAGE_LIMIT`]
    /// and resuming each page from the previous page's last key. `visit` sees every
    /// raw `(key, value)` under `prefix` in order and may stop the walk early. A
    /// backend that reports a truncated page but yields no key to resume from is a
    /// corrupt scan state, surfaced as [`StoreError::InvalidCursor`].
    fn for_each_page_entry(
        &self,
        prefix: &[u8],
        mut visit: impl FnMut(&[u8], &[u8]) -> Result<std::ops::ControlFlow<()>, StoreError>,
    ) -> Result<(), StoreError> {
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let page = match cursor.as_ref() {
                Some(cursor) => self.scan_after(prefix, cursor, CHILD_SCAN_PAGE_LIMIT)?,
                None => self.scan(prefix, CHILD_SCAN_PAGE_LIMIT)?,
            };
            let next_cursor = page.entries.last().map(|(key, _)| key.clone());
            if let (Some(previous), Some(next)) = (&cursor, &next_cursor) {
                guard_page_cursor_advances(next, previous, std::cmp::Ordering::Greater)?;
            }
            for (key, value) in &page.entries {
                if visit(key, value)?.is_break() {
                    return Ok(());
                }
            }
            if !page.truncated {
                break;
            }
            if next_cursor.is_none() {
                return Err(StoreError::InvalidCursor {
                    message: "scan page was truncated without a cursor".into(),
                });
            }
            cursor = next_cursor;
        }
        Ok(())
    }

    fn scan_children_until(
        &self,
        prefix: &[u8],
        mut visit: impl FnMut(SavedKey) -> std::ops::ControlFlow<()>,
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<(), StoreError> {
        let mut last_child: Option<SavedKey> = None;
        self.for_each_page_entry(prefix, |key, _| {
            let rest = key.get(prefix.len()..).unwrap_or_default();
            let Some(child) = decode(rest)? else {
                return Ok(std::ops::ControlFlow::Continue(()));
            };
            if last_child.as_ref() == Some(&child) {
                return Ok(std::ops::ControlFlow::Continue(()));
            }
            last_child = Some(child.clone());
            Ok(visit(child))
        })
    }

    fn scan_children_reverse_until(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        mut visit: impl FnMut(SavedKey) -> std::ops::ControlFlow<()>,
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<(), StoreError> {
        let mut cursor = cursor.to_vec();
        let mut last_child: Option<SavedKey> = None;
        loop {
            let page = self.scan_before(prefix, &cursor, CHILD_SCAN_PAGE_LIMIT)?;
            let next_cursor = page.entries.last().map(|(key, _)| key.clone());
            for (key, _) in page.entries {
                let rest = key.get(prefix.len()..).unwrap_or_default();
                let Some(child) = decode(rest)? else {
                    continue;
                };
                if last_child.as_ref() == Some(&child) {
                    continue;
                }
                last_child = Some(child.clone());
                if visit(child).is_break() {
                    return Ok(());
                }
            }
            if !page.truncated {
                break;
            }
            let next_cursor = next_cursor.ok_or_else(|| StoreError::InvalidCursor {
                message: "reverse child scan page was truncated without a cursor".into(),
            })?;
            guard_page_cursor_advances(&next_cursor, &cursor, std::cmp::Ordering::Less)?;
            cursor = next_cursor;
        }
        Ok(())
    }

    fn scan_record_children_until(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        scan: RecordChildScan,
        mut visit: impl FnMut(SavedKey) -> std::ops::ControlFlow<()>,
    ) -> Result<(), StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        let mut last_child: Option<SavedKey> = None;
        self.for_each_page_entry(prefix.as_bytes(), |key, value| {
            let decoded = decode_data_cell_key(key).ok_or_else(|| corrupt_cell(key))?;
            let Some(child) = record_child_of(&decoded, value, store, identity_prefix, scan) else {
                return Ok(std::ops::ControlFlow::Continue(()));
            };
            if last_child.as_ref() == Some(&child) {
                return Ok(std::ops::ControlFlow::Continue(()));
            }
            last_child = Some(child.clone());
            Ok(visit(child))
        })
    }

    fn scan_record_children_after_cursor(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        cursor: &[u8],
        scan: RecordChildScan,
        mut visit: impl FnMut(SavedKey) -> std::ops::ControlFlow<()>,
    ) -> Result<(), StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        let mut cursor = cursor.to_vec();
        loop {
            let page = self.scan_after(prefix.as_bytes(), &cursor, 1)?;
            let next_cursor = page.entries.last().map(|(key, _)| key.clone());
            for (key, value) in page.entries {
                let decoded = decode_data_cell_key(&key).ok_or_else(|| corrupt_cell(&key))?;
                let Some(child) = record_child_of(&decoded, &value, store, identity_prefix, scan)
                else {
                    continue;
                };
                if visit(child).is_break() {
                    return Ok(());
                }
            }
            if !page.truncated {
                break;
            }
            let next_cursor = next_cursor.ok_or_else(|| StoreError::InvalidCursor {
                message: "record seek page was truncated without a cursor".into(),
            })?;
            guard_page_cursor_advances(&next_cursor, &cursor, std::cmp::Ordering::Greater)?;
            cursor = next_cursor;
        }
        Ok(())
    }

    fn scan_record_children_reverse_until(
        &self,
        store: &CatalogId,
        identity_prefix: &[SavedKey],
        cursor: &[u8],
        scan: RecordChildScan,
        mut visit: impl FnMut(SavedKey) -> std::ops::ControlFlow<()>,
    ) -> Result<(), StoreError> {
        let prefix = CellKey::record_prefix(store, identity_prefix);
        let mut cursor = cursor.to_vec();
        let mut last_child: Option<SavedKey> = None;
        loop {
            let page = self.scan_before(prefix.as_bytes(), &cursor, CHILD_SCAN_PAGE_LIMIT)?;
            let next_cursor = page.entries.last().map(|(key, _)| key.clone());
            for (key, value) in page.entries {
                let decoded = decode_data_cell_key(&key).ok_or_else(|| corrupt_cell(&key))?;
                let Some(child) = record_child_of(&decoded, &value, store, identity_prefix, scan)
                else {
                    continue;
                };
                if last_child.as_ref() == Some(&child) {
                    continue;
                }
                last_child = Some(child.clone());
                if visit(child).is_break() {
                    return Ok(());
                }
            }
            if !page.truncated {
                break;
            }
            let next_cursor = next_cursor.ok_or_else(|| StoreError::InvalidCursor {
                message: "reverse record scan page was truncated without a cursor".into(),
            })?;
            guard_page_cursor_advances(&next_cursor, &cursor, std::cmp::Ordering::Less)?;
            cursor = next_cursor;
        }
        Ok(())
    }

    fn index_child_stem(
        &self,
        index: &CatalogId,
        key_prefix: &[SavedKey],
        child: &SavedKey,
    ) -> Result<Vec<u8>, StoreError> {
        let identity_stem = index_identity_child_stem(index, key_prefix, child);
        let page = self.scan(&identity_stem, 1)?;
        if let Some((key, _)) = page.entries.first() {
            let prefix = CellKey::index_key_prefix(index, key_prefix);
            let rest = key.get(prefix.as_bytes().len()..).unwrap_or_default();
            if decode_index_child(rest)?.as_ref() == Some(child) {
                return Ok(identity_stem);
            }
        }
        Ok(index_key_child_stem(index, key_prefix, child))
    }

    fn next_child_after_cursor(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut cursor = cursor.to_vec();
        loop {
            let page = self.scan_after(prefix, &cursor, 1)?;
            let next_cursor = page.entries.last().map(|(key, _)| key.clone());
            for (key, _) in page.entries {
                let rest = key.get(prefix.len()..).unwrap_or_default();
                let Some(child) = decode(rest)? else {
                    continue;
                };
                return Ok(Some(child));
            }
            if !page.truncated {
                break;
            }
            let next_cursor = next_cursor.ok_or_else(|| StoreError::InvalidCursor {
                message: "child scan page was truncated without a cursor".into(),
            })?;
            guard_page_cursor_advances(&next_cursor, &cursor, std::cmp::Ordering::Greater)?;
            cursor = next_cursor;
        }
        Ok(None)
    }

    fn prev_child_before(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        before: &SavedKey,
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<SavedKey>, StoreError> {
        let mut result = None;
        self.scan_children_reverse_until(
            prefix,
            cursor,
            |child| {
                if &child != before {
                    result = Some(child);
                    return std::ops::ControlFlow::Break(());
                }
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
        let cursor = prefix_successor(prefix).ok_or_else(|| StoreError::InvalidCursor {
            message: "child prefix has no upper bound".into(),
        })?;
        self.scan_children_reverse_until(
            prefix,
            &cursor,
            |child| {
                result = Some(child);
                std::ops::ControlFlow::Break(())
            },
            decode,
        )?;
        Ok(result)
    }

    fn max_int_child(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        decode: fn(&[u8]) -> Result<Option<SavedKey>, StoreError>,
    ) -> Result<Option<i64>, StoreError> {
        let mut result = None;
        self.scan_children_reverse_until(
            prefix,
            cursor,
            |child| {
                if let SavedKey::Int(value) = child {
                    result = Some(value);
                }
                std::ops::ControlFlow::Break(())
            },
            decode,
        )?;
        Ok(result)
    }
}

fn decode_data_child(bytes: &[u8]) -> Result<Option<SavedKey>, StoreError> {
    decode_data_child_key(bytes).map_err(|_| corrupt_cell(bytes))
}

fn decode_index_child(bytes: &[u8]) -> Result<Option<SavedKey>, StoreError> {
    decode_index_child_key(bytes).map_err(|_| corrupt_cell(bytes))
}

fn index_identity_child_stem(
    index: &CatalogId,
    key_prefix: &[SavedKey],
    child: &SavedKey,
) -> Vec<u8> {
    let mut bytes = CellKey::index_tuple_prefix(index, key_prefix).into_bytes();
    bytes.extend_from_slice(&encode_key_value(child));
    bytes
}

fn index_key_child_stem(index: &CatalogId, key_prefix: &[SavedKey], child: &SavedKey) -> Vec<u8> {
    let mut child_prefix = key_prefix.to_vec();
    child_prefix.push(child.clone());
    CellKey::index_key_prefix(index, &child_prefix).into_bytes()
}

fn index_range_lower_bound(prefix: &[u8], range: &IndexRangeBounds) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    if let Some(lower) = &range.lower {
        bytes.extend_from_slice(&encode_key_value(lower));
        if !range.lower_inclusive {
            return prefix_successor(&bytes).unwrap_or(bytes);
        }
    }
    bytes
}

fn index_range_upper_bound(prefix: &[u8], range: &IndexRangeBounds) -> Option<Vec<u8>> {
    let upper = range.upper.as_ref()?;
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(&encode_key_value(upper));
    if range.upper_inclusive {
        prefix_successor(&bytes)
    } else {
        Some(bytes)
    }
}

fn normalized_index_range(prefix: &[u8], range: &IndexRangeBounds) -> Option<NormalizedIndexRange> {
    let lower = index_range_lower_bound(prefix, range);
    let upper = index_range_upper_bound(prefix, range);
    if upper.as_ref().is_some_and(|upper| lower >= *upper) {
        return None;
    }
    Some(NormalizedIndexRange { lower, upper })
}

fn empty_index_page() -> IndexPage {
    IndexPage {
        entries: Vec::new(),
        cursor: None,
        truncated: false,
    }
}

fn index_page_cursor(
    truncated: bool,
    last_key: Option<Vec<u8>>,
    prefix: &[u8],
    scope: IndexCursorScope,
) -> Result<Option<IndexCursor>, StoreError> {
    if !truncated {
        return Ok(None);
    }
    let last_key = last_key.ok_or_else(|| StoreError::InvalidCursor {
        message: "index scan page was truncated without a cursor".into(),
    })?;
    Ok(Some(IndexCursor {
        prefix: prefix.to_vec(),
        last_key,
        scope,
    }))
}

pub fn encode_tree_enum_member(value: &TreeEnumMember) -> Result<Vec<u8>, StoreError> {
    let mut bytes = vec![TREE_VALUE_VERSION_V0];
    put_catalog_id(&value.enum_id, &mut bytes)?;
    put_catalog_id(&value.member_id, &mut bytes)?;
    Ok(bytes)
}

pub fn decode_tree_enum_member(bytes: &[u8]) -> Result<TreeEnumMember, StoreError> {
    let mut cursor = BoundedReader::new(bytes, corrupt_cell);
    take_tree_value_version(&mut cursor)?;
    let enum_id = take_catalog_id(&mut cursor)?;
    let member_id = take_catalog_id(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(corrupt_cell(bytes));
    }
    Ok(TreeEnumMember { enum_id, member_id })
}

fn put_bytes(value: &[u8], out: &mut Vec<u8>) -> Result<(), StoreError> {
    let len = u32::try_from(value.len()).map_err(|_| StoreError::LimitExceeded {
        limit: "tree cell value length",
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

fn put_catalog_id(id: &CatalogId, out: &mut Vec<u8>) -> Result<(), StoreError> {
    put_bytes(id.as_str().as_bytes(), out)
}

type TreeValueReader<'a> = BoundedReader<'a, StoreError>;

fn take_tree_value_version(cursor: &mut TreeValueReader<'_>) -> Result<(), StoreError> {
    let version = cursor.take_u8()?;
    if version == TREE_VALUE_VERSION_V0 {
        Ok(())
    } else {
        Err(corrupt_cell(&[version]))
    }
}

fn take_catalog_id(cursor: &mut TreeValueReader<'_>) -> Result<CatalogId, StoreError> {
    let raw = cursor.take_prefixed_bytes()?;
    let id = std::str::from_utf8(raw).map_err(|_| corrupt_cell(raw))?;
    CatalogId::new(id).map_err(|_| corrupt_cell(raw))
}

fn corrupt_cell(bytes: &[u8]) -> StoreError {
    StoreError::Corruption {
        message: format!("tree-cell data is malformed ({} bytes)", bytes.len()),
    }
}

/// Fold a root's net digest delta into the commit record's per-root list, keyed by store id.
fn fold_root_digest(
    digests: &mut Vec<(CatalogId, RootDigest)>,
    store: &CatalogId,
    delta: RootDigest,
) {
    match digests.iter_mut().find(|(id, _)| id == store) {
        Some((_, digest)) => digest.add(delta),
        None => {
            let mut digest = RootDigest::zero();
            digest.add(delta);
            digests.push((store.clone(), digest));
        }
    }
}

/// The active saved roots the accepted catalog declares, by store id in canonical order — the set
/// the commit record seals and the store-open path cross-checks the record against.
fn active_store_roots(
    snapshot: Option<&marrow_catalog::CatalogMetadata>,
) -> Result<Vec<CatalogId>, StoreError> {
    let Some(snapshot) = snapshot else {
        return Ok(Vec::new());
    };
    let mut roots = snapshot
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == marrow_catalog::CatalogEntryKind::Store
                && entry.lifecycle == marrow_catalog::CatalogLifecycle::Active
        })
        .map(|entry| {
            CatalogId::new(&entry.stable_id).map_err(|_| StoreError::Corruption {
                message: "an active saved root has a malformed catalog id".into(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    roots.sort();
    Ok(roots)
}

/// A committed index entry stores its record identity after the `INDEX_IDENTITY` separator and
/// keeps a single redundant copy whose location the entry kind decides, discriminated by the cell
/// value shape: a non-unique entry carries the [`INDEX_MARKER`] value and repeats the identity as
/// the trailing keys of its ordered tuple, while a unique entry carries the identity in its value.
/// The check reconciles the stored identity against the one copy reads actually consume for that
/// kind — never an OR that lets a coincidental tuple suffix mask a corrupt unique value, or a value
/// marker mask a corrupt non-unique tuple.
fn index_entry_identity_is_consistent(entry: &IndexCellKey, value: &[u8]) -> bool {
    if value == INDEX_MARKER {
        entry.index_keys.ends_with(&entry.identity)
    } else {
        value == encode_identity_payload(&entry.identity).as_slice()
    }
}

/// A paged scan that resumes from a page's final key must move strictly past the
/// cursor it was seeded with: `scan_after` returns keys above its cursor and
/// `scan_before` keys below it, so on a healthy store the next resume key always
/// advances in `direction`. A non-advancing resume key is backend damage that would
/// re-query the same cursor forever, so it fails closed rather than looping.
fn guard_page_cursor_advances(
    next: &[u8],
    previous: &[u8],
    direction: std::cmp::Ordering,
) -> Result<(), StoreError> {
    if next.cmp(previous) != direction {
        return Err(StoreError::Corruption {
            message: "store scan returned a non-advancing page cursor".into(),
        });
    }
    Ok(())
}

/// A child step seeded from `cursor` must move strictly in `direction` past it; a
/// healthy ordered store guarantees this. A non-advancing or out-of-order child is
/// backend damage that would otherwise spin a descent forever, so it fails closed
/// rather than looping. `direction` is [`Ordering::Greater`] for forward steps and
/// [`Ordering::Less`] for reverse steps.
fn guard_child_advances(
    child: Option<SavedKey>,
    cursor: &SavedKey,
    direction: std::cmp::Ordering,
) -> Result<Option<SavedKey>, StoreError> {
    if let Some(child) = &child
        && child.cmp(cursor) != direction
    {
        return Err(StoreError::Corruption {
            message: "record scan returned a non-advancing child key".into(),
        });
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{
        CellKey, CommitMetadata, DataPathSegment, IndexCursor, IndexCursorScope, IndexRangeBounds,
        NODE_MARKER, StoreUid, TREE_BACKUP_MAX_CELL_BYTES, TreeBackupCellBuf,
        TreeBackupCellReadError, TreeStore,
    };
    use crate::StoreError;
    use crate::backend::counting::{BackendCounts, CountingBackend};
    use crate::backend::{Backend, ScanPage, ValuePrefix};
    use crate::cell::{CatalogId, DataCellKind, MetaCell};
    use crate::key::{INDEX_MARKER, SavedKey, encode_identity_payload};
    use crate::mem::MemStore;
    use crate::metadata::{decode_commit_metadata, encode_commit_metadata};

    enum BackendFault {
        FailOnNthWrite {
            writes_until_fault: Cell<usize>,
        },
        EmptyTruncatedScan {
            method: EmptyTruncatedScanMethod,
            prefix: Vec<u8>,
        },
        /// Model backend damage where a record-prefix `scan_after` yields a
        /// validly-decoding node cell whose child does not advance past the
        /// cursor, so a descent that trusts the backend to advance would loop.
        NonAdvancingRecordChild {
            prefix: Vec<u8>,
            cell_key: Vec<u8>,
        },
        /// Model damage where a prefix scan returns validly-decoding data cells whose
        /// decoded keys are not in scan order, the divergence `verify_readable` catches.
        OutOfOrderDataScan {
            prefix: Vec<u8>,
            cells: Vec<Vec<u8>>,
        },
        /// Model damage where a paged `scan`/`scan_after` under a prefix always reports
        /// the same final key with `truncated: true`, so a page-cursor loop that resumes
        /// from the last key never advances past it and would spin forever.
        NonAdvancingScanPage {
            prefix: Vec<u8>,
            cells: Vec<Vec<u8>>,
        },
        /// Model a flipped index-subtree byte: an index-family scan yields one cell whose
        /// key no longer decodes under the v0 grammar, the damage a structural index walk
        /// must catch even though the data records are intact.
        CorruptIndexScan {
            cells: Vec<Vec<u8>>,
        },
        /// Model an interior-separator flip that misroutes a point lookup: `read` reads
        /// absent for a key a range scan still yields, so the committed cell is invisible to
        /// a seek yet intact in the raw scan and the structural digest.
        SeekMisroute {
            hidden: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<Vec<u8>>>>,
        },
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum EmptyTruncatedScanMethod {
        Scan,
        ScanAfter,
        ScanBetween,
        ScanBetweenBefore,
    }

    struct FaultingBackend {
        inner: MemStore,
        fault: BackendFault,
    }

    impl FaultingBackend {
        fn fail_on_nth_write(writes_before_fault: usize) -> Self {
            Self {
                inner: MemStore::default(),
                fault: BackendFault::FailOnNthWrite {
                    writes_until_fault: Cell::new(writes_before_fault),
                },
            }
        }

        fn empty_truncated_backup_scan() -> Self {
            Self::empty_truncated_scan(
                EmptyTruncatedScanMethod::Scan,
                CellKey::data_family().as_bytes(),
            )
        }

        fn empty_truncated_scan(method: EmptyTruncatedScanMethod, prefix: &[u8]) -> Self {
            Self {
                inner: MemStore::default(),
                fault: BackendFault::EmptyTruncatedScan {
                    method,
                    prefix: prefix.to_vec(),
                },
            }
        }

        fn should_return_empty_truncated(
            &self,
            method: EmptyTruncatedScanMethod,
            prefix: &[u8],
        ) -> bool {
            matches!(
                &self.fault,
                BackendFault::EmptyTruncatedScan {
                    method: fault_method,
                    prefix: fault_prefix,
                } if *fault_method == method && fault_prefix.as_slice() == prefix
            )
        }

        fn empty_truncated_page() -> ScanPage {
            ScanPage {
                entries: Vec::new(),
                truncated: true,
            }
        }

        /// Seed two single-int records, then pin every record-prefix `scan_after`
        /// to the first record's node cell so the decoded child never advances.
        fn non_advancing_record_child(store: &CatalogId) -> Self {
            let mut inner = MemStore::default();
            for id in [1, 2] {
                let key = CellKey::node(store, &[SavedKey::Int(id)]).into_bytes();
                Backend::write(&mut inner, &key, NODE_MARKER.to_vec()).expect("seed record");
            }
            Self {
                inner,
                fault: BackendFault::NonAdvancingRecordChild {
                    prefix: CellKey::record_prefix(store, &[]).into_bytes(),
                    cell_key: CellKey::node(store, &[SavedKey::Int(1)]).into_bytes(),
                },
            }
        }

        /// Yield two record node cells whose decoded identities descend even though the
        /// scan reports them in forward order, the byte-vs-decoded-key divergence that
        /// stalls a descent. The data-family scan that `verify_readable` drives sees both.
        fn out_of_order_data_cells(store: &CatalogId) -> Self {
            Self {
                inner: MemStore::default(),
                fault: BackendFault::OutOfOrderDataScan {
                    prefix: CellKey::data_family().into_bytes(),
                    cells: vec![
                        CellKey::node(store, &[SavedKey::Int(2)]).into_bytes(),
                        CellKey::node(store, &[SavedKey::Int(1)]).into_bytes(),
                    ],
                },
            }
        }

        /// Seed two single-int records, then pin every `scan`/`scan_after` under the
        /// record family to a fixed truncated page so a page-cursor loop resuming from
        /// the last key never moves past it.
        fn non_advancing_scan_page(store: &CatalogId) -> Self {
            let mut inner = MemStore::default();
            for id in [1, 2] {
                let key = CellKey::node(store, &[SavedKey::Int(id)]).into_bytes();
                Backend::write(&mut inner, &key, NODE_MARKER.to_vec()).expect("seed record");
            }
            Self {
                inner,
                fault: BackendFault::NonAdvancingScanPage {
                    prefix: CellKey::data_family().into_bytes(),
                    cells: vec![CellKey::node(store, &[SavedKey::Int(1)]).into_bytes()],
                },
            }
        }

        /// Pin every record-family `scan`/`scan_after` to a fixed truncated page whose
        /// only entry the active record scan skips, so the inner page-cursor loop
        /// (`scan_record_children_after_cursor`) keeps re-querying the same cursor.
        fn non_advancing_record_page(store: &CatalogId) -> Self {
            Self {
                inner: MemStore::default(),
                fault: BackendFault::NonAdvancingScanPage {
                    prefix: CellKey::record_prefix(store, &[]).into_bytes(),
                    cells: vec![CellKey::node(store, &[SavedKey::Int(1)]).into_bytes()],
                },
            }
        }

        /// Pin every index-family `scan`/`scan_after` to a fixed truncated page holding one
        /// valid index entry, so the identity-scan loop resuming from the last key never
        /// moves past it and would spin without the page-cursor guard.
        fn non_advancing_index_page(index: &CatalogId) -> Self {
            Self {
                inner: MemStore::default(),
                fault: BackendFault::NonAdvancingScanPage {
                    prefix: CellKey::index_family().into_bytes(),
                    cells: vec![
                        CellKey::index(
                            index,
                            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
                            &[SavedKey::Int(1)],
                        )
                        .into_bytes(),
                    ],
                },
            }
        }

        /// Flip a byte in one stored index cell's key, so an index-family scan returns a
        /// cell that no longer decodes. Mirrors a single-byte corruption of an index node
        /// over an otherwise-intact store; the data family is left untouched.
        fn corrupt_index_cell(index: &CatalogId) -> Self {
            let mut healthy =
                CellKey::index(index, &[SavedKey::Str("a".into())], &[SavedKey::Int(1)])
                    .into_bytes();
            // Truncate the entry terminator so the key fails to decode under the v0 grammar.
            healthy.pop();
            Self {
                inner: MemStore::default(),
                fault: BackendFault::CorruptIndexScan {
                    cells: vec![healthy],
                },
            }
        }

        /// A backend whose point `read` reads absent for any key the returned handle lists,
        /// while every scan still yields it. Seed the store through it with the handle empty,
        /// then add committed cell keys to model the interior-separator flip that hides them
        /// from a seek.
        #[allow(clippy::type_complexity)]
        fn seek_misroute() -> (
            Self,
            std::rc::Rc<std::cell::RefCell<std::collections::HashSet<Vec<u8>>>>,
        ) {
            let hidden =
                std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new()));
            (
                Self {
                    inner: MemStore::default(),
                    fault: BackendFault::SeekMisroute {
                        hidden: hidden.clone(),
                    },
                },
                hidden,
            )
        }

        fn injected_page(&self, prefix: &[u8]) -> Option<ScanPage> {
            match &self.fault {
                BackendFault::CorruptIndexScan { cells }
                    if prefix.starts_with(CellKey::index_family().as_bytes()) =>
                {
                    Some(ScanPage {
                        entries: cells
                            .iter()
                            .map(|cell| (cell.clone(), Vec::new()))
                            .collect(),
                        truncated: false,
                    })
                }
                BackendFault::NonAdvancingRecordChild {
                    prefix: fault_prefix,
                    cell_key,
                } if fault_prefix.as_slice() == prefix => Some(ScanPage {
                    entries: vec![(cell_key.clone(), NODE_MARKER.to_vec())],
                    truncated: false,
                }),
                BackendFault::NonAdvancingScanPage {
                    prefix: fault_prefix,
                    cells,
                } if prefix.starts_with(fault_prefix) => Some(ScanPage {
                    entries: cells
                        .iter()
                        .map(|cell| (cell.clone(), NODE_MARKER.to_vec()))
                        .collect(),
                    truncated: true,
                }),
                BackendFault::OutOfOrderDataScan {
                    prefix: fault_prefix,
                    cells,
                } if fault_prefix.as_slice() == prefix => Some(ScanPage {
                    entries: cells
                        .iter()
                        .map(|cell| (cell.clone(), NODE_MARKER.to_vec()))
                        .collect(),
                    truncated: false,
                }),
                _ => None,
            }
        }
    }

    fn empty_commit_metadata(catalog_epoch: u64) -> CommitMetadata {
        CommitMetadata {
            commit_id: 0,
            catalog_epoch,
            layout_epoch: 0,
            source_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000001"
                    .to_string(),
            engine_profile_digest: [0; 8],
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
        }
    }

    impl Backend for FaultingBackend {
        fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
            if let BackendFault::SeekMisroute { hidden } = &self.fault
                && hidden.borrow().contains(key)
            {
                return Ok(None);
            }
            self.inner.read(key)
        }

        fn read_prefix(&self, key: &[u8], limit: usize) -> Result<Option<ValuePrefix>, StoreError> {
            self.inner.read_prefix(key, limit)
        }

        fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
            self.inner.require_write_access(op)
        }

        fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
            if let BackendFault::FailOnNthWrite { writes_until_fault } = &self.fault {
                let remaining = writes_until_fault.get();
                if remaining == 0 {
                    return Err(StoreError::Corruption {
                        message: "injected write fault".into(),
                    });
                }
                writes_until_fault.set(remaining - 1);
            }
            self.inner.write(key, value)
        }

        fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError> {
            self.inner.delete(prefix)
        }

        fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
            if self.should_return_empty_truncated(EmptyTruncatedScanMethod::Scan, prefix) {
                return Ok(Self::empty_truncated_page());
            }
            if let Some(page) = self.injected_page(prefix) {
                return Ok(page);
            }
            self.inner.scan(prefix, limit)
        }

        fn scan_after(
            &self,
            prefix: &[u8],
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            if self.should_return_empty_truncated(EmptyTruncatedScanMethod::ScanAfter, prefix) {
                return Ok(Self::empty_truncated_page());
            }
            if let Some(page) = self.injected_page(prefix) {
                return Ok(page);
            }
            self.inner.scan_after(prefix, cursor, limit)
        }

        fn scan_before(
            &self,
            prefix: &[u8],
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.inner.scan_before(prefix, cursor, limit)
        }

        fn scan_between(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            if self.should_return_empty_truncated(EmptyTruncatedScanMethod::ScanBetween, prefix) {
                return Ok(Self::empty_truncated_page());
            }
            self.inner.scan_between(prefix, lower, upper, limit)
        }

        fn scan_between_after(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.inner
                .scan_between_after(prefix, lower, upper, cursor, limit)
        }

        fn scan_between_before(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            if self
                .should_return_empty_truncated(EmptyTruncatedScanMethod::ScanBetweenBefore, prefix)
            {
                return Ok(Self::empty_truncated_page());
            }
            self.inner
                .scan_between_before(prefix, lower, upper, cursor, limit)
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

        fn transaction_depth(&self) -> usize {
            self.inner.transaction_depth()
        }

        fn begin_snapshot(&mut self) -> Result<(), StoreError> {
            self.inner.begin_snapshot()
        }

        fn end_snapshot(&mut self) {
            self.inner.end_snapshot();
        }
    }

    /// A record descent advances by feeding the prior child key back as an after-cursor.
    /// Backend damage that yields a non-advancing child must fail closed as corruption
    /// rather than loop forever, so the descent never trusts the backend to make progress.
    /// The walk runs on a worker thread with a deadline so a regression surfaces as a test
    /// timeout instead of hanging the suite.
    #[test]
    fn record_descent_rejects_a_non_advancing_child_as_corruption() {
        use std::sync::mpsc;
        use std::time::Duration;

        let store_id = catalog("cat_00000000000000000000000000000001");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let store = TreeStore::from_backend(Box::new(
                FaultingBackend::non_advancing_record_child(&store_id),
            ));
            let result = store.for_each_record(&store_id, 1, &mut |_| Ok(()));
            let _ = sender.send(result);
        });

        match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => assert_corruption(result),
            Err(_) => panic!("record descent did not terminate on a non-advancing child"),
        }
    }

    /// `recover` proves a store is traversable by reading it through `verify_readable`.
    /// Out-of-order data cells — the damage that stalls a record descent — must surface
    /// as corruption so recover refuses rather than reporting a false repair.
    #[test]
    fn verify_readable_rejects_out_of_order_data_cells() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let store = TreeStore::from_backend(Box::new(FaultingBackend::out_of_order_data_cells(
            &store_id,
        )));

        assert_corruption(store.verify_readable());
    }

    /// A flipped byte in an index subtree leaves the data records intact but clobbers an
    /// index cell. The structural index walk must surface that as corruption so an index
    /// lookup is never blessed while silently under-reading.
    #[test]
    fn verify_index_readable_rejects_a_corrupt_index_cell() {
        let index = catalog("cat_00000000000000000000000000000003");
        let store = TreeStore::from_backend(Box::new(FaultingBackend::corrupt_index_cell(&index)));

        assert_corruption(store.verify_index_readable());
        // The same corruption surfaces through the full readable check recover runs.
        assert_corruption(store.verify_readable());
    }

    /// A non-unique index repeats a record's identity as the trailing suffix of its ordered key
    /// tuple. A single-bit flip diverging the stored identity from that suffix is the corruption
    /// the runtime read path also refuses per entry; the store-open witness must condemn it at
    /// open — never leaving a read to surface it later as a typed program fault at an innocent
    /// source span — while a healthy sibling entry still verifies clean.
    #[test]
    fn verify_index_readable_rejects_a_non_unique_entry_whose_identity_diverges_from_its_tuple_suffix()
     {
        let index = catalog("cat_00000000000000000000000000000003");
        let store = TreeStore::memory();
        store
            .write_index_entry(
                &index,
                &[SavedKey::Str("fiction".into()), SavedKey::Int(1)],
                &[SavedKey::Int(1)],
                INDEX_MARKER.to_vec(),
            )
            .expect("write healthy index entry");
        store
            .verify_index_readable()
            .expect("a production-shaped index entry verifies clean");

        // The stored identity ([2]) diverges from the tuple's identity suffix ([1]) the way a
        // single-bit flip inside a committed entry does. Both the index witness and the full
        // readable check recover runs must fail closed.
        store
            .write_index_entry(
                &index,
                &[SavedKey::Str("fiction".into()), SavedKey::Int(1)],
                &[SavedKey::Int(2)],
                INDEX_MARKER.to_vec(),
            )
            .expect("write divergent index entry");
        assert_corruption(store.verify_index_readable());
        assert_corruption(store.verify_readable());
    }

    /// A unique index does not repeat the identity in its key tuple — the tuple is the unique key,
    /// with the record identity carried in the cell value. The store-open witness reconciles the
    /// stored identity against that value copy, so a healthy unique entry verifies clean while a
    /// flip diverging the value from the stored identity fails closed.
    #[test]
    fn verify_index_readable_reconciles_a_unique_entry_identity_against_its_value() {
        let index = catalog("cat_00000000000000000000000000000004");
        let store = TreeStore::memory();
        // A unique byIsbn entry: tuple [isbn], identity [id], value carrying the identity.
        store
            .write_index_entry(
                &index,
                &[SavedKey::Str("978-1".into())],
                &[SavedKey::Int(7)],
                encode_identity_payload(&[SavedKey::Int(7)]),
            )
            .expect("write healthy unique index entry");
        store
            .verify_index_readable()
            .expect("a unique index entry whose value carries its identity verifies clean");

        // Flip the value so it no longer carries the stored identity: the value now encodes [8]
        // while the entry identity is [7]. The witness must fail closed.
        store
            .write_index_entry(
                &index,
                &[SavedKey::Str("978-2".into())],
                &[SavedKey::Int(9)],
                encode_identity_payload(&[SavedKey::Int(8)]),
            )
            .expect("write divergent unique index entry");
        assert_corruption(store.verify_index_readable());
        assert_corruption(store.verify_readable());
    }

    /// A unique index may include the identity column in its key, so its tuple can byte-equal the
    /// stored identity — `index byCode(code) unique` on a record whose `code` equals its `id`. The
    /// value is still the read-authoritative identity copy, so a flip diverging the value from the
    /// stored identity must fail closed even though the tuple suffix coincidentally still matches;
    /// reconciling only the tuple here would bless a value corruption every unique read then faults.
    #[test]
    fn verify_index_readable_reconciles_a_unique_entry_whose_tuple_equals_its_identity() {
        let index = catalog("cat_00000000000000000000000000000005");
        let store = TreeStore::memory();
        // tuple [7] == identity [7]; the value carries the identity a unique read decodes.
        store
            .write_index_entry(
                &index,
                &[SavedKey::Int(7)],
                &[SavedKey::Int(7)],
                encode_identity_payload(&[SavedKey::Int(7)]),
            )
            .expect("write healthy unique index entry whose tuple equals its identity");
        store
            .verify_index_readable()
            .expect("a unique entry whose tuple equals its identity verifies clean");

        // Flip only the value payload to [8] while tuple and stored identity stay [7]. The tuple
        // suffix still matches, so an OR check would bless it, but the value copy reads consume is
        // corrupt: the witness must fail closed.
        store
            .write_index_entry(
                &index,
                &[SavedKey::Int(9)],
                &[SavedKey::Int(9)],
                encode_identity_payload(&[SavedKey::Int(8)]),
            )
            .expect("write divergent unique index entry whose tuple equals its stored identity");
        assert_corruption(store.verify_index_readable());
        assert_corruption(store.verify_readable());
    }

    /// A healthy store with index entries verifies, and every index identity is reachable
    /// through the seek-driven re-descent the verification runs.
    #[test]
    fn verify_index_readable_accepts_a_healthy_index() {
        let index = catalog("cat_00000000000000000000000000000003");
        let store = TreeStore::memory();
        let identities = [1, 2, 3];
        for id in identities {
            store
                .write_index_entry(
                    &index,
                    &[SavedKey::Str("shelf".into()), SavedKey::Int(id)],
                    &[SavedKey::Int(id)],
                    INDEX_MARKER.to_vec(),
                )
                .expect("write index entry");
        }

        store
            .verify_index_readable()
            .expect("healthy index verifies");

        let range = IndexRangeBounds {
            lower: None,
            lower_inclusive: true,
            upper: None,
            upper_inclusive: true,
        };
        let page = store
            .scan_index_range(&index, &[SavedKey::Str("shelf".into())], &range, 16)
            .expect("scan index range");
        assert_eq!(
            page.entries.len(),
            identities.len(),
            "every index identity is returned: {page:?}"
        );
    }

    /// An interior-separator flip can misroute a bounded index seek past a committed index
    /// entry a range scan still yields: the entry reads absent through the point-lookup descent
    /// while the linear scan and the schema-driven completeness count still cover it, so an index
    /// range read silently under-returns a contiguous subtree. The index store-open witness
    /// reconciles the point-lookup descent against the linear scan and fails closed, on many cells
    /// rather than one offset; a healthy index still verifies.
    #[test]
    fn a_seek_unreachable_index_cell_fails_the_store_open_witness() {
        let index = catalog("cat_00000000000000000000000000000003");
        let (backend, hidden) = FaultingBackend::seek_misroute();
        let store = TreeStore::from_backend(Box::new(backend));
        store.begin().expect("begin");
        for id in 1..=5 {
            store
                .write_index_entry(
                    &index,
                    &[SavedKey::Str("fiction".into()), SavedKey::Int(id)],
                    &[SavedKey::Int(id)],
                    INDEX_MARKER.to_vec(),
                )
                .expect("write index entry");
        }
        store.commit().expect("commit");
        store
            .verify_index_readable()
            .expect("a freshly committed index verifies clean");

        // Hide each committed index cell from point lookup one at a time, the way an interior
        // flip misroutes a seek while the range scan still yields the cell. Every one must fail
        // the index witness and the full readable check recover runs, even though the linear scan
        // and the seek-driven descent still traverse the intact bytes.
        for id in [1i64, 3, 5] {
            let hidden_key = CellKey::index(
                &index,
                &[SavedKey::Str("fiction".into()), SavedKey::Int(id)],
                &[SavedKey::Int(id)],
            )
            .into_bytes();
            hidden.borrow_mut().insert(hidden_key);
            assert_corruption(store.verify_index_readable());
            assert_corruption(store.verify_readable());
            hidden.borrow_mut().clear();
        }

        // With nothing hidden the index verifies clean again: the witness condemns only an
        // unreachable committed entry, never a healthy one.
        store
            .verify_index_readable()
            .expect("the index verifies clean once every entry is reachable again");
        store
            .verify_readable()
            .expect("the store is readable once every entry is reachable again");
    }

    /// The index-family walk pages the same way the data-family walk does. A backend that
    /// reports the same final index key with `truncated: true` on every page must fail
    /// closed as corruption rather than re-querying the same cursor forever.
    #[test]
    fn index_walk_rejects_a_non_advancing_page_cursor() {
        let index = catalog("cat_00000000000000000000000000000003");
        let result = assert_terminates(move || {
            let store = TreeStore::from_backend(Box::new(
                FaultingBackend::non_advancing_index_page(&index),
            ));
            store.verify_index_readable()
        });
        assert_corruption(result);
    }

    /// Run `body` on a worker thread with a deadline so a page-cursor spin surfaces as
    /// a test timeout rather than hanging the suite, and a fixed result is returned.
    fn assert_terminates<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
        use std::sync::mpsc;
        use std::time::Duration;

        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(body());
        });
        match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => panic!("store traversal did not terminate on damaged page cursor"),
        }
    }

    /// `data integrity`/`stats`/`dump` drive the data-family page walk in `visit_family`.
    /// A backend that reports the same final key with `truncated: true` on every page must
    /// fail closed as corruption rather than re-querying the same cursor forever.
    #[test]
    fn family_walk_rejects_a_non_advancing_page_cursor() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let result = assert_terminates(move || {
            let store = TreeStore::from_backend(Box::new(
                FaultingBackend::non_advancing_scan_page(&store_id),
            ));
            store.verify_readable()
        });
        assert_corruption(result);
    }

    /// The record-seek inner loop (`scan_record_children_after_cursor`) resumes from
    /// the last page key the same way. A truncated page whose only entry the scan skips
    /// leaves the cursor unmoved; that must fail closed, not spin.
    #[test]
    fn record_seek_rejects_a_non_advancing_page_cursor() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let result = assert_terminates(move || {
            let store = TreeStore::from_backend(Box::new(
                FaultingBackend::non_advancing_record_page(&store_id),
            ));
            store.record_next_child(&store_id, &[SavedKey::Int(5)], &SavedKey::Int(0))
        });
        assert_corruption(result);
    }

    #[test]
    fn visit_backup_cells_reports_empty_truncated_page_as_invalid_cursor() {
        let store =
            TreeStore::from_backend(Box::new(FaultingBackend::empty_truncated_backup_scan()));

        let error = store
            .visit_backup_cells(|_| Ok(()))
            .expect_err("empty truncated data-family scan should return an error");

        assert!(matches!(error, StoreError::InvalidCursor { .. }));
        assert_eq!(error.code(), "store.cursor");
        let StoreError::InvalidCursor { message } = &error else {
            unreachable!("matched above");
        };
        assert!(
            message.contains("data") && !message.contains("backup"),
            "a live-store scan names a data scan, not a backup scan: {message}"
        );
    }

    #[test]
    fn exact_index_scan_reports_empty_truncated_page_as_invalid_cursor() {
        let index = catalog("cat_00000000000000000000000000000001");
        let keys = [SavedKey::Str("fiction".into())];
        let prefix = CellKey::index_tuple_prefix(&index, &keys);
        let store = TreeStore::from_backend(Box::new(FaultingBackend::empty_truncated_scan(
            EmptyTruncatedScanMethod::Scan,
            prefix.as_bytes(),
        )));

        let error = store
            .scan_index_tuple(&index, &keys, 1)
            .expect_err("empty truncated exact index scan should return an error");

        assert!(matches!(error, StoreError::InvalidCursor { .. }));
        assert_eq!(error.code(), "store.cursor");
    }

    #[test]
    fn resumed_exact_index_scan_reports_empty_truncated_page_as_invalid_cursor() {
        let index = catalog("cat_00000000000000000000000000000001");
        let keys = [SavedKey::Str("fiction".into())];
        let prefix = CellKey::index_tuple_prefix(&index, &keys);
        let cursor = IndexCursor {
            prefix: prefix.as_bytes().to_vec(),
            last_key: prefix.as_bytes().to_vec(),
            scope: IndexCursorScope::Exact,
        };
        let store = TreeStore::from_backend(Box::new(FaultingBackend::empty_truncated_scan(
            EmptyTruncatedScanMethod::ScanAfter,
            prefix.as_bytes(),
        )));

        let error = store
            .scan_index_tuple_after(&index, &keys, &cursor, 1)
            .expect_err("empty truncated resumed exact index scan should return an error");

        assert!(matches!(error, StoreError::InvalidCursor { .. }));
        assert_eq!(error.code(), "store.cursor");
    }

    #[test]
    fn bounded_index_range_reports_empty_truncated_page_as_invalid_cursor() {
        let index = catalog("cat_00000000000000000000000000000001");
        let prefix = CellKey::index_key_prefix(&index, &[]);
        let range = IndexRangeBounds {
            lower: Some(SavedKey::Int(10)),
            lower_inclusive: true,
            upper: Some(SavedKey::Int(30)),
            upper_inclusive: false,
        };
        let store = TreeStore::from_backend(Box::new(FaultingBackend::empty_truncated_scan(
            EmptyTruncatedScanMethod::ScanBetween,
            prefix.as_bytes(),
        )));

        let error = store
            .scan_index_range(&index, &[], &range, 1)
            .expect_err("empty truncated bounded index scan should return an error");

        assert!(matches!(error, StoreError::InvalidCursor { .. }));
        assert_eq!(error.code(), "store.cursor");
    }

    #[test]
    fn bounded_index_range_lower_exclusive_skips_equal_range_key_entries() {
        let index = catalog("cat_00000000000000000000000000000001");
        let store = TreeStore::memory();
        store
            .write_index_entry(
                &index,
                &[
                    SavedKey::Str("fiction".into()),
                    SavedKey::Int(10),
                    SavedKey::Int(1),
                ],
                &[SavedKey::Int(1)],
                INDEX_MARKER.to_vec(),
            )
            .expect("write lower equal entry");
        store
            .write_index_entry(
                &index,
                &[
                    SavedKey::Str("fiction".into()),
                    SavedKey::Int(11),
                    SavedKey::Int(2),
                ],
                &[SavedKey::Int(2)],
                INDEX_MARKER.to_vec(),
            )
            .expect("write greater entry");
        let range = IndexRangeBounds {
            lower: Some(SavedKey::Int(10)),
            lower_inclusive: false,
            upper: Some(SavedKey::Int(11)),
            upper_inclusive: true,
        };

        let page = store
            .scan_index_range(&index, &[SavedKey::Str("fiction".into())], &range, 10)
            .expect("exclusive lower range scan");

        assert_eq!(page.entries.len(), 1, "{page:#?}");
        assert_eq!(page.entries[0].identity, vec![SavedKey::Int(2)]);
    }

    #[test]
    fn reverse_bounded_index_range_reports_empty_truncated_page_as_invalid_cursor() {
        let index = catalog("cat_00000000000000000000000000000001");
        let prefix = CellKey::index_key_prefix(&index, &[]);
        let range = IndexRangeBounds {
            lower: Some(SavedKey::Int(10)),
            lower_inclusive: true,
            upper: Some(SavedKey::Int(30)),
            upper_inclusive: false,
        };
        let store = TreeStore::from_backend(Box::new(FaultingBackend::empty_truncated_scan(
            EmptyTruncatedScanMethod::ScanBetweenBefore,
            prefix.as_bytes(),
        )));

        let error = store
            .scan_index_range_reverse(&index, &[], &range, 1)
            .expect_err("empty truncated reverse bounded index scan should return an error");

        assert!(matches!(error, StoreError::InvalidCursor { .. }));
        assert_eq!(error.code(), "store.cursor");
    }

    /// A storage fault part-way through a staged transaction rolls the whole bracket
    /// back: a write that succeeded before the fault must not survive, and no metadata
    /// stamp may land. This is the atomic guarantee evolution apply relies on when it
    /// commits backfills and the metadata stamp together; a read-only store fails at
    /// `begin`, so only a mid-plan fault proves the rollback covers committed writes.
    #[test]
    fn a_mid_transaction_write_fault_rolls_the_whole_bracket_back() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let path = [DataPathSegment::Member(member)];
        // The fault strikes on the second write, so the first data write lands in the
        // buffer before the bracket aborts.
        let store = TreeStore::from_backend(Box::new(FaultingBackend::fail_on_nth_write(1)));

        let before = store.read_commit_metadata().expect("read commit");
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
            store.read_commit_metadata().expect("read commit"),
            None,
            "no metadata stamp may land when the plan aborts"
        );
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
    fn commit_metadata_rejects_truncated_stamp_lists() {
        let metadata = CommitMetadata {
            commit_id: 7,
            catalog_epoch: 3,
            layout_epoch: 0,
            source_digest:
                "sha256:00000000000000000000000000000000000000000000000000000000deadbeef"
                    .to_string(),
            engine_profile_digest: [1, 2, 3, 4, 5, 6, 7, 8],
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
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
                .write_record_presence(&store_id, &[SavedKey::Int(id)])
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
            store
                .write_record_presence(&store_id, identity)
                .expect("seed record");
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

    #[test]
    fn record_last_child_uses_one_bounded_scan_not_all_pages() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let counts = BackendCounts::default();
        let store = TreeStore::from_backend(Box::new(CountingBackend::new(
            MemStore::default(),
            counts.clone(),
        )));
        for id in 0..257 {
            store
                .write_record_presence(&store_id, &[SavedKey::Int(id)])
                .expect("seed record");
        }

        counts.reset();
        let last = store
            .record_last_child_at_arity(&store_id, &[], 1)
            .expect("last child");

        assert_eq!(last, Some(SavedKey::Int(256)));
        assert_eq!(
            counts.total_scans(),
            1,
            "last-child lookup should be one bounded seek/reverse operation, not a full \
             forward scan over every prefix page"
        );
    }

    #[test]
    fn for_each_record_moves_linear_entries_not_repeated_prefix_pages() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let counts = BackendCounts::default();
        let store = TreeStore::from_backend(Box::new(CountingBackend::new(
            MemStore::default(),
            counts.clone(),
        )));
        let record_count = 257usize;
        for id in 0..record_count {
            store
                .write_record_presence(&store_id, &[SavedKey::Int(id as i64)])
                .expect("seed record");
        }

        let mut visited = 0usize;
        counts.reset();
        store
            .for_each_record(&store_id, 1, &mut |_| {
                visited += 1;
                Ok(())
            })
            .expect("iterate records");

        assert_eq!(visited, record_count);
        assert!(
            counts.entries_returned() <= record_count + super::CHILD_SCAN_PAGE_LIMIT + 2,
            "record iteration should move O(n) entries; moved {} entries for {record_count} \
             records",
            counts.entries_returned()
        );
    }

    #[test]
    fn max_int_record_child_uses_one_int_band_reverse_seek() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let counts = BackendCounts::default();
        let store = TreeStore::from_backend(Box::new(CountingBackend::new(
            MemStore::default(),
            counts.clone(),
        )));
        for id in 0..257 {
            store
                .write_record_presence(&store_id, &[SavedKey::Int(id)])
                .expect("seed int record");
        }
        store
            .write_record_presence(&store_id, &[SavedKey::Str("later type band".into())])
            .expect("seed non-int record");

        counts.reset();
        let max = store
            .max_int_record_child(&store_id, &[])
            .expect("max int record child");

        assert_eq!(max, Some(256));
        assert_eq!(
            counts.total_scans(),
            1,
            "max int record lookup should seek inside the int key band"
        );
        assert!(
            counts.entries_returned() <= super::CHILD_SCAN_PAGE_LIMIT,
            "max int record lookup should not move all children; moved {} entries",
            counts.entries_returned()
        );
    }

    #[test]
    fn max_int_data_child_uses_one_int_band_reverse_seek() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let identity = [SavedKey::Int(1)];
        let path = [DataPathSegment::Member(member)];
        let counts = BackendCounts::default();
        let store = TreeStore::from_backend(Box::new(CountingBackend::new(
            MemStore::default(),
            counts.clone(),
        )));
        for id in 0..257 {
            let mut child_path = path.to_vec();
            child_path.push(DataPathSegment::Key(SavedKey::Int(id)));
            store
                .write_data_value(&store_id, &identity, &child_path, b"value".to_vec())
                .expect("seed int data child");
        }
        let mut str_path = path.to_vec();
        str_path.push(DataPathSegment::Key(SavedKey::Str(
            "later type band".into(),
        )));
        store
            .write_data_value(&store_id, &identity, &str_path, b"value".to_vec())
            .expect("seed non-int data child");

        counts.reset();
        let max = store
            .max_int_data_child(&store_id, &identity, &path)
            .expect("max int data child");

        assert_eq!(max, Some(256));
        assert_eq!(
            counts.total_scans(),
            1,
            "max int data lookup should seek inside the int key band"
        );
        assert!(
            counts.entries_returned() <= super::CHILD_SCAN_PAGE_LIMIT,
            "max int data lookup should not move all children; moved {} entries",
            counts.entries_returned()
        );
    }

    #[test]
    fn linear_navigation_scale_smoke() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let counts = BackendCounts::default();
        let store = TreeStore::from_backend(Box::new(CountingBackend::new(
            MemStore::default(),
            counts.clone(),
        )));
        let record_count = 4096usize;
        for id in 0..record_count {
            store
                .write_record_presence(&store_id, &[SavedKey::Int(id as i64)])
                .expect("seed scale record");
        }

        counts.reset();
        assert_eq!(
            store
                .record_last_child_at_arity(&store_id, &[], 1)
                .expect("last child"),
            Some(SavedKey::Int((record_count - 1) as i64))
        );
        assert_eq!(counts.total_scans(), 1);

        counts.reset();
        assert_eq!(
            store
                .max_int_record_child(&store_id, &[])
                .expect("max int child"),
            Some((record_count - 1) as i64)
        );
        assert_eq!(counts.total_scans(), 1);

        counts.reset();
        let mut visited = 0usize;
        store
            .for_each_record(&store_id, 1, &mut |_| {
                visited += 1;
                Ok(())
            })
            .expect("iterate records");
        assert_eq!(visited, record_count);
        assert!(
            counts.entries_returned() <= record_count + super::CHILD_SCAN_PAGE_LIMIT + 2,
            "scale iteration should move O(n) entries; moved {} entries for {record_count} \
             records",
            counts.entries_returned()
        );
    }

    fn collect_backup_cells(store: &TreeStore) -> Vec<TreeBackupCellBuf> {
        let mut cells = Vec::new();
        store
            .visit_backup_cells(|cell| {
                cells.push(TreeBackupCellBuf::from_cell(cell));
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

    #[test]
    fn data_path_node_marks_presence_without_payload_bytes() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let identity = [SavedKey::Int(1)];
        let member_path = [DataPathSegment::Member(member.clone())];
        let entry_path = [
            DataPathSegment::Member(member),
            DataPathSegment::Key(SavedKey::Int(7)),
        ];
        let source = TreeStore::memory();
        source
            .write_record_presence(&store_id, &identity)
            .expect("write record presence");
        source
            .write_data_node(&store_id, &identity, &entry_path)
            .expect("write data path node");

        assert!(
            source
                .data_subtree_exists(&store_id, &identity, &entry_path)
                .expect("entry exists")
        );
        assert_eq!(
            source
                .read_data_value(&store_id, &identity, &entry_path)
                .expect("read data value"),
            None
        );
        assert_eq!(
            source
                .data_first_child(&store_id, &identity, &member_path)
                .expect("first child"),
            Some(SavedKey::Int(7))
        );

        let cells = collect_backup_cells(&source);
        assert!(cells.iter().any(|cell| {
            matches!(
                cell.data_key().kind,
                DataCellKind::PathNode { ref path } if path.as_slice() == entry_path
            ) && cell.value() == NODE_MARKER
        }));

        let restored = TreeStore::memory();
        for cell in &cells {
            replay_backup_cell(&restored, cell).expect("restore cell");
        }
        assert!(
            restored
                .data_subtree_exists(&store_id, &identity, &entry_path)
                .expect("restored entry exists")
        );
        assert_eq!(
            restored
                .read_data_value(&store_id, &identity, &entry_path)
                .expect("read restored value"),
            None
        );
    }

    #[test]
    fn read_data_value_prefix_moves_only_requested_value_bytes() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let identity = [SavedKey::Int(1)];
        let path = [DataPathSegment::Member(member)];
        let counts = BackendCounts::default();
        let store = TreeStore::from_backend(Box::new(CountingBackend::new(
            MemStore::default(),
            counts.clone(),
        )));
        let value = vec![b'x'; 4096];
        store
            .write_data_value(&store_id, &identity, &path, value)
            .expect("write value");

        counts.reset();
        let prefix = store
            .read_data_value_prefix(&store_id, &identity, &path, 16)
            .expect("prefix read")
            .expect("stored value");

        assert_eq!(prefix.bytes, vec![b'x'; 16]);
        assert!(prefix.truncated);
        assert!(
            counts.bytes_moved() < 512,
            "prefix read should account key plus copied prefix bytes, not the full value: {}",
            counts.bytes_moved()
        );
    }

    /// A backup carries every data-family cell and nothing else, and replaying that
    /// stream into a fresh store reproduces it byte-for-byte. Index cells are derived
    /// and rebuilt on restore, so they stay out of the stream; commit metadata stays
    /// out because restore restamps it from the manifest.
    #[test]
    fn backup_cells_round_trip_and_exclude_index_and_meta() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let title = catalog("cat_00000000000000000000000000000002");
        let index = catalog("cat_00000000000000000000000000000003");
        let path = [DataPathSegment::Member(title.clone())];

        let source = TreeStore::memory();
        source
            .write_record_presence(&store_id, &[SavedKey::Int(1)])
            .expect("write record presence");
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
                INDEX_MARKER.to_vec(),
            )
            .expect("write index");
        // A meta stamp that the backup stream must not carry.
        source
            .write_commit_metadata(&empty_commit_metadata(4))
            .expect("stamp commit metadata");

        let cells = collect_backup_cells(&source);
        assert!(!cells.is_empty(), "the populated store has backup cells");
        assert!(
            cells
                .iter()
                .all(|cell| cell.data_key().store.as_str() == store_id.as_str()),
            "the backup stream carries only data-family cells: {cells:?}"
        );

        let restored = TreeStore::memory();
        assert!(restored.is_empty().expect("fresh store is empty"));
        for cell in &cells {
            replay_backup_cell(&restored, cell).expect("restore cell");
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
        // The commit metadata cell was never part of the stream.
        assert_eq!(restored.read_commit_metadata().expect("read commit"), None);
    }

    #[test]
    fn backup_cell_rejects_a_meta_key() {
        // A meta-family key is not a restorable backup cell.
        let meta_key = CellKey::meta(super::MetaCell::Commit);
        assert_corruption(TreeBackupCellBuf::from_raw(
            meta_key.into_bytes(),
            b"4".to_vec(),
        ));
    }

    #[test]
    fn backup_cell_rejects_an_index_key() {
        // An index-family cell is derived and rebuilt on restore; a backup never
        // carries one, so replaying an index key is a malformed backup.
        let mut index_key = CellKey::index_family().as_bytes().to_vec();
        index_key.extend_from_slice(b"entry");
        assert_corruption(TreeBackupCellBuf::from_raw(index_key, b"1".to_vec()));
    }

    /// A torn on-disk data cell surfaces through the shared data-family walk that
    /// backs `data integrity`/`get`/`stats`/`dump`/`roots` and `run`. The corruption
    /// prose must name a stored data cell; no archive is involved, so it must not
    /// leak the word "backup".
    #[test]
    fn torn_live_store_data_cell_names_a_data_cell_not_a_backup() {
        let (store, backend, _root) = seeded_digest_store();
        let mut torn = CellKey::data_family().into_bytes();
        torn.extend_from_slice(b"\xff\xff");
        backend.tamper(|map| {
            map.insert(torn, b"x".to_vec());
        });
        let message = corruption_message(store.visit_backup_cells(|_| Ok(())));
        assert!(
            message.contains("data cell") && !message.contains("backup"),
            "a live-store decode failure names a data cell, not a backup cell: {message}"
        );
    }

    /// A genuinely malformed backup archive cell keeps the backup-domain wording:
    /// here an archive frame really is at fault.
    #[test]
    fn corrupt_backup_archive_frame_names_a_backup_cell() {
        let mut archive = Vec::new();
        archive.extend_from_slice(&1u32.to_be_bytes());
        archive.push(0xff);
        let error =
            TreeBackupCellBuf::read_framed(&mut archive.as_slice(), TREE_BACKUP_MAX_CELL_BYTES)
                .expect_err("a malformed target frame is rejected");
        assert_eq!(error, TreeBackupCellReadError::MalformedCell);
        assert!(
            error.to_string().contains("backup cell"),
            "a backup archive fault names a backup cell: {error}"
        );
    }

    /// A pinned read snapshot owns the handle's read view, so the same handle cannot
    /// publish writes until the snapshot is released.
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
            let error = store
                .write_data_value(&store_id, &[SavedKey::Int(2)], &path, b"second".to_vec())
                .expect_err("write is rejected while snapshot is pinned");
            assert_eq!(error.code(), "store.transaction");
            collect_backup_cells(&store)
        };
        assert_eq!(before.len(), 1, "snapshot still reads the existing cell");

        store
            .write_data_value(&store_id, &[SavedKey::Int(2)], &path, b"second".to_vec())
            .expect("write after snapshot");
        assert_eq!(collect_backup_cells(&store).len(), 2);
    }

    fn sample_catalog() -> marrow_catalog::CatalogMetadata {
        marrow_catalog::CatalogMetadata::new(
            1,
            vec![marrow_catalog::CatalogEntry {
                kind: marrow_catalog::CatalogEntryKind::Store,
                path: "books".to_string(),
                stable_id: "cat_00000000000000000000000000000009".to_string(),
                aliases: Vec::new(),
                lifecycle: marrow_catalog::CatalogLifecycle::Active,
                accepted_key_shape: Some("int".to_string()),
                accepted_index_shape: None,
                accepted_struct: None,
                applied_transform: None,
            }],
        )
        .expect("catalog builds")
    }

    /// A rollback covers the catalog table and data together: a transaction that
    /// publishes a catalog snapshot and writes a data cell, then rolls back, leaves
    /// both at their pre-transaction state. Commit advances both atomically, so the
    /// catalog and the backfills evolution apply stages always move as one.
    #[test]
    fn rollback_reverts_the_catalog_and_data_together() {
        let store_id = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_00000000000000000000000000000002");
        let path = [DataPathSegment::Member(member)];
        let store = TreeStore::memory();

        store.begin().expect("begin");
        store
            .write_data_value(&store_id, &[SavedKey::Int(1)], &path, b"first".to_vec())
            .expect("write data");
        store
            .replace_catalog_snapshot(&sample_catalog())
            .expect("publish catalog");
        store.rollback().expect("rollback");

        assert_eq!(
            store
                .read_data_value(&store_id, &[SavedKey::Int(1)], &path)
                .expect("read data"),
            None,
            "the data write must not survive the rollback"
        );
        assert_eq!(
            store.read_catalog_snapshot().expect("read catalog"),
            None,
            "the catalog snapshot must not survive the rollback"
        );

        // The same plan committed lands both together.
        store.begin().expect("begin commit path");
        store
            .write_data_value(&store_id, &[SavedKey::Int(1)], &path, b"first".to_vec())
            .expect("write data");
        store
            .replace_catalog_snapshot(&sample_catalog())
            .expect("publish catalog");
        store.commit().expect("commit");
        assert_eq!(
            store
                .read_data_value(&store_id, &[SavedKey::Int(1)], &path)
                .expect("read data"),
            Some(b"first".to_vec())
        );
        assert_eq!(
            store.read_catalog_snapshot().expect("read catalog"),
            Some(sample_catalog())
        );
    }

    /// Replacing the catalog is one transaction even with no caller transaction
    /// open, so a replace that is later undone restores the exact prior catalog
    /// rather than the half-written or empty state a non-atomic delete-then-write
    /// would leave. A standalone first publish commits durably; a second replace
    /// inside a transaction is visible within it but a rollback brings the first
    /// catalog back in full.
    #[test]
    fn a_rolled_back_replace_restores_the_prior_catalog() {
        let store = TreeStore::memory();
        let first = sample_catalog();
        store
            .replace_catalog_snapshot(&first)
            .expect("publish the first catalog with no open transaction");
        assert_eq!(
            store.read_catalog_snapshot().expect("read first"),
            Some(first.clone()),
            "a standalone replace must commit on its own"
        );

        let second = marrow_catalog::CatalogMetadata::new(
            2,
            vec![marrow_catalog::CatalogEntry {
                kind: marrow_catalog::CatalogEntryKind::Store,
                path: "authors".to_string(),
                stable_id: "cat_00000000000000000000000000000042".to_string(),
                aliases: Vec::new(),
                lifecycle: marrow_catalog::CatalogLifecycle::Active,
                accepted_key_shape: Some("int".to_string()),
                accepted_index_shape: None,
                accepted_struct: None,
                applied_transform: None,
            }],
        )
        .expect("catalog builds");
        store.begin().expect("begin");
        store
            .replace_catalog_snapshot(&second)
            .expect("replace inside the transaction");
        assert_eq!(
            store.read_catalog_snapshot().expect("read second"),
            Some(second),
            "the replacement is visible inside the open transaction"
        );
        store.rollback().expect("rollback");

        assert_eq!(
            store.read_catalog_snapshot().expect("read after rollback"),
            Some(first),
            "the rollback restores the prior catalog, not an empty or partial one"
        );
    }

    /// The public catalog read path fails closed when a well-formed entry row exists
    /// with no header row to anchor it. The read must reject the catalog rather than
    /// reconstruct a headerless snapshot, surfaced through `TreeStore`'s public API.
    #[test]
    fn the_public_catalog_read_rejects_an_entry_row_without_a_header() {
        let store = TreeStore::memory();
        // A fully valid entry row value: version, ordinal 0, a store-kind tag, a path,
        // zero aliases, an active lifecycle, and no optional fields. The only thing
        // wrong is the absent header, so corruption can come from nothing else.
        let mut value = vec![0x00];
        value.extend_from_slice(&0u64.to_be_bytes());
        value.push(0); // CatalogEntryKind::Store tag
        value.extend_from_slice(&4u32.to_be_bytes());
        value.extend_from_slice(b"book");
        value.extend_from_slice(&0u32.to_be_bytes()); // no aliases
        value.push(0); // CatalogLifecycle::Active tag
        value.push(0); // no accepted_key_shape
        value.push(0); // no accepted_struct
        value.push(0); // no accepted_index_shape

        let mut key_tail = vec![0x10]; // entry-row tag
        key_tail.extend_from_slice(b"cat_00000000000000000000000000000009");
        let mut key = CellKey::catalog_family().into_bytes();
        key.extend_from_slice(&key_tail);
        store
            .write_cell(&key, value)
            .expect("seed headerless catalog entry row");

        assert_corruption(store.read_catalog_snapshot());
        assert_corruption(store.catalog_snapshot_digest());
    }

    fn catalog(raw: &str) -> CatalogId {
        CatalogId::new(raw.to_string()).expect("valid catalog id")
    }

    fn replay_backup_cell(store: &TreeStore, cell: &TreeBackupCellBuf) -> Result<(), StoreError> {
        let target = cell.data_key();
        match &target.kind {
            DataCellKind::Node => store.write_record_presence(&target.store, &target.identity),
            DataCellKind::PathNode { path } => {
                store.write_data_node(&target.store, &target.identity, path)
            }
            DataCellKind::Leaf { member } => store.write_leaf(
                &target.store,
                &target.identity,
                member,
                cell.value().to_vec(),
            ),
            DataCellKind::Sequence { member, position } => store.write_sequence_position(
                &target.store,
                &target.identity,
                member,
                *position,
                cell.value().to_vec(),
            ),
            DataCellKind::Value { path } => {
                store.write_data_value(&target.store, &target.identity, path, cell.value().to_vec())
            }
        }
    }

    fn assert_corruption<T>(result: Result<T, StoreError>) {
        assert!(matches!(result, Err(StoreError::Corruption { .. })));
    }

    fn corruption_message<T: std::fmt::Debug>(result: Result<T, StoreError>) -> String {
        match result {
            Err(StoreError::Corruption { message }) => message,
            other => panic!("expected corruption, got {other:?}"),
        }
    }

    /// A `MemStore` shared between a `TreeStore` and the test, so the test can tamper the
    /// committed bytes out of band after the store has stamped its digest, modelling a
    /// backend corruption the store cannot see at write time.
    #[derive(Clone, Default)]
    struct SharedMem(std::rc::Rc<std::cell::RefCell<MemStore>>);

    impl SharedMem {
        fn tamper(&self, mutate: impl FnOnce(&mut std::collections::BTreeMap<Vec<u8>, Vec<u8>>)) {
            self.0.borrow_mut().tamper(mutate);
        }
    }

    impl Backend for SharedMem {
        fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
            self.0.borrow().read(key)
        }
        fn read_prefix(&self, key: &[u8], limit: usize) -> Result<Option<ValuePrefix>, StoreError> {
            self.0.borrow().read_prefix(key, limit)
        }
        fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
            self.0.borrow().require_write_access(op)
        }
        fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
            self.0.borrow_mut().write(key, value)
        }
        fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError> {
            self.0.borrow_mut().delete(prefix)
        }
        fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
            self.0.borrow().scan(prefix, limit)
        }
        fn scan_after(
            &self,
            prefix: &[u8],
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.0.borrow().scan_after(prefix, cursor, limit)
        }
        fn scan_between(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.0.borrow().scan_between(prefix, lower, upper, limit)
        }
        fn scan_between_after(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.0
                .borrow()
                .scan_between_after(prefix, lower, upper, cursor, limit)
        }
        fn scan_before(
            &self,
            prefix: &[u8],
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.0.borrow().scan_before(prefix, cursor, limit)
        }
        fn scan_between_before(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.0
                .borrow()
                .scan_between_before(prefix, lower, upper, cursor, limit)
        }
        fn begin(&mut self) -> Result<(), StoreError> {
            self.0.borrow_mut().begin()
        }
        fn commit(&mut self) -> Result<(), StoreError> {
            self.0.borrow_mut().commit()
        }
        fn rollback(&mut self) -> Result<(), StoreError> {
            self.0.borrow_mut().rollback()
        }
        fn transaction_depth(&self) -> usize {
            self.0.borrow().transaction_depth()
        }
        fn begin_snapshot(&mut self) -> Result<(), StoreError> {
            self.0.borrow_mut().begin_snapshot()
        }
        fn end_snapshot(&mut self) {
            self.0.borrow_mut().end_snapshot();
        }
    }

    /// Commit a few records under one root through the production write path, so the store
    /// stamps a structural digest, then return the store and a handle to tamper the bytes.
    fn seeded_digest_store() -> (TreeStore, SharedMem, CatalogId) {
        let backend = SharedMem::default();
        let store = TreeStore::from_backend(Box::new(backend.clone()));
        let root = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_000000000000000000000000000000aa");
        store.begin().expect("begin");
        for id in 1..=3 {
            let identity = [SavedKey::Int(id)];
            store
                .write_record_presence(&root, &identity)
                .expect("presence");
            store
                .write_data_value(
                    &root,
                    &identity,
                    &[DataPathSegment::Member(member.clone())],
                    format!("body-{id}").into_bytes(),
                )
                .expect("value");
        }
        store.commit().expect("commit");
        store
            .verify_structural_digests()
            .expect("a freshly committed store verifies clean");
        (store, backend, root)
    }

    /// One committed value cell dropped out of band leaves the record nodes intact, so the
    /// record and orphan passes read straight past it. The structural digest re-derived
    /// from the surviving cells no longer matches the stamp, so the cross-check fails
    /// closed — the completeness oracle a record count alone could not provide.
    #[test]
    fn a_dropped_value_cell_fails_the_structural_digest_check() {
        let (store, backend, root) = seeded_digest_store();
        let dropped = CellKey::data_path_value(
            &root,
            &[SavedKey::Int(2)],
            &[DataPathSegment::Member(catalog(
                "cat_000000000000000000000000000000aa",
            ))],
        );
        backend.tamper(|entries| {
            assert!(
                entries.remove(dropped.as_bytes()).is_some(),
                "the targeted value cell must exist before tampering"
            );
        });
        assert_corruption(store.verify_structural_digests());
    }

    /// A torn-but-decodable value — the same key, different bytes — keeps the record and
    /// cell counts unchanged, so a record-count anchor would bless it. The content-sensitive
    /// digest changes with the value, so the cross-check fails closed.
    #[test]
    fn a_torn_value_fails_the_structural_digest_check() {
        let (store, backend, root) = seeded_digest_store();
        let torn = CellKey::data_path_value(
            &root,
            &[SavedKey::Int(2)],
            &[DataPathSegment::Member(catalog(
                "cat_000000000000000000000000000000aa",
            ))],
        );
        backend.tamper(|entries| {
            let value = entries
                .get_mut(torn.as_bytes())
                .expect("the targeted value cell must exist before tampering");
            value[0] ^= 0xff;
        });
        assert_corruption(store.verify_structural_digests());
    }

    /// Unbracketed writes — each auto-committed at the backend with no open transaction —
    /// must still stamp the digest, so a store seeded by direct presence-then-value writes
    /// verifies clean. This is the shape the evolution test harness uses.
    #[test]
    fn unbracketed_auto_committed_writes_stamp_the_digest() {
        let store = TreeStore::memory();
        let root = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_000000000000000000000000000000aa");
        store
            .write_record_presence(&root, &[SavedKey::Int(1)])
            .expect("presence");
        store
            .write_data_value(
                &root,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(member)],
                b"Dune".to_vec(),
            )
            .expect("value");
        store
            .verify_structural_digests()
            .expect("auto-committed writes must stamp a matching digest");
    }

    /// The same unbracketed-write contract over the native redb backend, where each write
    /// auto-commits its own transaction rather than staying live in a map.
    #[cfg(feature = "native")]
    #[test]
    fn unbracketed_native_writes_stamp_the_digest() {
        let dir =
            std::env::temp_dir().join(format!("marrow-digest-native-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let store = TreeStore::open(&dir).expect("open native store");
        let root = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_000000000000000000000000000000aa");
        store
            .write_record_presence(&root, &[SavedKey::Int(1)])
            .expect("presence");
        let subtitle = catalog("cat_000000000000000000000000000000bb");
        store
            .write_data_value(
                &root,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(member)],
                b"Dune".to_vec(),
            )
            .expect("title");
        store
            .write_data_value(
                &root,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(subtitle)],
                b"Appendix".to_vec(),
            )
            .expect("subtitle");
        let write_result = store.verify_structural_digests();
        drop(store);
        let read_result = TreeStore::open_read_only(&dir)
            .and_then(|reopened| reopened.verify_structural_digests());
        let _ = std::fs::remove_file(&dir);
        write_result.expect("auto-committed native writes must stamp a matching digest");
        read_result.expect("a read-only reopen must also see a matching digest");
    }

    /// An unbracketed data mutation on a read-only handle is rejected with the mutation's own
    /// op before the digest-commit bracket opens, so it never leaves a half-open transaction
    /// for teardown to abort on, and the read-only handle drops cleanly.
    #[cfg(feature = "native")]
    #[test]
    fn unbracketed_writes_on_a_read_only_store_reject_without_panicking() {
        let dir = std::env::temp_dir().join(format!(
            "marrow-digest-readonly-{}.redb",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&dir);
        let root = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_000000000000000000000000000000aa");
        {
            let store = TreeStore::open(&dir).expect("open native store");
            store
                .write_record_presence(&root, &[SavedKey::Int(1)])
                .expect("presence");
            store
                .write_data_value(
                    &root,
                    &[SavedKey::Int(1)],
                    &[DataPathSegment::Member(member.clone())],
                    b"Dune".to_vec(),
                )
                .expect("value");
        }

        let store = TreeStore::open_read_only(&dir).expect("open read-only");
        assert!(matches!(
            store.write_data_value(
                &root,
                &[SavedKey::Int(2)],
                &[DataPathSegment::Member(member.clone())],
                b"Other".to_vec(),
            ),
            Err(StoreError::ReadOnly { op: "write" })
        ));
        assert!(matches!(
            store.delete_data_subtree(
                &root,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(member)],
            ),
            Err(StoreError::ReadOnly { op: "delete" })
        ));
        drop(store);
        let _ = std::fs::remove_file(&dir);
    }

    /// A legitimate overwrite and a legitimate delete each restamp the digest in their own
    /// commit, so a healthy store stays healthy: no false corruption after either.
    #[test]
    fn a_delete_and_an_overwrite_keep_the_structural_digest_exact() {
        let (store, _backend, root) = seeded_digest_store();
        let member = catalog("cat_000000000000000000000000000000aa");

        store.begin().expect("begin overwrite");
        store
            .write_data_value(
                &root,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(member.clone())],
                b"rewritten".to_vec(),
            )
            .expect("overwrite value");
        store.commit().expect("commit overwrite");
        store
            .verify_structural_digests()
            .expect("an overwrite must not false-corrupt the store");

        store.begin().expect("begin delete");
        store
            .delete_data_subtree(&root, &[SavedKey::Int(3)], &[])
            .expect("delete record");
        store.commit().expect("commit delete");
        store
            .verify_structural_digests()
            .expect("a delete must not false-corrupt the store");
    }

    /// An interior-separator flip can misroute a point lookup past a committed cell a range
    /// scan still yields: the cell reads absent though the raw scan and the structural digest
    /// still cover it, so a digest comparison alone blesses a committed cell no read can reach.
    /// The store-open witness reconciles the point-lookup descent against the raw scan and fails
    /// closed, and it does so for a hidden identity node or member value alike, on many cells
    /// rather than one offset.
    #[test]
    fn a_seek_unreachable_cell_fails_the_store_open_witness() {
        let root = catalog("cat_00000000000000000000000000000001");
        let member = catalog("cat_000000000000000000000000000000aa");
        let (backend, hidden) = FaultingBackend::seek_misroute();
        let store = TreeStore::from_backend(Box::new(backend));
        store.begin().expect("begin");
        for id in 1..=5 {
            let identity = [SavedKey::Int(id)];
            store
                .write_record_presence(&root, &identity)
                .expect("presence");
            store
                .write_data_value(
                    &root,
                    &identity,
                    &[DataPathSegment::Member(member.clone())],
                    format!("body-{id}").into_bytes(),
                )
                .expect("value");
        }
        store.commit().expect("commit");
        store
            .verify_structural_digests()
            .expect("a freshly committed store verifies clean");

        // Hide each committed cell from point lookup one at a time — member values and identity
        // nodes across several records — the way an interior flip misroutes a seek while the
        // range scan still yields the cell. Every one must fail both store-open witnesses.
        for id in [1i64, 3, 5] {
            for hidden_key in [
                CellKey::data_path_value(
                    &root,
                    &[SavedKey::Int(id)],
                    &[DataPathSegment::Member(member.clone())],
                )
                .into_bytes(),
                CellKey::node(&root, &[SavedKey::Int(id)]).into_bytes(),
            ] {
                hidden.borrow_mut().insert(hidden_key);
                assert_corruption(store.verify_structural_digests());
                assert_corruption(store.verify_readable());
                hidden.borrow_mut().clear();
            }
        }

        // With nothing hidden the store verifies clean again: the witness condemns only an
        // unreachable committed cell, never a healthy one.
        store
            .verify_structural_digests()
            .expect("the store verifies clean once every cell is reachable again");
        store
            .verify_readable()
            .expect("the store is readable once every cell is reachable again");
    }

    /// A present-but-undecodable cell in the catalog/meta family — the sealed commit record, the
    /// store uid, the accepted commit stamp, or a catalog snapshot row — is backend damage the
    /// runtime store-open refuses, so the store-open witness the admission ladder, `data integrity`,
    /// `data stats`, `data dump`, `backup`, and `data recover` share must refuse it too, regardless
    /// of who runs it or in what output format. This family sorts ahead of the data and carries no
    /// per-cell structural digest, so the data cells and their digests stay intact under the tamper;
    /// only a witness that re-decodes each cell catches it. The commit record binds the uid, epoch,
    /// catalog digest, active roots, and per-root digests under one content seal, so a flip of any
    /// bound byte breaks the seal, and a validly re-sealed record whose fields disagree with the
    /// auxiliary cells it binds fails the record-vs-cell check. A catalog row is caught at every
    /// byte offset because the read recomputes the snapshot digest from the stored rows. A healthy
    /// family still passes every witness.
    #[test]
    fn a_corrupt_catalog_or_meta_family_cell_fails_the_store_open_witness() {
        let backend = SharedMem::default();
        let store = TreeStore::from_backend(Box::new(backend.clone()));
        let root = catalog("cat_00000000000000000000000000000009");
        let member = catalog("cat_000000000000000000000000000000aa");
        store.begin().expect("begin");
        store
            .write_store_uid(&StoreUid::from_entropy_bytes([7; 16]))
            .expect("store uid");
        for id in 1..=3 {
            let identity = [SavedKey::Int(id)];
            store
                .write_record_presence(&root, &identity)
                .expect("presence");
            store
                .write_data_value(
                    &root,
                    &identity,
                    &[DataPathSegment::Member(member.clone())],
                    format!("body-{id}").into_bytes(),
                )
                .expect("value");
        }
        store
            .replace_catalog_snapshot(&sample_catalog())
            .expect("publish catalog");
        store
            .write_commit_metadata(&empty_commit_metadata(1))
            .expect("commit metadata");
        store.commit().expect("commit");
        store
            .verify_structural_digests()
            .expect("a healthy catalog/meta family verifies clean");
        store
            .verify_readable()
            .expect("a healthy catalog/meta family is readable");

        // Capture the healthy committed bytes so each tamper can be reverted, isolating the one
        // cell under test and proving the witness condemns only the damaged one.
        let mut healthy = std::collections::BTreeMap::new();
        backend.tamper(|entries| healthy = entries.clone());
        let fail_closed = |key: Vec<u8>, value: Vec<u8>| {
            backend.tamper(|entries| {
                entries.insert(key, value);
            });
            // The O(1) open witness the admission ladder runs and the deep re-walk the inspection
            // family runs must both refuse the damage.
            assert_corruption(store.validate_commit_record());
            assert_corruption(store.verify_structural_digests());
            assert_corruption(store.verify_readable());
            backend.tamper(|entries| *entries = healthy.clone());
        };

        // The store-uid cell: several byte patterns that no longer decode as `store_<32 hex>` —
        // an empty value, a stray byte, a dropped prefix, a truncated hex run, and a non-hex
        // character — each fail both witnesses closed.
        let uid_key = CellKey::meta(MetaCell::StoreUid).into_bytes();
        for value in [
            Vec::new(),
            vec![0xff],
            b"nostoreprefix00000000000000000000000000".to_vec(),
            b"store_00".to_vec(),
            b"store_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_vec(),
        ] {
            fail_closed(uid_key.clone(), value);
        }

        // The accepted commit stamp: an empty value, a stray byte, and a truncated header each
        // fail to decode and are refused.
        let commit_key = CellKey::meta(MetaCell::Commit).into_bytes();
        for value in [Vec::new(), vec![0xff], vec![0x00, 0x00, 0x00]] {
            fail_closed(commit_key.clone(), value);
        }

        // The commit stamp and the catalog snapshot both name the accepted epoch, so a
        // decodable commit whose `catalog_epoch` no longer matches the snapshot's epoch is an
        // internally-inconsistent store — the binding backup and the runtime fence reject. The
        // snapshot here holds epoch 1; several disagreeing stamped epochs each fail closed while
        // remaining a valid commit encoding.
        for epoch in [0, 2, 4278190081, u64::MAX] {
            let mismatched = encode_commit_metadata(&empty_commit_metadata(epoch))
                .expect("commit metadata encodes");
            fail_closed(commit_key.clone(), mismatched);
        }

        // The sealed commit record: an undecodable value fails to decode, and flipping any byte of
        // the healthy record breaks its content seal, so every byte of every bound field fails the
        // store closed.
        let record_key = CellKey::meta(MetaCell::CommitRecord).into_bytes();
        let healthy_record = healthy
            .get(&record_key)
            .expect("the committed store holds a sealed commit record")
            .clone();
        for value in [Vec::new(), vec![0xff], healthy_record[..8].to_vec()] {
            fail_closed(record_key.clone(), value);
        }
        for offset in 0..healthy_record.len() {
            let mut flipped = healthy_record.clone();
            flipped[offset] ^= 0xff;
            fail_closed(record_key.clone(), flipped);
        }

        // A validly re-sealed record whose sealed epoch, catalog digest, or active-root set
        // disagrees with the data-identity cell it binds is a self-consistent swap the content seal
        // alone cannot see, so the record-vs-cell bind fails it closed. Each mismatched field
        // differs from the healthy store's (epoch 1, snapshot digest, root cat_...09).
        let decoded =
            crate::metadata::decode_commit_record(&healthy_record).expect("record decodes");
        let mut wrong_epoch = decoded.clone();
        wrong_epoch.catalog_epoch = Some(99);
        let mut wrong_digest = decoded.clone();
        wrong_digest.catalog_digest = Some("sha256:0".into());
        let mut wrong_roots = decoded.clone();
        wrong_roots.active_roots = Vec::new();
        for mismatched in [wrong_epoch, wrong_digest, wrong_roots] {
            let bytes = crate::metadata::encode_commit_record(&mismatched).expect("record encodes");
            fail_closed(record_key.clone(), bytes);
        }

        // The catalog snapshot: flip every byte of every catalog-family row in turn. Each flip
        // either breaks a row's decode or shifts the digest the read recomputes from the entries
        // away from the stored header, so all of them fail closed — many offsets across the family.
        let catalog_prefix = CellKey::catalog_family().into_bytes();
        let catalog_rows: Vec<(Vec<u8>, Vec<u8>)> = healthy
            .iter()
            .filter(|(key, _)| key.starts_with(&catalog_prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        assert!(
            !catalog_rows.is_empty(),
            "the committed store must hold catalog rows to tamper",
        );
        for (key, value) in &catalog_rows {
            for offset in 0..value.len() {
                let mut flipped = value.clone();
                flipped[offset] ^= 0xff;
                fail_closed(key.clone(), flipped);
            }
        }
    }
}
