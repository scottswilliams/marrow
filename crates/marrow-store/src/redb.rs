//! The native persistent backend, over [redb](https://docs.rs/redb).
//!
//! One redb table (`marrow`) maps encoded saved paths to encoded values. redb's
//! `&[u8]` keys order byte-lexicographically — the same order as
//! [`encode_path`](crate::path::encode_path) and the in-memory `BTreeMap` — so
//! traversal yields identical results with no custom comparator. The post-range
//! logic (prefix bounds, child dedup, presence, roots) mirrors
//! [`MemStore`](crate::mem::MemStore); the shared [`conformance`](crate::conformance)
//! suite holds both stores to one contract.
//!
//! Transactions hold one redb write transaction for their whole life: every read
//! and write inside the transaction goes through it, so reads see their own
//! writes. Nesting is an undo journal, not redb savepoints (which cannot be
//! created once a transaction has written): each level records the pre-image of
//! every change, so an inner `rollback` replays its journal in reverse, an inner
//! `commit` merges its journal outward, the outermost `commit` persists the redb
//! transaction, and the outermost `rollback` aborts it. Outside a transaction
//! each write is its own short, immediately durable redb transaction.

use std::ops::Bound;
use std::path::Path;

use redb::{
    Database, ReadOnlyDatabase, ReadTransaction, ReadableDatabase, ReadableTable, TableDefinition,
    WriteTransaction,
};

use crate::backend::{Backend, Presence, ScanPage, StoreError};
use crate::path::{ChildSegment, int_index_key_band, int_record_key_band, root_name, subtree_band};
use crate::traversal;

/// The single table holding every encoded (path, value) pair.
const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("marrow");

/// A small table holding store metadata, currently just the format version.
const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");

/// The on-disk format version this build writes and accepts. A file recording a
/// different version is refused rather than misread (no auto-migration).
const FORMAT_VERSION: u32 = 1;

const DELETE_BATCH_LIMIT: usize = 256;

/// One undone change: a path and the value it held before the change (`None` if
/// it was absent). Replaying it restores that prior state.
type Undo = (Vec<u8>, Option<Vec<u8>>);

/// A redb-backed saved-tree store implementing the [`Backend`] contract.
pub struct RedbStore {
    db: DatabaseHandle,
    /// The live write transaction while one is open (`Some` iff `journals` is
    /// non-empty).
    txn: Option<WriteTransaction>,
    /// One undo log per open nesting level (innermost last).
    journals: Vec<Vec<Undo>>,
}

/// The redb handle is either writable or truly read-only at the storage layer.
enum DatabaseHandle {
    ReadWrite(Database),
    ReadOnly(ReadOnlyDatabase),
}

impl DatabaseHandle {
    fn begin_read(&self, op: &'static str) -> Result<ReadTransaction, StoreError> {
        match self {
            Self::ReadWrite(db) => db.begin_read().map_err(io(op)),
            Self::ReadOnly(db) => db.begin_read().map_err(io(op)),
        }
    }

    fn begin_write(&self, op: &'static str) -> Result<WriteTransaction, StoreError> {
        match self {
            Self::ReadWrite(db) => db.begin_write().map_err(io(op)),
            Self::ReadOnly(_) => Err(StoreError::ReadOnly { op }),
        }
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        match self {
            Self::ReadWrite(_) => Ok(()),
            Self::ReadOnly(_) => Err(StoreError::ReadOnly { op }),
        }
    }
}

/// Map any redb error to a [`StoreError::Io`] naming the operation.
fn io<E: std::fmt::Display>(op: &'static str) -> impl Fn(E) -> StoreError {
    move |error| StoreError::Io {
        op,
        message: error.to_string(),
    }
}

