//! The native persistent ordered-byte engine, over [redb](https://docs.rs/redb).
//!
//! redb's `&[u8]` keys order byte-lexicographically, the same order as the
//! in-memory `BTreeMap`, so range scans need no custom comparator. A
//! [`NativeEngine`] hands out a [`RedbView`] backed by a redb read transaction (a
//! stable version, so its reads are coherent) and one [`RedbTxn`] backed by a
//! redb write transaction (which reads its own staged writes and either commits
//! durably or aborts).
//!
//! ## Filesystem durability envelope
//!
//! The engine's native recovery and sync sit above a documented filesystem
//! contract; where the filesystem does not honor it, durability is not
//! guaranteed and no software layer here can restore it:
//!
//! - redb's only durability primitive is the standard library's `sync_data()`,
//!   i.e. `fsync(2)` (`fdatasync` where available). It issues **no**
//!   `F_FULLFSYNC` anywhere, so a confirmed commit is durable across a **process
//!   kill and an OS crash** — the engine flushes to the kernel and the kernel
//!   owns the page cache — but **not** across **power loss or a drive-cache
//!   reset**, where a drive may acknowledge an `fsync` before the bytes reach
//!   stable media. Default SQLite behaves identically. Routing commits through
//!   `F_FULLFSYNC` for power-loss durability is a later lifecycle
//!   (native-attach / audit) and operations-documentation decision, not this
//!   contract.
//! - `fsync` on a directory makes a new directory entry durable. The engine
//!   commits with [`Durability::Immediate`], and a fresh store's parent
//!   directory is fsynced after the create commit ([`sync_parent_directory`]).
//! - A rename is atomic and a `create_new` open is exclusive, so a partly-formed
//!   store is never mistaken for a complete one.
//! - The filesystem does not silently reorder or drop already-`fsync`ed data. A
//!   torn or truncated body is surfaced as [`StoreError::Corruption`] rather
//!   than misread; an unclean shutdown that left a repairable log is surfaced as
//!   [`StoreError::RecoveryRequired`] and replayed only by a write-capable open.
//!   The fast open path does **not** re-verify page checksums, so an external
//!   bit-flip on live bytes reads back silently altered on a clean open; the
//!   bounded [`ByteEngine::audit_integrity`](crate::ByteEngine::audit_integrity)
//!   walk is the primitive that catches it.
//!
//! The engine does not parse redb's pages, replace process-global hooks, or
//! assume any durability the filesystem does not provide. The adapter and the
//! Marrow workspace contain no `unsafe`; redb ships reviewed internal `unsafe`
//! (chiefly its xxHash3 checksums), so a corrupt or externally-mutated body is
//! contained at the adapter boundary ([`contain_panic`]) rather than trusted to
//! fail gracefully.

use std::fs;
use std::marker::PhantomData;
use std::ops::Bound;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use redb::{
    Database, DatabaseError, Durability, ReadOnlyDatabase, ReadTransaction, ReadableDatabase,
    ReadableTable, StorageError, TableDefinition, WriteTransaction,
};

use crate::engine::{ByteEngine, Cell, CommitOutcome, ReadView, WriteTxn, check_cell_limits};
use crate::error::StoreError;
use crate::traversal;

const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("marrow");

const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");

/// The on-disk format version this build writes and accepts. A file recording a
/// different version is refused rather than misread; there is no auto-migration.
const FORMAT_VERSION: u32 = 1;

const MARROW_REDB_DURABILITY: Durability = Durability::Immediate;

#[cfg(unix)]
const STORE_SYMLINK_HOP_LIMIT: usize = 40;

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

