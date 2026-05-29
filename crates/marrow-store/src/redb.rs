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

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition, WriteTransaction};

use crate::backend::{Backend, Presence, ScanPage, StoreError};
use crate::path::{
    ChildSegment, SavedKey, decode_child_segment, decode_key_value, int_index_key_band,
    int_record_key_band, root_name, segment_len,
};

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

/// Build a [`StoreError::CorruptPath`] for a stored key that failed to decode.
fn corrupt(key: &[u8]) -> StoreError {
    StoreError::CorruptPath { path: key.to_vec() }
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
    /// its last entry (redb ranges are double-ended) is the highest; the key
    /// after `prefix` is the kind tag (one byte) then the integer key encoding,
    /// so it decodes from `prefix.len() + 1`. `None` when the band is empty.
    fn max_int_in_band(
        &self,
        prefix: &[u8],
        (lo, hi): (Vec<u8>, Vec<u8>),
    ) -> Result<Option<i64>, StoreError> {
        read_view!(self, "max_int_key", |table| {
            let Some(entry) = table
                .range::<&[u8]>(lo.as_slice()..hi.as_slice())
                .map_err(io("max_int_key"))?
                .next_back()
            else {
                return Ok(None);
            };
            let (key, _) = entry.map_err(io("max_int_key"))?;
            let key = key.value();
            match decode_key_value(key.get(prefix.len() + 1..).unwrap_or(&[])) {
                Some((SavedKey::Int(value), _)) => Ok(Some(value)),
                _ => Err(corrupt(key)),
            }
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
            let mut has_descendants = false;
            for entry in table.range::<&[u8]>(path..).map_err(io("presence"))? {
                let (key, _) = entry.map_err(io("presence"))?;
                let key = key.value();
                if !key.starts_with(path) {
                    break;
                }
                if key.len() > path.len() {
                    has_descendants = true;
                    break;
                }
            }
            Ok(match (has_value, has_descendants) {
                (false, false) => Presence::Absent,
                (true, false) => Presence::ValueOnly,
                (false, true) => Presence::ChildrenOnly,
                (true, true) => Presence::ValueAndChildren,
            })
        })
    }

    fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        read_view!(self, "child_keys", |table| {
            let mut children = Vec::new();
            let mut last: Option<Vec<u8>> = None;
            for entry in table.range::<&[u8]>(path..).map_err(io("child_keys"))? {
                let (key, _) = entry.map_err(io("child_keys"))?;
                let key = key.value();
                if !key.starts_with(path) {
                    break;
                }
                if key.len() <= path.len() {
                    continue; // the path's own entry, not a child
                }
                let rest = &key[path.len()..];
                let len = segment_len(rest).ok_or_else(|| corrupt(key))?;
                let segment = &rest[..len];
                if last.as_deref() == Some(segment) {
                    continue; // same immediate child as the previous descendant
                }
                last = Some(segment.to_vec());
                children.push(decode_child_segment(segment).ok_or_else(|| corrupt(key))?);
            }
            Ok(children)
        })
    }

    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        read_view!(self, "scan", |table| {
            let mut page = ScanPage::default();
            for entry in table.range::<&[u8]>(path..).map_err(io("scan"))? {
                let (key, value) = entry.map_err(io("scan"))?;
                let key = key.value();
                if !key.starts_with(path) {
                    break;
                }
                if page.entries.len() == limit {
                    page.truncated = true;
                    break;
                }
                page.entries.push((key.to_vec(), value.value().to_vec()));
            }
            Ok(page)
        })
    }

    fn roots(&self) -> Result<Vec<String>, StoreError> {
        read_view!(self, "roots", |table| {
            let mut roots: Vec<String> = Vec::new();
            for entry in table.range::<&[u8]>(..).map_err(io("roots"))? {
                let (key, _) = entry.map_err(io("roots"))?;
                let name = root_name(key.value()).ok_or_else(|| corrupt(key.value()))?;
                if roots.last() != Some(&name) {
                    roots.push(name);
                }
            }
            Ok(roots)
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