fn streamed_neighbor_child<T>(
    table: &T,
    parent: &[u8],
    bound: &[u8],
    dir: SeekDir,
    op: &'static str,
) -> Result<Option<ChildSegment>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut start = parent.to_vec();
    start.extend_from_slice(bound);
    let seek = traversal::NeighborSeek::new(parent, bound);
    match dir {
        SeekDir::Forward => {
            for entry in table.range::<&[u8]>(start.as_slice()..).map_err(io(op))? {
                let (key, _) = entry.map_err(io(op))?;
                match seek.step(key.value())? {
                    traversal::NeighborStep::Done => break,
                    traversal::NeighborStep::Skip => {}
                    traversal::NeighborStep::Child(child) => return Ok(Some(child)),
                }
            }
        }
        SeekDir::Reverse => {
            for entry in table
                .range::<&[u8]>(parent..=start.as_slice())
                .map_err(io(op))?
                .rev()
            {
                let (key, _) = entry.map_err(io(op))?;
                match seek.step(key.value())? {
                    traversal::NeighborStep::Done => break,
                    traversal::NeighborStep::Skip => {}
                    traversal::NeighborStep::Child(child) => return Ok(Some(child)),
                }
            }
        }
    }
    Ok(None)
}

fn streamed_edge_child<T>(
    table: &T,
    parent: &[u8],
    dir: SeekDir,
    op: &'static str,
) -> Result<Option<ChildSegment>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let seek = traversal::NeighborSeek::new(parent, b"");
    match dir {
        SeekDir::Forward => {
            for entry in table.range::<&[u8]>(parent..).map_err(io(op))? {
                let (key, _) = entry.map_err(io(op))?;
                match seek.step(key.value())? {
                    traversal::NeighborStep::Done => break,
                    traversal::NeighborStep::Skip => {}
                    traversal::NeighborStep::Child(child) => return Ok(Some(child)),
                }
            }
        }
        SeekDir::Reverse => {
            let (lo, hi) = subtree_band(parent);
            let range = match &hi {
                Some(hi) => table.range::<&[u8]>(lo.as_slice()..hi.as_slice()),
                None => table.range::<&[u8]>(lo.as_slice()..),
            }
            .map_err(io(op))?;
            for entry in range.rev() {
                let (key, _) = entry.map_err(io(op))?;
                match seek.step(key.value())? {
                    traversal::NeighborStep::Done => break,
                    traversal::NeighborStep::Skip => {}
                    traversal::NeighborStep::Child(child) => return Ok(Some(child)),
                }
            }
        }
    }
    Ok(None)
}

/// Which way a sibling seek walks: forward for the next sibling, reversed for the
/// previous. A private two-state direction.
#[derive(Clone, Copy)]
enum SeekDir {
    Forward,
    Reverse,
}

fn delete_key_batch<T>(table: &T, path: &[u8]) -> Result<Vec<Vec<u8>>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut keys = Vec::new();
    for entry in table.range::<&[u8]>(path..).map_err(io("delete"))? {
        let (key, _) = entry.map_err(io("delete"))?;
        let key = key.value();
        if !key.starts_with(path) {
            break;
        }
        keys.push(key.to_vec());
        if keys.len() == DELETE_BATCH_LIMIT {
            break;
        }
    }
    Ok(keys)
}

fn streamed_roots<T>(table: &T) -> Result<Vec<String>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut roots = Vec::new();
    for entry in table.range::<&[u8]>(..).map_err(io("roots"))? {
        let (key, _) = entry.map_err(io("roots"))?;
        let key = key.value();
        let name = root_name(key).ok_or_else(|| StoreError::CorruptPath { path: key.to_vec() })?;
        if roots.last() != Some(&name) {
            roots.push(name);
        }
    }
    Ok(roots)
}

fn streamed_child_keys<T>(table: &T, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut children = Vec::new();
    let mut collapse = traversal::ChildCollapse::new(path);
    for entry in table.range::<&[u8]>(path..).map_err(io("child_keys"))? {
        let (key, _) = entry.map_err(io("child_keys"))?;
        match collapse.step(key.value())? {
            traversal::ChildStep::Done => break,
            traversal::ChildStep::Skip => {}
            traversal::ChildStep::Child(child) => children.push(child),
        }
    }
    Ok(children)
}

fn streamed_child_keys_rev<T>(table: &T, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let (lo, hi) = subtree_band(path);
    let range = match &hi {
        Some(hi) => table.range::<&[u8]>(lo.as_slice()..hi.as_slice()),
        None => table.range::<&[u8]>(lo.as_slice()..),
    }
    .map_err(io("child_keys_rev"))?;
    let mut children = Vec::new();
    let mut collapse = traversal::ChildCollapse::new(path);
    for entry in range.rev() {
        let (key, _) = entry.map_err(io("child_keys_rev"))?;
        match collapse.step(key.value())? {
            traversal::ChildStep::Done => break,
            traversal::ChildStep::Skip => {}
            traversal::ChildStep::Child(child) => children.push(child),
        }
    }
    Ok(children)
}