/// A redb-backed native ordered-byte engine, durable across processes.
pub struct NativeEngine {
    db: DatabaseHandle,
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
            Self::ReadWrite(db) => {
                let mut write = db.begin_write().map_err(io(op))?;
                pin_write_durability(&mut write, op)?;
                Ok(write)
            }
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

fn pin_write_durability(write: &mut WriteTransaction, op: &'static str) -> Result<(), StoreError> {
    write.set_durability(MARROW_REDB_DURABILITY).map_err(io(op))
}

fn io<E: std::fmt::Display>(op: &'static str) -> impl Fn(E) -> StoreError {
    move |error| StoreError::Io {
        op,
        message: error.to_string(),
    }
}

/// Contain a panic from the redb dependency at the adapter boundary. redb asserts
/// internally on some externally-mutated files rather than returning `Err`, so an
/// operation over a corrupt body can unwind instead of failing cleanly; this
/// converts that unwind into a typed corruption error. It is adapter-boundary
/// containment of one dependency's panic policy over a bounded call, not a
/// process-global panic hook (the deleted `B00` hook swap stays forbidden). The
/// engine itself contains no `unsafe`; redb ships reviewed internal `unsafe`, so
/// a corrupt body is contained here rather than trusted to fail gracefully.
fn contain_panic<T>(
    op: &'static str,
    body: impl FnOnce() -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(_) => Err(StoreError::Corruption {
            message: format!(
                "the storage engine panicked during {op}; the store body is corrupt or was \
                 modified externally"
            ),
        }),
    }
}

/// Open a redb database with write capability, retrying briefly past a transient
/// advisory-lock release lag.
///
/// redb guards a store with an OS file lock (`flock` on Unix) acquired on open and
/// released when the handle drops. The release can lag the close that the drop
/// performs, so a write-capable open issued just after another handle on the same
/// path was dropped — by this process or by one that has already exited, such as a
/// preceding read-only inspection — can observe the not-yet-released lock and fail
/// with `DatabaseAlreadyOpen` even though no holder remains. A genuine concurrent
/// holder keeps the lock for the whole budget and still surfaces promptly as
/// [`StoreError::Locked`]; only the brief release lag is absorbed.
fn open_write_capable(
    path: &Path,
    open: impl Fn() -> Result<Database, DatabaseError>,
) -> Result<Database, StoreError> {
    const LOCK_RELEASE_BACKOFF: [u64; 4] = [1, 2, 4, 8];
    let mut attempt = 0;
    loop {
        match open() {
            Ok(db) => return Ok(db),
            Err(DatabaseError::DatabaseAlreadyOpen) if attempt < LOCK_RELEASE_BACKOFF.len() => {
                std::thread::sleep(std::time::Duration::from_millis(
                    LOCK_RELEASE_BACKOFF[attempt],
                ));
                attempt += 1;
            }
            Err(error) => return Err(map_open_error(path, error)),
        }
    }
}

/// Map a redb open error to the store error that faithfully reflects the damage,
/// so a torn body, a recoverable unclean shutdown, and a transient fault are not
/// collapsed into one untyped bucket. redb internals never leak as the surfaced
/// message: Marrow authors its own prose and reports stable typed codes.
///
/// - a second writer, or a writer racing a read-only open in either direction, is
///   the store lock;
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
        // A denied open is its own path-bearing state: the fix is to grant access, not retry.
        DatabaseError::Storage(StorageError::Io(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            StoreError::PermissionDenied {
                path: path.to_path_buf(),
            }
        }
        DatabaseError::Storage(storage) => map_storage_error(storage),
        _ => transient_open_io(),
    }
}

