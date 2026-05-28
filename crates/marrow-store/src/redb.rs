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

use crate::backend::Backend;
use crate::mem::{Presence, ScanPage, StoreError};
use crate::path::{ChildSegment, decode_child_segment, root_name, segment_len};

/// The single table holding every encoded (path, value) pair.
const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("marrow");

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
    /// Open the store at `path`, creating the redb file and table if needed.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let db = Database::create(path).map_err(io("open"))?;
        // Create the table now so later reads never meet a missing table.
        let write = db.begin_write().map_err(io("open"))?;
        write.open_table(TABLE).map_err(io("open"))?;
        write.commit().map_err(io("open"))?;
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

    fn begin(&mut self) -> Result<(), StoreError> {
        if self.txn.is_none() {
            self.txn = Some(self.db.begin_write().map_err(io("begin"))?);
        }
        self.journals.push(Vec::new());
        Ok(())
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        let Some(journal) = self.journals.pop() else {
            return Err(io("commit")("no open transaction"));
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
        let Some(journal) = self.journals.pop() else {
            return Err(io("rollback")("no open transaction"));
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
