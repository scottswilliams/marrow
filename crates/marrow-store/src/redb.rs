//! Private native persistent ordered-byte engine, over [redb](https://docs.rs/redb).
//!
//! redb's `&[u8]` keys order byte-lexicographically, the same order as the
//! in-memory `BTreeMap`, so range scans need no custom comparator.
//!
//! A transaction holds one redb write transaction for its whole life, so reads
//! inside it see their own writes. Nesting is an undo journal rather than redb
//! savepoints (which cannot be created once a transaction has written): each
//! level records pre-images, so an inner `rollback` replays its journal in
//! reverse, an inner `commit` merges its journal outward, and only the outermost
//! `commit`/`rollback` persists or aborts the redb transaction.

use std::ops::Bound;
use std::panic::AssertUnwindSafe;
use std::path::Path;

use redb::{
    Database, DatabaseError, ReadOnlyDatabase, ReadTransaction, ReadableDatabase, ReadableTable,
    StorageError, Table, TableDefinition, WriteTransaction,
};

use crate::backend::{Backend, ScanPage, StoreError};
use crate::traversal;

const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("marrow");

const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");

/// The on-disk format version this build writes and accepts. A file recording a
/// different version is refused rather than misread; there is no auto-migration.
const FORMAT_VERSION: u32 = 1;

const DELETE_BATCH_LIMIT: usize = 256;

/// A key and the value it held before a change, so a rollback can restore it.
type Undo = (Vec<u8>, Option<Vec<u8>>);

impl<'a> traversal::ScanEntry
    for (
        redb::AccessGuard<'a, &'static [u8]>,
        redb::AccessGuard<'a, &'static [u8]>,
    )
{
    fn key(&self) -> &[u8] {
        self.0.value()
    }

    fn value(&self) -> &[u8] {
        self.1.value()
    }
}

pub(crate) struct RedbStore {
    db: DatabaseHandle,
    /// The live write transaction while one is open (`Some` iff `journals` is
    /// non-empty).
    txn: Option<WriteTransaction>,
    /// One undo log per open nesting level (innermost last).
    journals: Vec<Vec<Undo>>,
    /// A pinned read transaction while a snapshot is held, so reads observe one
    /// consistent version even as later write transactions commit.
    read_view: Option<ReadTransaction>,
}

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

fn io<E: std::fmt::Display>(op: &'static str) -> impl Fn(E) -> StoreError {
    move |error| StoreError::Io {
        op,
        message: error.to_string(),
    }
}

/// Map a redb open error to the store error that faithfully reflects the damage,
/// so a torn body, a recoverable unclean shutdown, and a transient fault are not
/// collapsed into one untyped bucket. redb internals never leak as the surfaced
/// message: Marrow authors its own prose and reports stable typed codes.
///
/// - a second writer is the store lock;
/// - a file left needing repair is recoverable, not corrupt;
/// - reported corruption, and a torn or truncated body (an I/O `InvalidData` or
///   unexpected EOF as redb walks the file), are hard corruption;
/// - anything else is transient I/O.
fn map_open_error(path: &Path, error: DatabaseError) -> StoreError {
    match error {
        DatabaseError::DatabaseAlreadyOpen => StoreError::Locked {
            data_dir: path.to_path_buf(),
        },
        DatabaseError::RepairAborted => StoreError::RecoveryRequired,
        DatabaseError::Storage(storage) => map_storage_error(storage),
        other => StoreError::Io {
            op: "open",
            message: other.to_string(),
        },
    }
}

/// Classify a redb storage error surfaced while opening or probing a store:
/// reported corruption and a torn or truncated body (an I/O `InvalidData` or
/// unexpected EOF as redb walks the file) are hard corruption; anything else is
/// transient I/O. redb internals never become the whole surfaced message.
fn map_storage_error(error: StorageError) -> StoreError {
    match error {
        StorageError::Corrupted(message) => StoreError::Corruption {
            message: format!("the storage engine reported corruption: {message}"),
        },
        StorageError::Io(error) if is_torn_body(&error) => StoreError::Corruption {
            message: "the store body is truncated or torn".into(),
        },
        other => StoreError::Io {
            op: "open",
            message: other.to_string(),
        },
    }
}

/// Whether an I/O error from a store open reflects a damaged on-disk body rather
/// than a transient fault: redb surfaces a truncated or torn file as invalid data
/// or an unexpected end of file while it walks the structure it expects.
fn is_torn_body(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
    )
}