/// The store error for a transient open fault, reported in place of the OS error
/// string the engine or filesystem produced. That string embeds the platform
/// errno — an `ELOOP` symlink loop or a dangling `ENOENT` target each surface as a
/// transient fault — and a surfaced `message` is render-only prose, never machine
/// detail for a client to parse.
fn transient_open_io() -> StoreError {
    StoreError::Io {
        op: "open",
        message: "the store file could not be opened; the path may be unreachable or temporarily \
                  unavailable"
            .into(),
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
        _ => transient_open_io(),
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

/// Run a read-only or existing-store open, absorbing the window in which a concurrent
/// creator has the store file only part-formed. redb lays a store's header down under
/// the lock and writes its magic last, so a first-run creation, or a delete-and-create
/// recreation two or more writers race on distinct inodes, can momentarily leave the
/// path a header-absent placeholder or a torn intermediate that already bears the
/// magic. A corruption an open reports against such a store file is retried across a
/// brief budget: a live race settles to the creator's finished store, its lock, or a
/// transient fault within it. A settled writer-free torn or incomplete file keeps
/// returning corruption on every attempt, so it still surfaces once the budget is
/// spent.
fn open_tolerating_creation_race(
    path: &Path,
    open: impl Fn() -> Result<DatabaseHandle, StoreError>,
) -> Result<DatabaseHandle, StoreError> {
    const CREATION_RACE_BACKOFF: [u64; 4] = [1, 2, 4, 8];
    let mut attempt = 0;
    loop {
        match open() {
            Err(StoreError::Corruption { .. })
                if attempt < CREATION_RACE_BACKOFF.len() && store_file_may_be_forming(path) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(
                    CREATION_RACE_BACKOFF[attempt],
                ));
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Whether a corruption an open reported for this path may be a transient artifact of
/// a concurrent creator still forming the store rather than settled damage. The header
/// is laid down under the lock with the magic written last, and a delete-and-create
/// recreation raced on distinct inodes can leave the file header-absent or a torn
/// intermediate that already bears the magic. Neither state is distinguishable from
/// settled damage by a cheap probe once the open has already failed, so a corruption
/// against any regular store file is retried; a settled torn store keeps failing every
/// attempt and surfaces once the retry budget is spent. A non-regular path (a FIFO,
/// socket, or directory) or a missing file is not a store under construction.
fn store_file_may_be_forming(path: &Path) -> bool {
    matches!(fs::metadata(path), Ok(metadata) if metadata.file_type().is_file())
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

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = fs::File::open(parent).map_err(io("sync_parent_dir"))?;
    directory.sync_all().map_err(io("sync_parent_dir"))
}

#[cfg(windows)]
fn sync_parent_directory(path: &Path) -> Result<(), StoreError> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(parent)
        .map_err(io("sync_parent_dir"))?;
    directory.sync_all().map_err(io("sync_parent_dir"))
}

#[cfg(not(any(unix, windows)))]
fn sync_parent_directory(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

/// Reject a store path that resolves to an existing non-regular file before any
/// handle is opened. redb opens the store file `O_RDWR`, so a FIFO, socket, or
/// device at the path can block the open syscall indefinitely (a FIFO with no
/// writer) or drive the engine through a body it can never lay out. A regular file
/// is the only valid store body; anything else is treated as corruption, located at
/// the path, so every open path fails closed promptly with a typed diagnostic. A
/// missing path is left to the caller: creation handles it, and an existing-only
/// open surfaces its own not-found error.
fn guard_regular_store_file(path: &Path) -> Result<(), StoreError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(StoreError::Corruption {
            message: "store path is not a regular file".into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(StoreError::PermissionDenied {
                path: path.to_path_buf(),
            })
        }
        Err(_) => Err(transient_open_io()),
    }
}

#[cfg(unix)]
fn prepare_new_store_file(path: &Path) -> Result<Option<std::path::PathBuf>, StoreError> {
    let Some(create_path) = missing_file_or_symlink_target(path)? else {
        return Ok(None);
    };
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&create_path)
    {
        Ok(file) => {
            drop(file);
            Ok(Some(create_path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(StoreError::PermissionDenied {
                path: path.to_path_buf(),
            })
        }
        Err(_) => Err(transient_open_io()),
    }
}

#[cfg(not(unix))]
fn prepare_new_store_file(path: &Path) -> Result<Option<std::path::PathBuf>, StoreError> {
    Ok((!path.exists()).then(|| path.to_path_buf()))
}

#[cfg(unix)]
fn missing_file_or_symlink_target(path: &Path) -> Result<Option<std::path::PathBuf>, StoreError> {
    let mut path = path.to_path_buf();
    let mut visited = Vec::new();
    for _ in 0..STORE_SYMLINK_HOP_LIMIT {
        if visited.iter().any(|visited| visited == &path) {
            return Ok(None);
        }
        visited.push(path.clone());
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Some(path)),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(StoreError::PermissionDenied { path: path.clone() });
            }
            Err(_) => return Err(transient_open_io()),
        };
        if !metadata.file_type().is_symlink() {
            return Ok(None);
        }
        let target = fs::read_link(&path).map_err(|_| transient_open_io())?;
        path = resolve_link_target(&path, target);
    }
    Ok(None)
}

#[cfg(unix)]
fn resolve_link_target(link_path: &Path, target: std::path::PathBuf) -> std::path::PathBuf {
    if target.is_absolute() {
        target
    } else {
        link_path
            .parent()
            .map_or_else(|| target.clone(), |parent| parent.join(&target))
    }
}

/// Stamp the format version on a brand-new file or verify it on an existing one,
/// then ensure the data table exists, in one write transaction. `Database::create`
/// opens existing files too, so a database with no tables is fresh and gets stamped;
/// a non-empty file with no `marrow.meta` is foreign and rejected as corruption
/// rather than adopted. This probe also forces redb to walk the file's structure,
/// so a damaged body surfaces here (as a typed error or a caught panic) rather than
/// on first use.
fn stamp_or_verify_format_version(
    sync_parent_after_commit: Option<&Path>,
    db: &Database,
) -> Result<(), StoreError> {
    let mut write = db.begin_write().map_err(open_transaction_error)?;
    pin_write_durability(&mut write, "open")?;
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
    write.commit().map_err(open_commit_error)?;
    if let Some(created_path) = sync_parent_after_commit {
        sync_parent_directory(created_path)?;
    }
    Ok(())
}

/// Verify the recorded format version and data table on an existing-store open.
/// A file with no meta table or no data table is not a complete Marrow store;
/// this path never creates, so it cannot be a fresh one. Damage below the table
/// roots is not probed here: redb walks those pages lazily, so it surfaces as a
/// typed [`StoreError`] when a read traverses the tree.
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

impl NativeEngine {
    /// Open the redb-backed store at `path`, creating the file if needed. A
    /// concurrent read-only or read-write holder is rejected as
    /// [`StoreError::Locked`], and a file recording a different [`FORMAT_VERSION`]
    /// as [`StoreError::FormatVersion`]. A brand-new file is stamped with the
    /// current format version; an existing complete store is verified. A malformed
    /// body surfaces redb's own open error as a typed [`StoreError`] through
    /// [`map_open_error`].
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        contain_panic("open", || {
            guard_regular_store_file(path)?;
            let sync_parent_after_commit = prepare_new_store_file(path)?;
            let db = open_write_capable(path, || Database::create(path))?;
            stamp_or_verify_format_version(sync_parent_after_commit.as_deref(), &db)?;
            Ok(Self {
                db: DatabaseHandle::ReadWrite(db),
            })
        })
    }

    /// Open an existing store read-only. Unlike [`open`](Self::open) it never
    /// creates the file and only verifies the recorded [`FORMAT_VERSION`] rather
    /// than stamping it; write-capability operations fail before any write
    /// transaction begins. A malformed body surfaces redb's own open error as a
    /// typed [`StoreError`] through [`map_open_error`].
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        contain_panic("open", || {
            let db = open_tolerating_creation_race(path, || {
                guard_regular_store_file(path)?;
                let db =
                    ReadOnlyDatabase::open(path).map_err(|error| map_open_error(path, error))?;
                verify_existing_store_shape(&db)?;
                Ok(DatabaseHandle::ReadOnly(db))
            })?;
            Ok(Self { db })
        })
    }
}

