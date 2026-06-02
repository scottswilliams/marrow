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
    AccessGuard, Database, ReadOnlyDatabase, ReadTransaction, ReadableDatabase, ReadableTable,
    TableDefinition, WriteTransaction,
};

use crate::backend::{Backend, Presence, ScanPage, StoreError};
use crate::path::{
    ChildSegment, int_index_key_band, int_record_key_band, is_key_child_segment, root_name,
    segment_len, subtree_band,
};
use crate::traversal::{self, Entries};

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

/// One row's borrowed access guards, holding the encoded key and value alive so a
/// borrow into them stays valid while the shared traversal handles the row.
type Row<'t> = (
    AccessGuard<'t, &'static [u8]>,
    AccessGuard<'t, &'static [u8]>,
);

/// Collect the rows of the subtree at `prefix`, in Marrow order, as access guards.
/// The guards (not the bytes) are materialized so a later borrow into them
/// outlives a single iteration step; redb hands back a fresh guard per row, which
/// a plain `Iterator` cannot lend across steps. Mapping these into [`entries`]
/// gives the shared [`traversal`] functions their source.
///
/// Only the subtree is collected (the range stops at the first key past `prefix`),
/// and `stop_at` caps it earlier still — given the rows gathered so far, it returns
/// whether the traversal can no longer change its answer — so an early-exiting walk
/// (presence, a bounded scan, an edge seek) does not materialize the whole subtree
/// just to look at the rows it needs.
fn collect_rows<'t, T>(
    table: &'t T,
    prefix: &[u8],
    stop_at: impl Fn(&[Row<'t>]) -> bool,
    op: &'static str,
) -> Result<Vec<Row<'t>>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut rows = Vec::new();
    for entry in table.range::<&[u8]>(prefix..).map_err(io(op))? {
        let (key, value) = entry.map_err(io(op))?;
        if !key.value().starts_with(prefix) {
            break; // past the subtree
        }
        if stop_at(&rows) {
            break; // the traversal has already seen all it needs
        }
        rows.push((key, value));
    }
    Ok(rows)
}

fn collect_rows_after<'t, T>(
    table: &'t T,
    prefix: &[u8],
    cursor: &[u8],
    stop_at: impl Fn(&[Row<'t>]) -> bool,
    op: &'static str,
) -> Result<Vec<Row<'t>>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut rows = Vec::new();
    let range = (Bound::Excluded(cursor), Bound::Unbounded);
    for entry in table.range::<&[u8]>(range).map_err(io(op))? {
        let (key, value) = entry.map_err(io(op))?;
        if !key.value().starts_with(prefix) {
            break;
        }
        if stop_at(&rows) {
            break;
        }
        rows.push((key, value));
    }
    Ok(rows)
}

/// Collect the rows of the subtree at `prefix` in **reverse** Marrow order, as
/// access guards. redb ranges are double-ended, so this ranges the subtree band
/// `[prefix, successor)` and walks it backward with `.rev()`; the band's upper
/// bound is what keeps a reverse walk inside the subtree (an unbounded reverse
/// range starts at the global maximum). Mirrors [`collect_rows`] but descending,
/// for [`child_keys_rev`](RedbStore::child_keys_rev) and the `prev`/`last` seeks.
fn collect_rows_rev<'t, T>(
    table: &'t T,
    prefix: &[u8],
    stop_at: impl Fn(&[Row<'t>]) -> bool,
    op: &'static str,
) -> Result<Vec<Row<'t>>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let (lo, hi) = subtree_band(prefix);
    let range = match &hi {
        Some(hi) => table.range::<&[u8]>(lo.as_slice()..hi.as_slice()),
        None => table.range::<&[u8]>(lo.as_slice()..),
    }
    .map_err(io(op))?;
    let mut rows = Vec::new();
    for entry in range.rev() {
        let (key, value) = entry.map_err(io(op))?;
        // The band already bounds the walk to the subtree, so no prefix check is
        // needed; it cannot yield a key outside `[prefix, successor)`.
        if stop_at(&rows) {
            break;
        }
        rows.push((key, value));
    }
    Ok(rows)
}