/// Run a store open and its structural probe under a panic backstop.
///
/// redb does not return an error for every damaged file: a truncated or clobbered
/// body drives its open-and-repair path into a layout assertion or an
/// `unreachable!()` in btree traversal, which panics. Marrow builds unwind on
/// panic, so the backstop catches that escape here and converts it into
/// [`StoreError::Corruption`], leaving the process alive and the fault fail-closed
/// with a typed code instead of a redb backtrace on stderr.
///
/// The catch wraps only the open and its probe so it cannot mask an unrelated bug;
/// the closure itself maps redb open errors through [`map_open_error`]. A no-op
/// panic hook is installed for the duration of the open so an expected redb open
/// panic does not print its message and backtrace, then the previous hook is
/// restored — every other panic in the process still reports normally. The hook is
/// process-global, so this is sound only because every store open is serialized on
/// one thread: the CLI runs on a single worker, and serve and lsp open one store at
/// a time. A future concurrent opener would have to serialize this swap.
fn catch_open<T>(open: impl FnOnce() -> Result<T, StoreError>) -> Result<T, StoreError> {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = std::panic::catch_unwind(AssertUnwindSafe(open));
    std::panic::set_hook(previous_hook);
    match caught {
        Ok(result) => result,
        Err(_) => Err(StoreError::Corruption {
            message: "the storage engine could not open the store file".into(),
        }),
    }
}

/// Classify the version recorded in a store's meta table. A missing version is
/// corruption: a store this build wrote always stamps one, and an unstamped file
/// is foreign, not a fresh store (callers stamp fresh stores before this check).
fn check_format_version(recorded: Option<u32>) -> Result<(), StoreError> {
    match recorded {
        Some(FORMAT_VERSION) => Ok(()),
        Some(found) => Err(StoreError::FormatVersion {
            found,
            supported: FORMAT_VERSION,
        }),
        None => Err(StoreError::Corruption {
            message: "store is missing its format version".into(),
        }),
    }
}

/// Stamp the format version on a brand-new file or verify it on an existing one,
/// then ensure the data table exists, in one write transaction. `Database::create`
/// opens existing files too, so a database with no tables is fresh and gets stamped;
/// a non-empty file with no `marrow.meta` is foreign and rejected as corruption
/// rather than adopted. This probe also forces redb to walk the file's structure,
/// so a damaged body surfaces here (as a typed error or a caught panic) rather than
/// on first use.
fn stamp_or_verify_format_version(db: &Database) -> Result<(), StoreError> {
    let write = db.begin_write().map_err(open_transaction_error)?;
    let is_new = write
        .list_tables()
        .map_err(open_storage_error)?
        .next()
        .is_none();
    {
        // Read the recorded version into an owned `Option` so the access guard
        // drops before the `insert` below.
        let mut meta = write.open_table(META).map_err(open_table_error)?;
        let recorded = meta
            .get("format_version")
            .map_err(open_storage_error)?
            .map(|guard| guard.value());
        if recorded.is_none() && is_new {
            meta.insert("format_version", FORMAT_VERSION)
                .map_err(open_storage_error)?;
        } else {
            check_format_version(recorded)?;
        }
    }
    // Create the data table now so later reads never meet a missing table.
    write.open_table(TABLE).map_err(open_table_error)?;
    write.commit().map_err(open_commit_error)
}

/// Verify the recorded format version and data table on an existing-store open.
/// A file with no meta table or no data table is not a complete Marrow store;
/// this path never creates, so it cannot be a fresh one. This probe walks the
/// file structure for the same fail-fast reason as the creating writable open.
fn verify_existing_store_shape(db: &impl ReadableDatabase) -> Result<(), StoreError> {
    let read = db.begin_read().map_err(open_transaction_error)?;
    let recorded = match read.open_table(META) {
        Ok(meta) => meta
            .get("format_version")
            .map_err(open_storage_error)?
            .map(|guard| guard.value()),
        Err(redb::TableError::TableDoesNotExist(_)) => None,
        Err(other) => return Err(open_table_error(other)),
    };
    check_format_version(recorded)?;
    match read.open_table(TABLE) {
        Ok(_) => Ok(()),
        Err(redb::TableError::TableDoesNotExist(_)) => Err(StoreError::Corruption {
            message: "store is missing its data table".into(),
        }),
        Err(other) => Err(open_table_error(other)),
    }
}