fn streamed_child_count<T>(table: &T, path: &[u8]) -> Result<usize, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut count = 0;
    let mut collapse = traversal::ChildCollapse::new(path);
    for entry in table.range::<&[u8]>(path..).map_err(io("child_count"))? {
        let (key, _) = entry.map_err(io("child_count"))?;
        match collapse.step(key.value())? {
            traversal::ChildStep::Done => break,
            traversal::ChildStep::Skip => {}
            traversal::ChildStep::Child(_) => count += 1,
        }
    }
    Ok(count)
}

fn streamed_presence<T>(table: &T, path: &[u8]) -> Result<Presence, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let has_value = table.get(path).map_err(io("presence"))?.is_some();
    let probe = traversal::DescendantProbe::new(path);
    let mut has_descendants = false;
    for entry in table.range::<&[u8]>(path..).map_err(io("presence"))? {
        let (key, _) = entry.map_err(io("presence"))?;
        match probe.step(key.value()) {
            traversal::DescendantStep::Done => break,
            traversal::DescendantStep::Skip => {}
            traversal::DescendantStep::Found => {
                has_descendants = true;
                break;
            }
        }
    }
    Ok(traversal::presence_from_parts(has_value, has_descendants))
}

fn streamed_scan<T>(table: &T, path: &[u8], limit: usize) -> Result<ScanPage, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut scan = traversal::ScanAccumulator::new(path, limit);
    for entry in table.range::<&[u8]>(path..).map_err(io("scan"))? {
        let (key, value) = entry.map_err(io("scan"))?;
        match scan.step(key.value(), value.value()) {
            traversal::ScanStep::Done => break,
            traversal::ScanStep::Continue => {}
        }
    }
    Ok(scan.into_page())
}

fn streamed_scan_after<T>(
    table: &T,
    path: &[u8],
    cursor: &[u8],
    limit: usize,
) -> Result<ScanPage, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut scan = traversal::ScanAccumulator::new(path, limit);
    let range = table
        .range::<&[u8]>((Bound::Excluded(cursor), Bound::Unbounded))
        .map_err(io("scan_after"))?;
    for entry in range {
        let (key, value) = entry.map_err(io("scan_after"))?;
        match scan.step(key.value(), value.value()) {
            traversal::ScanStep::Done => break,
            traversal::ScanStep::Continue => {}
        }
    }
    Ok(scan.into_page())
}

/// Run a read `$body` over the current view's table: the open transaction's
/// table (so a transaction reads its own writes), or a fresh read transaction
/// otherwise. A macro, not a `&dyn` helper, because redb's `ReadableTable` is not
/// object-safe — the body is monomorphized for each table type instead.
macro_rules! read_view {
    ($self:expr, $op:expr, |$table:ident| $body:expr) => {
        match &$self.txn {
            Some(write) => {
                let $table = write.open_table(TABLE).map_err(io($op))?;
                $body
            }
            None => {
                let read = $self.db.begin_read($op)?;
                let $table = read.open_table(TABLE).map_err(io($op))?;
                $body
            }
        }
    };
}