/// Borrow each collected row as the shared `(key, value)` item shape. The borrows
/// live as long as `rows`, so the traversal can read them across its whole walk.
fn entries<'a>(rows: &'a [Row<'_>]) -> impl Entries<'a> {
    rows.iter()
        .map(|(key, value)| Ok((key.value(), value.value())))
}

/// Whether the already-collected `rows` include an immediate *key* child of
/// `parent` — the edge `first_child`/`last_child` seek target. Named members (a
/// declared index, field, or child layer) sort to one end of the child range, so a
/// `last_child` reverse walk meets them first and a forward `first_child` may skip
/// past them; either way the seek must keep collecting until a navigable key child
/// is in hand. Checking the last collected row suffices: once one key-child row is
/// present the collection can stop. A row above `parent` (the parent's own entry)
/// or with a malformed segment is not a key child, so the walk continues.
fn edge_key_child_seen(parent: &[u8], rows: &[Row<'_>]) -> bool {
    let Some((key, _)) = rows.last() else {
        return false;
    };
    let key = key.value();
    let Some(rest) = key.get(parent.len()..) else {
        return false;
    };
    segment_len(rest).is_some_and(|len| is_key_child_segment(&rest[..len]))
}

/// Whether `key`'s first post-`parent` segment differs from `bound`. A sibling
/// seek collects rows only up to and including the first such key: everything
/// before it is `bound`'s own entry or a descendant — the consecutive run the
/// shared seek skips — so this lets redb stop one row past the run rather than
/// materialize a whole large subtree. A key not under `parent`, or one with a
/// malformed segment, counts as differing so the seek ends (the shared walk
/// reports any corruption).
fn first_segment_differs(parent: &[u8], bound: &[u8], key: &[u8]) -> bool {
    let Some(rest) = key.get(parent.len()..) else {
        return true;
    };
    match segment_len(rest) {
        Some(len) => &rest[..len] != bound,
        None => true,
    }
}

/// Collect the rows of `parent`'s subtree adjacent to the child segment `bound`,
/// in `dir`'s direction, stopping at and including the first row whose segment
/// differs from `bound`. The forward direction begins at `parent ++ bound`
/// (inclusive) for [`next_sibling`](RedbStore::next_sibling); the reversed one
/// walks down to it for [`prev_sibling`](RedbStore::prev_sibling). Either way the
/// collected rows are exactly `bound`'s own run plus the one neighbor past it, so
/// the shared [`traversal::neighbor_child`] reads the neighbor without redb
/// materializing the rest of the subtree.
fn collect_seek<'t, T>(
    table: &'t T,
    parent: &[u8],
    bound: &[u8],
    dir: SeekDir,
    op: &'static str,
) -> Result<Vec<Row<'t>>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut start = parent.to_vec();
    start.extend_from_slice(bound);
    let mut rows = Vec::new();
    match dir {
        // Forward from `parent ++ bound` (inclusive) to the end of the subtree.
        SeekDir::Forward => {
            for entry in table.range::<&[u8]>(start.as_slice()..).map_err(io(op))? {
                let (key, value) = entry.map_err(io(op))?;
                if !key.value().starts_with(parent) {
                    break; // past the subtree (no neighbor that way)
                }
                let differs = first_segment_differs(parent, bound, key.value());
                rows.push((key, value));
                if differs {
                    break; // the neighbor row; the shared seek reads it
                }
            }
        }
        // Reversed, down to `parent ++ bound` (inclusive): bound the band so the
        // reverse walk starts at `bound`'s deepest descendant, not the global max.
        SeekDir::Reverse => {
            for entry in table
                .range::<&[u8]>(parent..=start.as_slice())
                .map_err(io(op))?
                .rev()
            {
                let (key, value) = entry.map_err(io(op))?;
                if key.value().len() <= parent.len() {
                    break; // reached the parent's own entry; no prior child
                }
                let differs = first_segment_differs(parent, bound, key.value());
                rows.push((key, value));
                if differs {
                    break;
                }
            }
        }
    }
    Ok(rows)
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
        read_view!(self, "presence", |table| {
            let has_value = table.get(path).map_err(io("presence"))?.is_some();
            // The subtree's rows sort with `path`'s own entry (if any) first, so the
            // first descendant — the only one presence needs — is among the first
            // two rows. Stopping there avoids walking a large subtree to learn it
            // merely has children.
            let rows = collect_rows(&table, path, |rows| rows.len() >= 2, "presence")?;
            traversal::presence(has_value, entries(&rows), path)
        })
    }

    fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        read_view!(self, "child_keys", |table| {
            let rows = collect_rows(&table, path, |_| false, "child_keys")?;
            traversal::child_keys(entries(&rows), path)
        })
    }

    fn child_keys_rev(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        read_view!(self, "child_keys_rev", |table| {
            let rows = collect_rows_rev(&table, path, |_| false, "child_keys_rev")?;
            traversal::child_keys(entries(&rows), path)
        })
    }

    fn child_count(&self, path: &[u8]) -> Result<usize, StoreError> {
        read_view!(self, "child_count", |table| {
            let rows = collect_rows(&table, path, |_| false, "child_count")?;
            traversal::child_count(entries(&rows), path)
        })
    }

    fn next_sibling(
        &self,
        parent: &[u8],
        after: &[u8],
    ) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "next_sibling", |table| {
            let rows = collect_seek(&table, parent, after, SeekDir::Forward, "next_sibling")?;
            traversal::neighbor_child(entries(&rows), parent, after)
        })
    }

    fn prev_sibling(
        &self,
        parent: &[u8],
        before: &[u8],
    ) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "prev_sibling", |table| {
            let rows = collect_seek(&table, parent, before, SeekDir::Reverse, "prev_sibling")?;
            traversal::neighbor_child(entries(&rows), parent, before)
        })
    }

    fn first_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "first_child", |table| {
            // The first key child sorts before any named member, so it is among the
            // leading rows; collect until one is in hand (or the subtree is spent).
            let rows = collect_rows(
                &table,
                parent,
                |rows| edge_key_child_seen(parent, rows),
                "first_child",
            )?;
            traversal::neighbor_child(entries(&rows), parent, b"")
        })
    }

    fn last_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "last_child", |table| {
            // Reversed, the trailing rows are the named members (a declared index,
            // field, or layer) that sort after the key children; `next`/`prev`
            // navigate keys only, so collect until the first key child surfaces past
            // them (or the subtree is spent), then name it.
            let rows = collect_rows_rev(
                &table,
                parent,
                |rows| edge_key_child_seen(parent, rows),
                "last_child",
            )?;
            traversal::neighbor_child(entries(&rows), parent, b"")
        })
    }

    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        read_view!(self, "scan", |table| {
            // One row past the limit is enough for the scan to report truncation.
            let cap = limit.saturating_add(1);
            let rows = collect_rows(&table, path, move |rows| rows.len() >= cap, "scan")?;
            traversal::scan(entries(&rows), path, limit)
        })
    }

    fn scan_after(&self, path: &[u8], cursor: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        read_view!(self, "scan_after", |table| {
            let cap = limit.saturating_add(1);
            let rows = collect_rows_after(
                &table,
                path,
                cursor,
                move |rows| rows.len() >= cap,
                "scan_after",
            )?;
            traversal::scan(entries(&rows), path, limit)
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
                Backend::read(&store, key.as_slice()).expect("read restored key"),
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