impl ByteEngine for NativeEngine {
    type View<'a> = RedbView<'a>;
    type Txn<'a> = RedbTxn<'a>;

    fn read_view(&self) -> Result<RedbView<'_>, StoreError> {
        Ok(RedbView {
            read: self.db.begin_read("read_view")?,
            _engine: PhantomData,
        })
    }

    fn begin(&mut self) -> Result<RedbTxn<'_>, StoreError> {
        Ok(RedbTxn {
            write: Some(self.db.begin_write("begin")?),
            _engine: PhantomData,
        })
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        self.db.require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        match &mut self.db {
            DatabaseHandle::ReadWrite(db) => contain_panic("audit", || {
                match db
                    .check_integrity()
                    .map_err(|error| map_open_error(Path::new(""), error))
                {
                    // The full Merkle walk passed, or found and repaired damage: a
                    // repaired store had been externally modified, which the audit
                    // reports as corruption rather than silently accepting.
                    Ok(true) => Ok(()),
                    Ok(false) => Err(StoreError::Corruption {
                        message: "integrity audit found and repaired external damage".into(),
                    }),
                    Err(error) => Err(error),
                }
            }),
            DatabaseHandle::ReadOnly(_) => Err(StoreError::ReadOnly { op: "audit" }),
        }
    }
}