fn open_transaction_error(error: redb::TransactionError) -> StoreError {
    match error {
        redb::TransactionError::Storage(storage) => map_storage_error(storage),
        other => StoreError::Io {
            op: "open",
            message: other.to_string(),
        },
    }
}

fn open_table_error(error: redb::TableError) -> StoreError {
    match error {
        redb::TableError::Storage(storage) => map_storage_error(storage),
        other => StoreError::Io {
            op: "open",
            message: other.to_string(),
        },
    }
}

fn open_storage_error(error: StorageError) -> StoreError {
    map_storage_error(error)
}

fn open_commit_error(error: redb::CommitError) -> StoreError {
    match error {
        redb::CommitError::Storage(storage) => map_storage_error(storage),
        other => StoreError::Io {
            op: "open",
            message: other.to_string(),
        },
    }
}

fn delete_key_batch<T>(table: &T, prefix: &[u8]) -> Result<Vec<Vec<u8>>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut keys = Vec::new();
    for entry in table.range::<&[u8]>(prefix..).map_err(io("delete"))? {
        let (key, _) = entry.map_err(io("delete"))?;
        let key = key.value();
        if !key.starts_with(prefix) {
            break;
        }
        keys.push(key.to_vec());
        if keys.len() == DELETE_BATCH_LIMIT {
            break;
        }
    }
    Ok(keys)
}

fn streamed_scan<T>(table: &T, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let range = table.range::<&[u8]>(prefix..).map_err(io("scan"))?;
    traversal::scan(range, prefix, limit, io("scan"))
}

fn streamed_scan_after<T>(
    table: &T,
    prefix: &[u8],
    cursor: &[u8],
    limit: usize,
) -> Result<ScanPage, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let range = table
        .range::<&[u8]>((Bound::Excluded(cursor), Bound::Unbounded))
        .map_err(io("scan_after"))?;
    traversal::scan(range, prefix, limit, io("scan_after"))
}

/// Run a read `$body` over the current read view: the open write transaction's
/// table, the pinned snapshot, or a fresh read transaction. A macro rather than a
/// `&dyn` helper because redb's `ReadableTable` is not object-safe, so the body is
/// monomorphized per table type.
macro_rules! read_view {
    ($self:expr, $op:expr, |$table:ident| $body:expr) => {
        match (&$self.txn, &$self.read_view) {
            // An open write transaction reads its own writes.
            (Some(write), _) => {
                let $table = write.open_table(TABLE).map_err(io($op))?;
                $body
            }
            // A pinned snapshot reads its consistent version.
            (None, Some(read)) => {
                let $table = read.open_table(TABLE).map_err(io($op))?;
                $body
            }
            // Otherwise read the latest committed data.
            (None, None) => {
                let read = $self.db.begin_read($op)?;
                let $table = read.open_table(TABLE).map_err(io($op))?;
                $body
            }
        }
    };
}

impl RedbStore {
    /// Open the redb-backed store at `path`, creating the file if needed. A
    /// second writer is rejected as [`StoreError::Locked`], and a file recording
    /// a different [`FORMAT_VERSION`] as [`StoreError::FormatVersion`]. A damaged
    /// body fails closed as [`StoreError::Corruption`] rather than panicking the
    /// process; the open and its structural probe run under [`catch_open`].
    pub(crate) fn open(path: &Path) -> Result<Self, StoreError> {
        let db = catch_open(|| {
            let db = Database::create(path).map_err(|error| map_open_error(path, error))?;
            stamp_or_verify_format_version(&db)?;
            Ok(db)
        })?;
        Ok(Self {
            db: DatabaseHandle::ReadWrite(db),
            txn: None,
            journals: Vec::new(),
            read_view: None,
        })
    }

    /// Open an existing redb-backed store with write capability, without creating
    /// or adopting missing/non-store data. This is the repair path for a file that
    /// already carries Marrow metadata.
    pub(crate) fn open_existing(path: &Path) -> Result<Self, StoreError> {
        let db = catch_open(|| {
            let db = Database::open(path).map_err(|error| map_open_error(path, error))?;
            verify_existing_store_shape(&db)?;
            Ok(db)
        })?;
        Ok(Self {
            db: DatabaseHandle::ReadWrite(db),
            txn: None,
            journals: Vec::new(),
            read_view: None,
        })
    }

