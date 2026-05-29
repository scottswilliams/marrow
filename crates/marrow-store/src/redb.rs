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

use std::path::Path;

use redb::{
    AccessGuard, Database, ReadableDatabase, ReadableTable, TableDefinition, WriteTransaction,
};

use crate::backend::{Backend, Presence, ScanPage, StoreError};
use crate::path::{
    ChildSegment, int_index_key_band, int_record_key_band, segment_len, subtree_band,
};
use crate::traversal::{self, Entries};

/// The single table holding every encoded (path, value) pair.
const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("marrow");

/// A small table holding store metadata, currently just the format version.
const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");

/// The on-disk format version this build writes and accepts. A file recording a
/// different version is refused rather than misread (no auto-migration).
const FORMAT_VERSION: u32 = 1;

/// One undone change: a path and the value it held before the change (`None` if
/// it was absent). Replaying it restores that prior state.
type Undo = (Vec<u8>, Option<Vec<u8>>);

/// A redb-backed saved-tree store implementing the [`Backend`] contract.
pub struct RedbStore {
    db: Database,
    /// The live write transaction while one is open (`Some` iff `journals` is
    /// non-empty).
    txn: Option<WriteTransaction>,
    /// One undo log per open nesting level (innermost last).
    journals: Vec<Vec<Undo>>,
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
/// and `stop_at` caps it earlier still — a count past which the traversal cannot
/// change its answer — so an early-exiting walk (presence, a bounded scan) does
/// not materialize the whole subtree just to look at its first rows.
fn collect_rows<'t, T>(
    table: &'t T,
    prefix: &[u8],
    stop_at: impl Fn(usize) -> bool,
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
        if stop_at(rows.len()) {
            break; // the traversal has already seen all it needs
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
    stop_at: impl Fn(usize) -> bool,
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
        if stop_at(rows.len()) {
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
/// previous. A private two-state direction, never the deferred per-component
/// index direction.
#[derive(Clone, Copy)]
enum SeekDir {
    Forward,
    Reverse,
}

/// The encoded keys of the subtree at `path` (the path's own entry and every
/// descendant), in Marrow order.
fn subtree_keys<T>(table: &T, path: &[u8]) -> Result<Vec<Vec<u8>>, StoreError>
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
    }
    Ok(keys)
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
                let read = $self.db.begin_read().map_err(io($op))?;
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
            db,
            txn: None,
            journals: Vec::new(),
        })
    }

    /// Open an existing store for read-only inspection. Unlike [`open`](Self::open)
    /// it never creates the file — a missing path is an error — and it only
    /// verifies the recorded [`FORMAT_VERSION`] rather than stamping it. redb has
    /// no read-only database handle, so the returned store is technically writable;
    /// an inspecting caller must use only the reading [`Backend`] methods.
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let db = Database::open(path).map_err(|error| match error {
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
            db,
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
        if self.txn.is_none() {
            let write = self.db.begin_write().map_err(io("write"))?;
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
        if self.txn.is_none() {
            let write = self.db.begin_write().map_err(io("delete"))?;
            {
                let mut table = write.open_table(TABLE).map_err(io("delete"))?;
                for key in subtree_keys(&table, path)? {
                    table.remove(key.as_slice()).map_err(io("delete"))?;
                }
            }
            return write.commit().map_err(io("delete"));
        }
        // In a transaction: remove each subtree key, journaling its prior value.
        let undo = {
            let write = self.txn.as_ref().expect("an open transaction");
            let mut table = write.open_table(TABLE).map_err(io("delete"))?;
            let mut undo = Vec::new();
            for key in subtree_keys(&table, path)? {
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
        Ok(())
    }

    fn presence(&self, path: &[u8]) -> Result<Presence, StoreError> {
        read_view!(self, "presence", |table| {
            let has_value = table.get(path).map_err(io("presence"))?.is_some();
            // The subtree's rows sort with `path`'s own entry (if any) first, so the
            // first descendant — the only one presence needs — is among the first
            // two rows. Stopping there avoids walking a large subtree to learn it
            // merely has children.
            let rows = collect_rows(&table, path, |seen| seen >= 2, "presence")?;
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
            // The first stored row under `parent` is its first child (or its own
            // entry, which the shared seek skips); two rows always suffice.
            let rows = collect_rows(&table, parent, |seen| seen >= 2, "first_child")?;
            traversal::neighbor_child(entries(&rows), parent, b"")
        })
    }

    fn last_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError> {
        read_view!(self, "last_child", |table| {
            // Reversed, the first row is the last child's deepest descendant; one
            // row is enough for the shared edge seek to name its immediate child.
            let rows = collect_rows_rev(&table, parent, |seen| seen >= 1, "last_child")?;
            traversal::neighbor_child(entries(&rows), parent, b"")
        })
    }

    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        read_view!(self, "scan", |table| {
            // One row past the limit is enough for the scan to report truncation.
            let cap = limit.saturating_add(1);
            let rows = collect_rows(&table, path, move |seen| seen >= cap, "scan")?;
            traversal::scan(entries(&rows), path, limit)
        })
    }

    fn roots(&self) -> Result<Vec<String>, StoreError> {
        read_view!(self, "roots", |table| {
            let rows = collect_rows(&table, &[], |_| false, "roots")?;
            traversal::roots(entries(&rows))
        })
    }

    fn max_int_record_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError> {
        self.max_int_in_band(prefix, int_record_key_band(prefix))
    }

    fn max_int_index_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError> {
        self.max_int_in_band(prefix, int_index_key_band(prefix))
    }

    fn begin(&mut self) -> Result<(), StoreError> {
        if self.txn.is_none() {
            self.txn = Some(self.db.begin_write().map_err(io("begin"))?);
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