impl RedbStore {
    /// Open the redb-backed store at `path`, creating the file if needed. A
    /// second writer for the same file is rejected as [`StoreError::Locked`]
    /// (redb holds an OS lock), and a file recording a different
    /// [`FORMAT_VERSION`] is rejected as [`StoreError::FormatVersion`].
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let db = Database::create(path).map_err(|error| match error {
            redb::DatabaseError::DatabaseAlreadyOpen => StoreError::Locked {
                data_dir: path.to_path_buf(),
            },
            other => StoreError::Io {
                op: "open",
                message: other.to_string(),
            },
        })?;
        let write = db.begin_write().map_err(io("open"))?;
        // `Database::create` also opens an existing file, so a brand-new database
        // must be told apart from one that already has tables. A fresh database has
        // none; stamp the version only then. A non-empty file with no meta is a
        // foreign or meta-less store, rejected as corruption (matching
        // `open_read_only`) rather than silently adopted and written into.
        let is_new = write.list_tables().map_err(io("open"))?.next().is_none();
        {
            // Check or stamp the format version before touching data. Read the
            // value into an owned `Option<u32>` first so the access guard drops
            // before the `insert` below.
            let mut meta = write.open_table(META).map_err(io("open"))?;
            let recorded = meta
                .get("format_version")
                .map_err(io("open"))?
                .map(|guard| guard.value());
            match recorded {
                Some(found) if found != FORMAT_VERSION => {
                    return Err(StoreError::FormatVersion {
                        found,
                        supported: FORMAT_VERSION,
                    });
                }
                Some(_) => {}
                None if is_new => {
                    meta.insert("format_version", FORMAT_VERSION)
                        .map_err(io("open"))?;
                }
                None => {
                    return Err(StoreError::Corruption {
                        message: "store is missing its format version".into(),
                    });
                }
            }
        }
        // Create the data table now so later reads never meet a missing table.
        write.open_table(TABLE).map_err(io("open"))?;
        write.commit().map_err(io("open"))?;
        Ok(Self {
            db: DatabaseHandle::ReadWrite(db),
            txn: None,
            journals: Vec::new(),
        })
    }

    /// Open an existing store for read-only inspection. Unlike [`open`](Self::open)
    /// it never creates the file — a missing path is an error — and it only
    /// verifies the recorded [`FORMAT_VERSION`] rather than stamping it. The
    /// returned store uses redb's read-only handle, and Marrow's write-capability
    /// operations fail before starting any write transaction.
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let db = ReadOnlyDatabase::open(path).map_err(|error| match error {
            redb::DatabaseError::DatabaseAlreadyOpen => StoreError::Locked {
                data_dir: path.to_path_buf(),
            },
            other => StoreError::Io {
                op: "open",
                message: other.to_string(),
            },
        })?;
        {
            // Verify (never stamp) the format version through a read transaction. A
            // file with no meta table is not a Marrow store, not a fresh one.
            let read = db.begin_read().map_err(io("open"))?;
            let meta = match read.open_table(META) {
                Ok(meta) => meta,
                Err(redb::TableError::TableDoesNotExist(_)) => {
                    return Err(StoreError::Corruption {
                        message: "store is missing its format version".into(),
                    });
                }
                Err(other) => return Err(io("open")(other)),
            };
            let recorded = meta
                .get("format_version")
                .map_err(io("open"))?
                .map(|guard| guard.value());
            match recorded {
                Some(found) if found != FORMAT_VERSION => {
                    return Err(StoreError::FormatVersion {
                        found,
                        supported: FORMAT_VERSION,
                    });
                }
                Some(_) => {}
                None => {
                    return Err(StoreError::Corruption {
                        message: "store is missing its format version".into(),
                    });
                }
            }
        }
        Ok(Self {
            db: DatabaseHandle::ReadOnly(db),
            txn: None,
            journals: Vec::new(),
        })
    }

    /// Record `entry` in the innermost open journal, so a later `rollback` can
    /// undo the change it describes.
    fn record(&mut self, entry: Undo) {
        self.journals
            .last_mut()
            .expect("a journal while a transaction is open")
            .push(entry);
    }

    /// The highest integer key in the half-open byte `band` of integer-keyed
    /// children of `prefix`. The band is one contiguous numeric-ordered run, so
    /// its last entry (redb ranges are double-ended) is the highest; the shared
    /// decode reads the key just past the kind tag. `None` when the band is empty.
    fn max_int_in_band(
        &self,
        prefix: &[u8],
        (lo, hi): (Vec<u8>, Vec<u8>),
    ) -> Result<Option<i64>, StoreError> {
        read_view!(self, "max_int_key", |table| {
            let last = table
                .range::<&[u8]>(lo.as_slice()..hi.as_slice())
                .map_err(io("max_int_key"))?
                .next_back();
            // Keep the last row's guard alive so the borrow into it survives the
            // shared decode below.
            let last = last.transpose().map_err(io("max_int_key"))?;
            traversal::max_int_key(last.as_ref().map(|(key, _)| Ok(key.value())), prefix)
        })
    }
}