    /// Open an existing store read-only. Unlike [`open`](Self::open) it never
    /// creates the file and only verifies the recorded [`FORMAT_VERSION`] rather
    /// than stamping it; write-capability operations fail before any write
    /// transaction begins. A store left needing repair by an unclean shutdown is
    /// reported as [`StoreError::RecoveryRequired`]: a write-capable open attempts
    /// the replay and reports whether the store opened, so a store damaged beyond
    /// replay still surfaces corruption. The open and its probe run under
    /// [`catch_open`].
    pub(crate) fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let db = catch_open(|| {
            let db = ReadOnlyDatabase::open(path).map_err(|error| map_open_error(path, error))?;
            verify_existing_store_shape(&db)?;
            Ok(db)
        })?;
        Ok(Self {
            db: DatabaseHandle::ReadOnly(db),
            txn: None,
            journals: Vec::new(),
            read_view: None,
        })
    }

    fn record(&mut self, entry: Undo) {
        self.journals
            .last_mut()
            .expect("a journal while a transaction is open")
            .push(entry);
    }

    /// Require a writable handle and no pinned read snapshot. `on_snapshot` names
    /// the snapshot-conflict error for this operation.
    fn ensure_writable(
        &self,
        op: &'static str,
        on_snapshot: fn() -> StoreError,
    ) -> Result<(), StoreError> {
        self.db.require_write_access(op)?;
        if self.read_view.is_some() {
            return Err(on_snapshot());
        }
        Ok(())
    }

    /// Run `mutate` against the current write table. Outside a transaction it is
    /// its own short, immediately durable redb transaction and the reported
    /// preimages are discarded; inside one they join the innermost journal for
    /// rollback.
    fn mutate(
        &mut self,
        op: &'static str,
        mutate: impl FnOnce(&mut Table<&[u8], &[u8]>) -> Result<Vec<Undo>, StoreError>,
    ) -> Result<(), StoreError> {
        if self.txn.is_none() {
            let write = self.db.begin_write(op)?;
            {
                let mut table = write.open_table(TABLE).map_err(io(op))?;
                mutate(&mut table)?;
            }
            return write.commit().map_err(io(op));
        }
        let undo = {
            let write = self.txn.as_ref().expect("an open transaction");
            let mut table = write.open_table(TABLE).map_err(io(op))?;
            mutate(&mut table)?
        };
        for entry in undo {
            self.record(entry);
        }
        Ok(())
    }
}

impl Backend for RedbStore {
    fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        read_view!(self, "read", |table| Ok(table
            .get(key)
            .map_err(io("read"))?
            .map(|guard| guard.value().to_vec())))
    }

    fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.ensure_writable("write", StoreError::write_while_snapshot_pinned)?;
        self.mutate("write", |table| {
            let old = table
                .insert(key, value.as_slice())
                .map_err(io("write"))?
                .map(|guard| guard.value().to_vec());
            Ok(vec![(key.to_vec(), old)])
        })
    }

    fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError> {
        self.ensure_writable("delete", StoreError::delete_while_snapshot_pinned)?;
        self.mutate("delete", |table| {
            // Collect and remove keys in bounded batches so a large prefix subtree
            // never materializes every key at once.
            let mut undo = Vec::new();
            loop {
                let keys = delete_key_batch(&*table, prefix)?;
                if keys.is_empty() {
                    break;
                }
                for key in keys {
                    let old = table
                        .remove(key.as_slice())
                        .map_err(io("delete"))?
                        .map(|guard| guard.value().to_vec());
                    undo.push((key, old));
                }
            }
            Ok(undo)
        })
    }

    fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        read_view!(self, "scan", |table| streamed_scan(&table, prefix, limit))
    }

    fn scan_after(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        read_view!(self, "scan_after", |table| {
            streamed_scan_after(&table, prefix, cursor, limit)
        })
    }

    fn begin(&mut self) -> Result<(), StoreError> {
        self.ensure_writable("begin", StoreError::begin_while_snapshot_pinned)?;
        if self.txn.is_none() {
            self.txn = Some(self.db.begin_write("begin")?);
        }
        self.journals.push(Vec::new());
        Ok(())
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        // No open transaction is a harmless no-op, matching the in-memory store.
        let Some(journal) = self.journals.pop() else {
            return Ok(());
        };
        // An inner commit keeps its writes but moves its undo log outward, so an
        // outer rollback still undoes them; the outermost commit persists redb.
        match self.journals.last_mut() {
            Some(outer) => outer.extend(journal),
            None => {
                let write = self.txn.take().expect("a transaction while committing");
                write.commit().map_err(io("commit"))?;
            }
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), StoreError> {
        // No open transaction is a harmless no-op, matching the in-memory store.
        let Some(journal) = self.journals.pop() else {
            return Ok(());
        };
        if self.journals.is_empty() {
            let write = self.txn.take().expect("a transaction while rolling back");
            write.abort().map_err(io("rollback"))?;
            return Ok(());
        }
        // Undo this level in reverse against the open transaction, leaving outer
        // levels in place.
        let write = self.txn.as_ref().expect("a transaction while rolling back");
        let mut table = write.open_table(TABLE).map_err(io("rollback"))?;
        for (key, old) in journal.into_iter().rev() {
            match old {
                Some(value) => {
                    table
                        .insert(key.as_slice(), value.as_slice())
                        .map_err(io("rollback"))?;
                }
                None => {
                    table.remove(key.as_slice()).map_err(io("rollback"))?;
                }
            }
        }
        Ok(())
    }

    fn begin_snapshot(&mut self) -> Result<(), StoreError> {
        if self.txn.is_some() {
            return Err(StoreError::snapshot_while_transaction_open());
        }
        if self.read_view.is_some() {
            return Err(StoreError::snapshot_already_pinned());
        }
        // A redb read transaction is a stable version unaffected by later writes.
        self.read_view = Some(self.db.begin_read("snapshot")?);
        Ok(())
    }

    fn end_snapshot(&mut self) {
        self.read_view = None;
    }
}