/// A coherent read view over a redb read transaction — a stable version whose
/// reads are unaffected by later commits. Bound to the engine borrow that
/// produced it, so no write can interleave for its life.
pub struct RedbView<'a> {
    read: ReadTransaction,
    _engine: PhantomData<&'a NativeEngine>,
}

impl ReadView for RedbView<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        contain_panic("read", || {
            let table = self.read.open_table(TABLE).map_err(io("read"))?;
            Ok(table
                .get(key)
                .map_err(io("read"))?
                .map(|guard| guard.value().to_vec()))
        })
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        contain_panic("scan_after", || {
            let table = self.read.open_table(TABLE).map_err(io("scan_after"))?;
            scan_after_table(&table, prefix, cursor)
        })
    }
}

/// A redb write transaction. Reads observe its own staged writes; it commits
/// durably or, on drop, aborts. Borrows the engine mutably, so a second
/// transaction cannot be named while it is live.
pub struct RedbTxn<'a> {
    write: Option<WriteTransaction>,
    _engine: PhantomData<&'a mut NativeEngine>,
}

impl RedbTxn<'_> {
    fn write(&self) -> &WriteTransaction {
        self.write
            .as_ref()
            .expect("write transaction is live until commit or drop")
    }
}

impl ReadView for RedbTxn<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        contain_panic("read", || {
            let table = self.write().open_table(TABLE).map_err(io("read"))?;
            Ok(table
                .get(key)
                .map_err(io("read"))?
                .map(|guard| guard.value().to_vec()))
        })
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        contain_panic("scan_after", || {
            let table = self.write().open_table(TABLE).map_err(io("scan_after"))?;
            scan_after_table(&table, prefix, cursor)
        })
    }
}

impl WriteTxn for RedbTxn<'_> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        check_cell_limits(key, &value)?;
        let mut table = self.write().open_table(TABLE).map_err(io("put"))?;
        table.insert(key, value.as_slice()).map_err(io("put"))?;
        Ok(())
    }

    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        let mut table = self.write().open_table(TABLE).map_err(io("remove"))?;
        table.remove(key).map_err(io("remove"))?;
        Ok(())
    }

    fn commit(mut self) -> CommitOutcome {
        let Some(write) = self.write.take() else {
            return CommitOutcome::Aborted;
        };
        // A commit failure — a returned error or a contained panic over a corrupt
        // body — leaves durability unknown: the write may or may not have reached
        // disk, so the caller must close and reclassify on reopen rather than
        // retry.
        match contain_panic("commit", || write.commit().map_err(io("commit"))) {
            Ok(()) => CommitOutcome::Confirmed,
            Err(_) => CommitOutcome::Indeterminate,
        }
    }
}

impl Drop for RedbTxn<'_> {
    fn drop(&mut self) {
        if let Some(write) = self.write.take() {
            let _ = write.abort();
        }
    }
}