impl Backend for RedbStore {
    fn read(&self, path: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        read_view!(self, "read", |table| Ok(table
            .get(path)
            .map_err(io("read"))?
            .map(|guard| guard.value().to_vec())))
    }

    fn write(&mut self, path: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.db.require_write_access("write")?;
        if self.txn.is_none() {
            let write = self.db.begin_write("write")?;
            {
                let mut table = write.open_table(TABLE).map_err(io("write"))?;
                table.insert(path, value.as_slice()).map_err(io("write"))?;
            }
            return write.commit().map_err(io("write"));
        }
        // In a transaction: write through it and journal the prior value (the
        // value `insert` returns) so a rollback can restore it.
        let old = {
            let write = self.txn.as_ref().expect("an open transaction");
            let mut table = write.open_table(TABLE).map_err(io("write"))?;
            table
                .insert(path, value.as_slice())
                .map_err(io("write"))?
                .map(|guard| guard.value().to_vec())
        };
        self.record((path.to_vec(), old));
        Ok(())
    }

    fn delete(&mut self, path: &[u8]) -> Result<(), StoreError> {
        self.db.require_write_access("delete")?;
        if self.txn.is_none() {
            let write = self.db.begin_write("delete")?;
            {
                let mut table = write.open_table(TABLE).map_err(io("delete"))?;
                loop {
                    let keys = delete_key_batch(&table, path)?;
                    if keys.is_empty() {
                        break;
                    }
                    for key in keys {
                        table.remove(key.as_slice()).map_err(io("delete"))?;
                    }
                }
            }
            return write.commit().map_err(io("delete"));
        }
        // In a transaction: journal each removed preimage before advancing to the
        // next bounded key batch.
        loop {
            let undo = {
                let write = self.txn.as_ref().expect("an open transaction");
                let mut table = write.open_table(TABLE).map_err(io("delete"))?;
                let keys = delete_key_batch(&table, path)?;
                if keys.is_empty() {
                    break;
                }
                let mut undo = Vec::with_capacity(keys.len());
                for key in keys {
                    let old = table
                        .remove(key.as_slice())
                        .map_err(io("delete"))?
                        .map(|guard| guard.value().to_vec());
                    undo.push((key, old));
                }
                undo
            };
            for entry in undo {
                self.record(entry);
            }
        }
        Ok(())
    }

    fn presence(&self, path: &[u8]) -> Result<Presence, StoreError> {
        read_view!(self, "presence", |table| streamed_presence(&table, path))
    }

    fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        read_view!(self, "child_keys", |table| {
            streamed_child_keys(&table, path)
        })
    }

    fn child_keys_rev(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        read_view!(self, "child_keys_rev", |table| {
            streamed_child_keys_rev(&table, path)
        })
    }

    fn child_count(&self, path: &[u8]) -> Result<usize, StoreError> {
        read_view!(self, "child_count", |table| {
            streamed_child_count(&table, path)
        })
    }

    fn next_sibling(
        &self,
        parent: &[u8],
        after: &[u8],
    ) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "next_sibling", |table| {
            streamed_neighbor_child(&table, parent, after, SeekDir::Forward, "next_sibling")
        })
    }

    fn prev_sibling(
        &self,
        parent: &[u8],
        before: &[u8],
    ) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "prev_sibling", |table| {
            streamed_neighbor_child(&table, parent, before, SeekDir::Reverse, "prev_sibling")
        })
    }

    fn first_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "first_child", |table| {
            streamed_edge_child(&table, parent, SeekDir::Forward, "first_child")
        })
    }

    fn last_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "last_child", |table| {
            streamed_edge_child(&table, parent, SeekDir::Reverse, "last_child")
        })
    }

    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        read_view!(self, "scan", |table| streamed_scan(&table, path, limit))
    }

    fn scan_after(&self, path: &[u8], cursor: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        read_view!(self, "scan_after", |table| {
            streamed_scan_after(&table, path, cursor, limit)
        })
    }

    fn roots(&self) -> Result<Vec<String>, StoreError> {
        read_view!(self, "roots", |table| streamed_roots(&table))
    }

    fn max_int_record_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError> {
        self.max_int_in_band(prefix, int_record_key_band(prefix))
    }

    fn max_int_index_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError> {
        self.max_int_in_band(prefix, int_index_key_band(prefix))
    }

    fn begin(&mut self) -> Result<(), StoreError> {
        self.db.require_write_access("begin")?;
        if self.txn.is_none() {
            self.txn = Some(self.db.begin_write("begin")?);
        }
        self.journals.push(Vec::new());
        Ok(())
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        // With no open transaction, commit is a no-op (the in-memory store agrees):
        // callers pair begin with commit, so a stray commit is a harmless misuse.
        let Some(journal) = self.journals.pop() else {
            return Ok(());
        };
        match self.journals.last_mut() {
            // An inner commit keeps its writes; its undo log moves outward so an
            // outer rollback still undoes them.
            Some(outer) => outer.extend(journal),
            // The outermost commit persists the whole redb transaction.
            None => {
                let write = self.txn.take().expect("a transaction while committing");
                write.commit().map_err(io("commit"))?;
            }
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), StoreError> {
        // With no open transaction, rollback is a no-op (matching the in-memory
        // store), so an unbalanced rollback is harmless rather than a store.io error.
        let Some(journal) = self.journals.pop() else {
            return Ok(());
        };
        if self.journals.is_empty() {
            // Outermost: abort the redb transaction, discarding every change.
            let write = self.txn.take().expect("a transaction while rolling back");
            write.abort().map_err(io("rollback"))?;
            return Ok(());
        }
        // Inner: undo this level's changes in reverse, against the open
        // transaction, leaving the outer levels in place.
        let write = self.txn.as_ref().expect("a transaction while rolling back");
        let mut table = write.open_table(TABLE).map_err(io("rollback"))?;
        for (path, old) in journal.into_iter().rev() {
            match old {
                Some(value) => {
                    table
                        .insert(path.as_slice(), value.as_slice())
                        .map_err(io("rollback"))?;
                }
                None => {
                    table.remove(path.as_slice()).map_err(io("rollback"))?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;

    /// The native store satisfies the same backend conformance suite as the
    /// in-memory store — one contract, two backends.
    #[test]
    fn redb_store_passes_the_conformance_suite() {
        let dir = tempfile::tempdir().expect("create a temp dir");
        let mut counter = 0;
        conformance::run_all(|| {
            // Each law gets a fresh redb file in the shared temp dir; the dir (and
            // its files) outlives every store, dropping only when the test ends.
            counter += 1;
            let path = dir.path().join(format!("store-{counter}.redb"));
            RedbStore::open(&path).expect("open a fresh redb store")
        });
    }

    #[test]
    fn delete_removes_more_than_one_bounded_batch() {
        let dir = tempfile::tempdir().expect("create a temp dir");
        let path = dir.path().join("bulk-delete.redb");
        let mut store = RedbStore::open(&path).expect("open a fresh redb store");
        let prefix = b"bulk/";
        let outside = b"bulk0/kept".as_slice();

        let mut keys = Vec::new();
        for n in 0..DELETE_BATCH_LIMIT + 7 {
            let key = format!("bulk/{n:04}").into_bytes();
            Backend::write(&mut store, key.as_slice(), b"value".to_vec()).expect("write bulk key");
            keys.push(key);
        }
        Backend::write(&mut store, outside, b"kept".to_vec()).expect("write outside key");

        Backend::delete(&mut store, prefix).expect("delete bulk prefix");

        for key in keys {
            assert_eq!(
                Backend::read(&store, key.as_slice()).expect("read bulk key"),
                None
            );
        }
        assert_eq!(
            Backend::read(&store, outside).expect("read outside key"),
            Some(b"kept".to_vec())
        );
    }

    #[test]
    fn rollback_restores_delete_across_more_than_one_bounded_batch() {
        let dir = tempfile::tempdir().expect("create a temp dir");
        let path = dir.path().join("bulk-delete-rollback.redb");
        let mut store = RedbStore::open(&path).expect("open a fresh redb store");
        let prefix = b"bulk/";
        let outside = b"bulk0/kept".as_slice();

        let mut keys = Vec::new();
        for n in 0..DELETE_BATCH_LIMIT + 7 {
            let key = format!("bulk/{n:04}").into_bytes();
            Backend::write(&mut store, key.as_slice(), b"value".to_vec()).expect("write bulk key");
            keys.push(key);
        }
        Backend::write(&mut store, outside, b"kept".to_vec()).expect("write outside key");

        Backend::begin(&mut store).expect("begin transaction");
        Backend::delete(&mut store, prefix).expect("delete bulk prefix");
        assert_eq!(
            Backend::read(&store, keys[0].as_slice()).expect("read deleted key"),
            None
        );
        Backend::rollback(&mut store).expect("rollback delete");

        for key in keys {
            assert_eq!(
                Backend::read(&store, key.as_slice()).expect("read rollback key"),
                Some(b"value".to_vec())
            );
        }
        assert_eq!(
            Backend::read(&store, outside).expect("read outside key"),
            Some(b"kept".to_vec())
        );
    }

    #[test]
    fn redb_read_transactions_are_stable_snapshots() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("snapshot.redb");
        let key: &[u8] = b"k";
        let old: &[u8] = b"old";
        let new: &[u8] = b"new";

        let mut store = RedbStore::open(&path).expect("open");
        Backend::write(&mut store, key, old.to_vec()).expect("seed old value");

        let db = match store.db {
            DatabaseHandle::ReadWrite(db) => db,
            DatabaseHandle::ReadOnly(_) => panic!("expected a read-write redb handle"),
        };

        let read = db.begin_read().expect("begin read transaction");
        let table = read
            .open_table(TABLE)
            .expect("open table in read transaction");
        assert_eq!(
            table
                .get(key)
                .expect("read original value")
                .map(|value| value.value().to_vec()),
            Some(old.to_vec())
        );

        let write = db.begin_write().expect("begin write transaction");
        {
            let mut table = write.open_table(TABLE).expect("open table for write");
            table.insert(key, new).expect("replace value");
        }
        write.commit().expect("commit replacement");

        assert_eq!(
            table
                .get(key)
                .expect("read through original transaction")
                .map(|value| value.value().to_vec()),
            Some(old.to_vec())
        );

        drop(table);
        drop(read);

        let read = db.begin_read().expect("begin fresh read transaction");
        let table = read.open_table(TABLE).expect("open table in fresh read");
        assert_eq!(
            table
                .get(key)
                .expect("read fresh value")
                .map(|value| value.value().to_vec()),
            Some(new.to_vec())
        );
    }

    #[test]
    fn redb_aborted_write_transaction_does_not_publish_raw_byte_changes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("aborted-write.redb");

        let store = RedbStore::open(&path).expect("open");
        let db = match store.db {
            DatabaseHandle::ReadWrite(db) => db,
            DatabaseHandle::ReadOnly(_) => panic!("expected a read-write redb handle"),
        };

        let seed = db.begin_write().expect("begin seed transaction");
        {
            let mut table = seed.open_table(TABLE).expect("open table for seed");
            table
                .insert(b"kept".as_slice(), b"old".as_slice())
                .expect("seed kept value");
            table
                .insert(b"removed".as_slice(), b"still-here".as_slice())
                .expect("seed removable value");
        }
        seed.commit().expect("commit seed values");

        let write = db.begin_write().expect("begin write transaction");
        {
            let mut table = write.open_table(TABLE).expect("open table for write");
            table
                .insert(b"kept".as_slice(), b"new".as_slice())
                .expect("replace raw byte key");
            table
                .insert(b"added".as_slice(), b"transient".as_slice())
                .expect("insert raw byte key");
            table
                .remove(b"removed".as_slice())
                .expect("remove raw byte key");
        }
        write.abort().expect("abort raw byte changes");

        let read = db.begin_read().expect("begin fresh read transaction");
        let table = read.open_table(TABLE).expect("open table for read");
        assert_eq!(
            table
                .get(b"kept".as_slice())
                .expect("read kept value")
                .map(|value| value.value().to_vec()),
            Some(b"old".to_vec())
        );
        assert_eq!(
            table
                .get(b"removed".as_slice())
                .expect("read removed value")
                .map(|value| value.value().to_vec()),
            Some(b"still-here".to_vec())
        );
        assert_eq!(
            table
                .get(b"added".as_slice())
                .expect("read added value")
                .map(|value| value.value().to_vec()),
            None
        );
    }

    #[test]
    fn redb_table_orders_raw_byte_keys_lexicographically() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("ordered-bytes.redb");

        let store = RedbStore::open(&path).expect("open");
        let db = match store.db {
            DatabaseHandle::ReadWrite(db) => db,
            DatabaseHandle::ReadOnly(_) => panic!("expected a read-write redb handle"),
        };

        let write = db.begin_write().expect("begin write transaction");
        {
            let mut table = write.open_table(TABLE).expect("open table for write");
            let value: &[u8] = b"value";
            for key in [b"b".as_slice(), b"a", &[0x00], &[0x00, 0xff], b"aa"] {
                table.insert(key, value).expect("insert raw byte key");
            }
        }
        write.commit().expect("commit raw byte keys");

        let read = db.begin_read().expect("begin read transaction");
        let table = read.open_table(TABLE).expect("open table for read");
        let all_keys = table
            .range::<&[u8]>(..)
            .expect("range all raw byte keys")
            .map(|entry| {
                let (key, _) = entry.expect("read raw byte key");
                key.value().to_vec()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            all_keys,
            vec![
                vec![0x00],
                vec![0x00, 0xff],
                b"a".to_vec(),
                b"aa".to_vec(),
                b"b".to_vec()
            ]
        );

        let a_to_b_keys = table
            .range::<&[u8]>(b"a".as_slice()..b"b".as_slice())
            .expect("range raw byte keys from a to b")
            .map(|entry| {
                let (key, _) = entry.expect("read raw byte key in half-open range");
                key.value().to_vec()
            })
            .collect::<Vec<_>>();
        assert_eq!(a_to_b_keys, vec![b"a".to_vec(), b"aa".to_vec()]);
    }

    /// A foreign or meta-less redb file — one with tables but no `marrow.meta` —
    /// must be rejected as corruption, not silently adopted and stamped as a
    /// Marrow store. (`Database::create` opens existing files too, so `open` tells
    /// a brand-new database from an existing one by whether it has any tables.)
    #[test]
    fn open_rejects_an_existing_file_missing_meta() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("foreign.redb");

        // Build a non-empty redb file with some other table and no `marrow.meta`.
        {
            let db = Database::create(&path).expect("create foreign db");
            let write = db.begin_write().expect("begin");
            const OTHER: TableDefinition<&str, u32> = TableDefinition::new("not.marrow");
            write.open_table(OTHER).expect("open foreign table");
            write.commit().expect("commit foreign db");
        }

        match RedbStore::open(&path) {
            Err(StoreError::Corruption { .. }) => {}
            Err(other) => panic!("expected corruption for a meta-less file, got {other:?}"),
            Ok(_) => panic!("a meta-less file must not be adopted as a Marrow store"),
        }
    }

    #[test]
    fn open_rejects_unsupported_format_version_with_typed_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("future-format.redb");
        let unsupported = FORMAT_VERSION + 1;

        {
            let db = Database::create(&path).expect("create redb file");
            let write = db.begin_write().expect("begin");
            {
                let mut meta = write.open_table(META).expect("open meta table");
                meta.insert("format_version", unsupported)
                    .expect("write future format version");
            }
            {
                let _table = write.open_table(TABLE).expect("open data table");
            }
            write.commit().expect("commit future-format store");
        }

        for result in [RedbStore::open(&path), RedbStore::open_read_only(&path)] {
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("future format version must be rejected"),
            };
            assert_eq!(error.code(), "store.format_version");
            match error {
                StoreError::FormatVersion { found, supported } => {
                    assert_eq!(found, unsupported);
                    assert_eq!(supported, FORMAT_VERSION);
                }
                other => panic!("expected format version error, got {other:?}"),
            }
        }
    }

    /// A brand-new file is created and stamped, and reopening the stamped store
    /// succeeds — the new-vs-existing distinction does not break the normal path.
    #[test]
    fn open_creates_and_reopens_a_fresh_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("fresh.redb");
        {
            let mut store = RedbStore::open(&path).expect("create fresh");
            store.write(b"k", b"v".to_vec()).expect("write");
        }
        let store = RedbStore::open(&path).expect("reopen stamped store");
        assert_eq!(store.read(b"k").expect("read"), Some(b"v".to_vec()));
    }
}