#[cfg(test)]
mod tests {
    use redb::{Database, ReadableDatabase, TableDefinition};

    use super::{
        DELETE_BATCH_LIMIT, DatabaseHandle, FORMAT_VERSION, META, RedbStore, TABLE, catch_open,
        map_open_error,
    };
    use crate::backend::{Backend, StoreError};
    use crate::conformance;

    /// A redb panic during open or repair must not abort the process: the backstop
    /// converts it into typed corruption. Proven by injecting a panicking open so the
    /// catch is exercised even without a file that forces redb's exact `unreachable!`.
    #[test]
    fn catch_open_converts_a_panicking_open_into_corruption() {
        let result: Result<(), StoreError> = catch_open(|| panic!("redb unreachable during open"));
        match result {
            Err(StoreError::Corruption { .. }) => {}
            other => panic!("expected corruption from a caught open panic, got {other:?}"),
        }
    }

    /// A non-panicking closure passes its result through unchanged, so the backstop
    /// adds no behavior beyond catching a panic.
    #[test]
    fn catch_open_passes_a_clean_result_through() {
        assert_eq!(catch_open(|| Ok(7)).expect("clean open"), 7);
    }

    /// The redb-error mapping is damage-faithful: a recoverable unclean shutdown, a
    /// reported corruption, a torn body, a second writer, and a transient fault each
    /// land on their own typed code instead of collapsing to `store.io`.
    #[test]
    fn map_open_error_classifies_each_redb_failure() {
        let path = std::path::Path::new("/tmp/marrow-store.redb");

        assert_eq!(
            map_open_error(path, redb::DatabaseError::RepairAborted).code(),
            "store.recovery_required"
        );
        assert_eq!(
            map_open_error(
                path,
                redb::DatabaseError::Storage(redb::StorageError::Corrupted("torn page".into()))
            )
            .code(),
            "store.corruption"
        );
        assert_eq!(
            map_open_error(
                path,
                redb::DatabaseError::Storage(redb::StorageError::Io(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof
                )))
            )
            .code(),
            "store.corruption"
        );
        match map_open_error(path, redb::DatabaseError::DatabaseAlreadyOpen) {
            StoreError::Locked { data_dir } => assert_eq!(data_dir, path),
            other => panic!("expected store.locked, got {other:?}"),
        }
        assert_eq!(
            map_open_error(
                path,
                redb::DatabaseError::Storage(redb::StorageError::Io(std::io::Error::from(
                    std::io::ErrorKind::PermissionDenied
                )))
            )
            .code(),
            "store.io"
        );
    }

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