/// Collect the cells under `prefix` strictly after `cursor` from a readable redb
/// table, bounded by the shared scan limits.
fn scan_after_table<T>(table: &T, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let range = table
        .range::<&[u8]>((Bound::Excluded(cursor), Bound::Unbounded))
        .map_err(io("scan_after"))?;
    traversal::collect_after(range, prefix, io("scan_after"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use redb::{Database, ReadableDatabase, TableDefinition};

    use super::{FORMAT_VERSION, META, NativeEngine, TABLE, map_open_error};
    use crate::conformance;
    use crate::engine::{ByteEngine, CommitOutcome, ReadView, WriteTxn};
    use crate::error::StoreError;

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug)]
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> std::io::Result<Self> {
            let base = std::env::temp_dir();
            let process = std::process::id();
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for attempt in 0..128u64 {
                let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = base.join(format!("{prefix}-{process}-{nonce}-{counter}-{attempt}"));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Ok(Self { path }),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate a unique temp dir",
            ))
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn redb_write_policy_is_pinned_and_fresh_store_creation_syncs_the_parent() {
        let source = include_str!("redb.rs");
        let durability_pin = ["set", "_durability(MARROW_REDB_DURABILITY)"].concat();
        let two_phase = ["set", "_two_phase_commit("].concat();

        assert!(
            source.contains(&durability_pin),
            "redb write transactions must explicitly pin Marrow's durability policy"
        );
        assert!(
            source.contains("sync_parent_directory(created_path)?"),
            "fresh native store creation must fsync the containing directory"
        );
        assert!(
            !source.contains(&two_phase),
            "W2.2 documents one-phase redb commit; do not enable two-phase commit here"
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_symlink_target_detection_stops_relative_cycles() {
        let root = TempDir::new("redb-symlink-cycle").expect("temp dir");
        let data_dir = root.path().join(".data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        let store_path = data_dir.join("marrow.redb");
        std::os::unix::fs::symlink("../.data/marrow.redb", &store_path)
            .expect("create relative symlink cycle");

        assert_eq!(
            super::missing_file_or_symlink_target(&store_path).expect("resolve symlink target"),
            None,
            "relative symlink cycles must not spin while preparing owner-only creation"
        );
    }

    /// An unreadable store file — a regular store body whose mode denies access while
    /// its parent directory stays searchable — is a permission fault, not a transient
    /// I/O blip. The engine open on the denied body must carry the typed
    /// `store.permission_denied` code and name the path on every open path, never
    /// collapse into the `store.io` catch-all with a raw errno.
    #[cfg(unix)]
    #[test]
    fn opening_a_denied_store_file_is_permission_denied_on_every_open_path() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("marrow-store-redb-denied-file").expect("temp dir");
        let path = dir.path().join("marrow.redb");
        {
            let mut store = NativeEngine::open(&path).expect("create fresh store");
            let mut txn = store.begin().expect("begin");
            txn.put(b"k", b"v".to_vec()).expect("write");
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
            .expect("deny access to the store file");

        for result in [
            NativeEngine::open(&path).map(|_| ()),
            NativeEngine::open_read_only(&path).map(|_| ()),
        ] {
            match result {
                Err(StoreError::PermissionDenied { path: reported }) => {
                    assert_eq!(reported, path);
                }
                other => {
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
                    panic!("a denied store file must be permission_denied, got {other:?}");
                }
            }
        }

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
    }

    /// A store path that is a symlink loop (`ELOOP`) or a dangling symlink to a
    /// missing target (`ENOENT`) fails closed as the transient `store.io` on every
    /// open path, and the rendered message never embeds the platform errno the OS
    /// error string carries.
    #[cfg(unix)]
    #[test]
    fn opening_a_symlink_loop_or_dangling_target_is_io_without_a_raw_errno() {
        let dir = TempDir::new("marrow-store-redb-symlink").expect("temp dir");

        let loop_a = dir.path().join("loop-a.redb");
        let loop_b = dir.path().join("loop-b.redb");
        std::os::unix::fs::symlink(&loop_b, &loop_a).expect("link a -> b");
        std::os::unix::fs::symlink(&loop_a, &loop_b).expect("link b -> a");

        let dangling = dir.path().join("dangling.redb");
        std::os::unix::fs::symlink(dir.path().join("absent.redb"), &dangling)
            .expect("link to a missing target");

        let expect_io = |result: Result<(), StoreError>, label: &str| match result {
            Err(error @ StoreError::Io { .. }) => assert!(
                !error.to_string().contains("(os error"),
                "store.io message must not leak the OS errno ({label}): {error}"
            ),
            other => panic!("expected store.io ({label}), got {other:?}"),
        };

        // A symlink loop is rejected before any handle opens, on every open path.
        expect_io(NativeEngine::open(&loop_a).map(|_| ()), "loop open");
        expect_io(
            NativeEngine::open_read_only(&loop_a).map(|_| ()),
            "loop read-only",
        );

        // A dangling target is a missing store to the read-only open; `open`
        // creates the target, so only inspection surfaces the fault.
        expect_io(
            NativeEngine::open_read_only(&dangling).map(|_| ()),
            "dangling read-only",
        );
    }

    /// The redb-error mapping is damage-faithful: a recoverable unclean shutdown, a
    /// reported corruption, a torn body, a read/write lock conflict, a denied open, and a
    /// transient fault each land on their own typed code instead of collapsing to `store.io`.
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
        match map_open_error(
            path,
            redb::DatabaseError::Storage(redb::StorageError::Io(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            ))),
        ) {
            StoreError::PermissionDenied { path: reported } => assert_eq!(reported, path),
            other => panic!("expected store.permission_denied, got {other:?}"),
        }
    }

    /// The native store satisfies the same backend conformance suite as the
    /// in-memory store — one contract, two backends.
    #[test]
    fn redb_store_passes_the_conformance_suite() -> Result<(), StoreError> {
        let dir = TempDir::new("marrow-store-redb-test").map_err(|error| StoreError::Io {
            op: "create temp dir",
            message: error.to_string(),
        })?;
        let mut counter = 0;
        conformance::run_all(|| {
            // Each law gets a fresh redb file in the shared temp dir; the dir (and
            // its files) outlives every store, dropping only when the test ends.
            counter += 1;
            let path = dir.path().join(format!("store-{counter}.redb"));
            NativeEngine::open(&path)
        })
    }

    /// A fresh store passes its integrity audit, and an externally byte-mutated
    /// store body is rejected by the audit — as a returned typed error or a
    /// contained panic — rather than read back silently altered or crashing.
    #[test]
    fn audit_detects_external_corruption_without_crashing() {
        use std::io::{Read, Seek, SeekFrom, Write};

        let dir = TempDir::new("marrow-store-redb-audit").expect("temp dir");
        let path = dir.path().join("audit.redb");
        {
            let mut store = NativeEngine::open(&path).expect("open fresh");
            let mut txn = store.begin().expect("begin");
            for n in 0..64u32 {
                txn.put(format!("k{n:03}").as_bytes(), vec![n as u8; 32])
                    .expect("put");
            }
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
            store
                .audit_integrity()
                .expect("a fresh store passes its audit");
        }

        // Flip a spread of live bytes in the store body out from under redb.
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("reopen store file");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read body");
        for offset in (0..bytes.len()).step_by(97) {
            bytes[offset] ^= 0xFF;
        }
        file.seek(SeekFrom::Start(0)).expect("seek");
        file.write_all(&bytes).expect("write mutated body");
        file.sync_all().expect("sync mutated body");
        drop(file);

        // Opening and auditing a corrupted body must fail closed with a typed
        // error, never a process panic.
        let audited = std::panic::catch_unwind(|| match NativeEngine::open(&path) {
            Ok(mut store) => store.audit_integrity(),
            Err(error) => Err(error),
        });
        match audited {
            Ok(Err(_)) => {}
            Ok(Ok(())) => panic!("a byte-mutated store must not pass the integrity audit"),
            Err(_) => panic!("the adapter must contain redb's panic as a typed error"),
        }
    }

    /// The memory and redb engines compute the same ordered-byte algebra: an
    /// identical put/remove sequence, read back by point `get` and by `scan_after`
    /// at the boundary minus/at/plus each key, agrees cell-for-cell across both.
    #[test]
    fn memory_and_redb_agree_byte_for_byte() {
        use crate::MemoryEngine;

        fn apply<E: ByteEngine>(engine: &mut E) -> Vec<(Vec<u8>, Vec<u8>)> {
            {
                let mut txn = engine.begin().expect("begin");
                for n in 0..40u32 {
                    let key = format!("\x30{:02}", (n * 3) % 20).into_bytes();
                    if n.is_multiple_of(4) {
                        txn.remove(&key).expect("remove");
                    } else {
                        txn.put(&key, format!("v{n}").into_bytes()).expect("put");
                    }
                }
                assert_eq!(txn.commit(), CommitOutcome::Confirmed);
            }
            let view = engine.read_view().expect("view");
            // Page the whole \x30 range and probe boundary cursors around each key.
            let mut all = Vec::new();
            let mut cursor = b"\x30".to_vec();
            loop {
                let page = view.scan_after(b"\x30", &cursor).expect("scan");
                let Some((last, _)) = page.last().cloned() else {
                    break;
                };
                cursor = last;
                all.extend(page);
            }
            for (key, _) in all.clone() {
                let mut minus = key.clone();
                *minus.last_mut().unwrap() -= 1;
                // A cursor just below a key includes it; at it excludes it.
                assert!(
                    view.scan_after(b"\x30", &minus)
                        .expect("minus")
                        .iter()
                        .any(|(k, _)| *k == key)
                );
                assert!(
                    view.scan_after(b"\x30", &key)
                        .expect("at")
                        .iter()
                        .all(|(k, _)| *k != key)
                );
            }
            all
        }

        let mem = apply(&mut MemoryEngine::new());

        let dir = TempDir::new("marrow-store-redb-diff").expect("temp dir");
        let path = dir.path().join("diff.redb");
        let native = apply(&mut NativeEngine::open(&path).expect("open native"));

        assert_eq!(mem, native, "memory and redb disagree on the byte algebra");
    }

    #[test]
    fn redb_read_transactions_are_stable_snapshots() {
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
        let path = dir.path().join("snapshot.redb");
        let key: &[u8] = b"k";
        let old: &[u8] = b"old";
        let new: &[u8] = b"new";

        let mut store = NativeEngine::open(&path).expect("open");
        {
            let mut txn = store.begin().expect("begin");
            txn.put(key, old.to_vec()).expect("seed old value");
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        drop(store);
        let db = Database::open(&path).expect("reopen raw redb handle");

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
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
        let path = dir.path().join("aborted-write.redb");

        drop(NativeEngine::open(&path).expect("open"));
        let db = Database::open(&path).expect("reopen raw redb handle");

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
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
        let path = dir.path().join("ordered-bytes.redb");

        drop(NativeEngine::open(&path).expect("open"));
        let db = Database::open(&path).expect("reopen raw redb handle");

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
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
        let path = dir.path().join("foreign.redb");

        // Build a non-empty redb file with some other table and no `marrow.meta`.
        {
            let db = Database::create(&path).expect("create foreign db");
            let write = db.begin_write().expect("begin");
            const OTHER: TableDefinition<&str, u32> = TableDefinition::new("not.marrow");
            write.open_table(OTHER).expect("open foreign table");
            write.commit().expect("commit foreign db");
        }

        match NativeEngine::open(&path) {
            Err(StoreError::Corruption { .. }) => {}
            Err(other) => panic!("expected corruption for a meta-less file, got {other:?}"),
            Ok(_) => panic!("a meta-less file must not be adopted as a Marrow store"),
        }
    }

    #[test]
    fn open_rejects_unsupported_format_version_with_typed_error() {
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
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

        for result in [
            NativeEngine::open(&path),
            NativeEngine::open_read_only(&path),
        ] {
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
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
        let path = dir.path().join("fresh.redb");
        {
            let mut store = NativeEngine::open(&path).expect("create fresh");
            let mut txn = store.begin().expect("begin");
            txn.put(b"k", b"v".to_vec()).expect("write");
            assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        }
        let store = NativeEngine::open(&path).expect("reopen stamped store");
        assert_eq!(
            store
                .read_view()
                .expect("read view")
                .get(b"k")
                .expect("read"),
            Some(b"v".to_vec())
        );
    }

    /// A store path that is a FIFO (or any other non-regular file) must fail closed
    /// with a typed corruption diagnostic on every open path rather than blocking
    /// forever in the `open()` syscall waiting for a writer. The open runs on a worker
    /// thread with a deadline so a regression surfaces as a timeout, not a hung suite.
    #[cfg(unix)]
    #[test]
    fn opening_a_fifo_store_fails_closed_without_blocking() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = TempDir::new("marrow-store-redb-fifo").expect("temp dir");
        let path = dir.path().join("marrow.redb");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("spawn mkfifo");
        assert!(status.success(), "mkfifo failed");

        for label in ["open", "open_read_only"] {
            let path = path.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let result = match label {
                    "open" => NativeEngine::open(&path),
                    _ => NativeEngine::open_read_only(&path),
                };
                let _ = sender.send(result.map(|_| ()));
            });
            // The regular-file guard fails closed without ever issuing the blocking
            // open, so the result is effectively immediate; the generous deadline only
            // distinguishes a real infinite block from scheduling latency under load.
            match receiver.recv_timeout(Duration::from_secs(30)) {
                Ok(Err(StoreError::Corruption { .. })) => {}
                Ok(other) => panic!("{label} on a FIFO should be corruption, got {other:?}"),
                Err(_) => panic!("{label} on a FIFO blocked instead of failing closed"),
            }
        }
    }
}
