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
//!   stable media. Default SQLite behaves identically.
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

use redb::ReadOnlyDatabase;
use redb::{
    Database, DatabaseError, Durability, ReadTransaction, ReadableDatabase, ReadableTable,
    StorageError, TableDefinition, WriteTransaction,
};

use crate::engine::{ByteEngine, Cell, CommitOutcome, ReadView, WriteTxn, check_cell_limits};
use crate::error::{StoreError, StoreOp};
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
pub(crate) struct NativeEngine {
    db: Option<DatabaseHandle>,
    drop_trust: DropTrust,
}

/// Whether the dependency handle is still trusted to unwind cleanly on drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropTrust {
    /// No audit has failed; dropping the handle runs ordinarily.
    Ordinary,
    /// A failed integrity audit can leave redb's live handle traversing corrupt
    /// allocator metadata again from `Database::drop`, so that second dependency
    /// unwind is contained at the adapter boundary. The persistent recovery
    /// caller separately quarantines its owner lock before auditing; this
    /// containment does not itself authorize or classify later use.
    ContainedAfterFailedAudit,
}

enum DatabaseHandle {
    ReadWrite(Database),
    ReadOnly(ReadOnlyDatabase),
}

impl DatabaseHandle {
    fn begin_read(&self, op: StoreOp) -> Result<ReadTransaction, StoreError> {
        match self {
            Self::ReadWrite(db) => db.begin_read().map_err(io(op)),
            Self::ReadOnly(db) => db.begin_read().map_err(io(op)),
        }
    }

    fn begin_write(&self, op: StoreOp) -> Result<WriteTransaction, StoreError> {
        match self {
            Self::ReadWrite(db) => {
                let mut write = db.begin_write().map_err(io(op))?;
                pin_write_durability(&mut write, op)?;
                Ok(write)
            }
            Self::ReadOnly(_) => Err(StoreError::ReadOnly { op }),
        }
    }

    fn require_write_access(&self, op: StoreOp) -> Result<(), StoreError> {
        match self {
            Self::ReadWrite(_) => Ok(()),
            Self::ReadOnly(_) => Err(StoreError::ReadOnly { op }),
        }
    }
}

impl NativeEngine {
    fn db(&self) -> &DatabaseHandle {
        self.db
            .as_ref()
            .expect("a live native engine retains its database handle")
    }

    fn db_mut(&mut self) -> &mut DatabaseHandle {
        self.db
            .as_mut()
            .expect("a live native engine retains its database handle")
    }
}

impl Drop for NativeEngine {
    fn drop(&mut self) {
        let Some(db) = self.db.take() else {
            return;
        };
        match self.drop_trust {
            DropTrust::Ordinary => drop(db),
            DropTrust::ContainedAfterFailedAudit => {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(db)));
            }
        }
    }
}

fn pin_write_durability(write: &mut WriteTransaction, op: StoreOp) -> Result<(), StoreError> {
    write.set_durability(MARROW_REDB_DURABILITY).map_err(io(op))
}

fn io<E: std::fmt::Display>(op: StoreOp) -> impl Fn(E) -> StoreError {
    move |error| StoreError::Io {
        op,
        message: error.to_string(),
    }
}

/// Contain a panic from the redb dependency at the adapter boundary. redb asserts
/// internally on some externally-mutated files rather than returning `Err`, so an
/// operation over a corrupt body can unwind instead of failing cleanly; this
/// converts that unwind into a typed corruption error. It is adapter-boundary
/// containment of one dependency's panic policy over a bounded call; swapping a
/// process-global panic hook to achieve the same thing stays forbidden.
fn contain_panic<T>(
    op: StoreOp,
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

/// Open a redb database, waiting briefly past a lock that a dropped handle may still hold.
///
/// Generic over the opened type because a read-only open meets the same wait: redb takes
/// the file lock for `ReadOnlyDatabase::open` too, so a read-only inspection issued right
/// after a failed write-capable open — which drops the writer it acquired — can observe
/// the lock and surface as [`StoreError::Locked`] instead of the corruption or
/// format-version verdict the caller was testing for.
///
/// redb guards a store with an OS file lock — `flock` on Unix targets that support it,
/// and redb proceeds unlocked where the platform reports locking unsupported — acquired
/// on open and released when the handle drops. A write-capable open issued just after
/// another handle on the same path was dropped can still observe that lock held and fail
/// with `DatabaseAlreadyOpen`, and the window widens with machine load. Why it is still
/// held is not established — the kernel's release trailing the close, or a concurrently
/// spawned child inheriting the descriptor until its exec — and the same wait absorbs
/// either.
///
/// A genuine conflicting holder (two read-only handles take compatible shared locks and
/// the second simply succeeds) keeps the lock for the whole budget and surfaces as
/// [`StoreError::Locked`], as does a transient window longer than the budget. Only the
/// retry sleeps are bounded — 1, 2, 4 and 8 ms; the filesystem and database open being
/// retried are not.
fn open_past_lock_release<T>(
    path: &Path,
    open: impl Fn() -> Result<T, DatabaseError>,
) -> Result<T, StoreError> {
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
        op: StoreOp::Open,
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
/// a concurrent creator still forming the store rather than settled damage. Neither
/// forming state is distinguishable from settled damage by a cheap probe once the open
/// has already failed, so a corruption against any regular store file is retried. A
/// non-regular path (a FIFO, socket, or directory) or a missing file is not a store
/// under construction.
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
    let directory = fs::File::open(parent).map_err(io(StoreOp::SyncParentDir))?;
    directory.sync_all().map_err(io(StoreOp::SyncParentDir))
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
        .map_err(io(StoreOp::SyncParentDir))?;
    directory.sync_all().map_err(io(StoreOp::SyncParentDir))
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
    pin_write_durability(&mut write, StoreOp::Open)?;
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
            op: StoreOp::Open,
            message: other.to_string(),
        },
    }
}

fn open_table_error(error: redb::TableError) -> StoreError {
    match error {
        redb::TableError::Storage(storage) => map_storage_error(storage),
        other => StoreError::Io {
            op: StoreOp::Open,
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
            op: StoreOp::Open,
            message: other.to_string(),
        },
    }
}

impl NativeEngine {
    /// The on-disk format version this build stamps into a new store and requires on open.
    /// The single owner of the value; a store's persisted-envelope engine tuple
    /// records it from here rather than mirroring the literal, so provenance cannot drift from
    /// what the engine actually wrote.
    pub(crate) const FORMAT_VERSION: u32 = FORMAT_VERSION;

    /// Create and stamp exactly one new native store file. Unlike redb's
    /// create-or-open primitive, this refuses an existing path before opening
    /// it, so provisioning can never adopt or restamp an existing body.
    pub(crate) fn create_new(path: &Path) -> Result<Self, StoreError> {
        contain_panic(StoreOp::Provision, || {
            guard_regular_store_file(path)?;
            let Some(created_path) = prepare_new_store_file(path)? else {
                return Err(StoreError::Io {
                    op: StoreOp::Provision,
                    message: "the native store file already exists".into(),
                });
            };
            let db = open_past_lock_release(path, || Database::create(path))?;
            stamp_or_verify_format_version(Some(&created_path), &db)?;
            Ok(Self {
                db: Some(DatabaseHandle::ReadWrite(db)),
                drop_trust: DropTrust::Ordinary,
            })
        })
    }

    /// Open an existing store with write capability. Unlike
    /// [`create_new`](Self::create_new), this operation never creates a file or
    /// stamps a database: the complete
    /// Marrow metadata and data tables must already be present. A missing,
    /// malformed, foreign, or unstamped file is refused without modification.
    pub(crate) fn open_existing(path: &Path) -> Result<Self, StoreError> {
        contain_panic(StoreOp::Open, || {
            let db = open_tolerating_creation_race(path, || {
                guard_regular_store_file(path)?;
                let db = open_past_lock_release(path, || Database::open(path))?;
                verify_existing_store_shape(&db)?;
                Ok(DatabaseHandle::ReadWrite(db))
            })?;
            Ok(Self {
                db: Some(db),
                drop_trust: DropTrust::Ordinary,
            })
        })
    }

    /// Prepare service from read-only-admitted bytes under cooperative exclusion.
    /// Unchanged bytes use the same saved allocator state. The callback aborts
    /// full repair, but header recovery may precede it; this is not a defense
    /// against external mutation. Neither corruption nor replacement is retried.
    pub(crate) fn open_for_service(path: &Path) -> Result<Self, StoreError> {
        contain_panic(StoreOp::Open, || {
            guard_regular_store_file(path)?;
            let mut builder = Database::builder();
            builder.set_repair_callback(|session| session.abort());
            let db = open_past_lock_release(path, || builder.open(path))?;
            verify_existing_store_shape(&db)?;
            Ok(Self {
                db: Some(DatabaseHandle::ReadWrite(db)),
                drop_trust: DropTrust::Ordinary,
            })
        })
    }

    /// Open an existing store read-only. Unlike
    /// [`create_new`](Self::create_new) it never creates the file and only
    /// verifies the recorded [`FORMAT_VERSION`]  rather than stamping it; write-capability operations fail before any write
    /// transaction begins. A malformed body surfaces redb's own open error as a
    /// typed [`StoreError`] through [`map_open_error`].
    pub(crate) fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        contain_panic(StoreOp::Open, || {
            let db = open_tolerating_creation_race(path, || {
                guard_regular_store_file(path)?;
                let db = open_past_lock_release(path, || ReadOnlyDatabase::open(path))?;
                verify_existing_store_shape(&db)?;
                Ok(DatabaseHandle::ReadOnly(db))
            })?;
            Ok(Self {
                db: Some(db),
                drop_trust: DropTrust::Ordinary,
            })
        })
    }
}

impl ByteEngine for NativeEngine {
    type View<'a> = RedbView<'a>;
    type Txn<'a> = RedbTxn<'a>;

    fn read_view(&self) -> Result<RedbView<'_>, StoreError> {
        Ok(RedbView {
            read: self.db().begin_read(StoreOp::BeginRead)?,
            _engine: PhantomData,
        })
    }

    fn begin(&mut self) -> Result<RedbTxn<'_>, StoreError> {
        Ok(RedbTxn {
            write: Some(self.db().begin_write(StoreOp::BeginWrite)?),
            _engine: PhantomData,
        })
    }

    fn require_write_access(&self, op: StoreOp) -> Result<(), StoreError> {
        self.db().require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        let result = match self.db_mut() {
            DatabaseHandle::ReadWrite(db) => contain_panic(StoreOp::Audit, || {
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
            DatabaseHandle::ReadOnly(_) => Err(StoreError::ReadOnly { op: StoreOp::Audit }),
        };
        if result.is_err() {
            self.drop_trust = DropTrust::ContainedAfterFailedAudit;
        }
        result
    }
}

/// A coherent read view over a redb read transaction — a stable version whose
/// reads are unaffected by later commits. Bound to the engine borrow that
/// produced it, so no write can interleave for its life.
pub(crate) struct RedbView<'a> {
    read: ReadTransaction,
    _engine: PhantomData<&'a NativeEngine>,
}

impl ReadView for RedbView<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        contain_panic(StoreOp::Read, || {
            let table = self.read.open_table(TABLE).map_err(io(StoreOp::Read))?;
            Ok(table
                .get(key)
                .map_err(io(StoreOp::Read))?
                .map(|guard| guard.value().to_vec()))
        })
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        contain_panic(StoreOp::ScanAfter, || {
            let table = self
                .read
                .open_table(TABLE)
                .map_err(io(StoreOp::ScanAfter))?;
            scan_after_table(&table, prefix, cursor)
        })
    }
}

/// A redb write transaction. Reads observe its own staged writes; it commits
/// durably or, on drop, aborts. Borrows the engine mutably, so a second
/// transaction cannot be named while it is live.
pub(crate) struct RedbTxn<'a> {
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
        contain_panic(StoreOp::Read, || {
            let table = self.write().open_table(TABLE).map_err(io(StoreOp::Read))?;
            Ok(table
                .get(key)
                .map_err(io(StoreOp::Read))?
                .map(|guard| guard.value().to_vec()))
        })
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        contain_panic(StoreOp::ScanAfter, || {
            let table = self
                .write()
                .open_table(TABLE)
                .map_err(io(StoreOp::ScanAfter))?;
            scan_after_table(&table, prefix, cursor)
        })
    }
}

impl WriteTxn for RedbTxn<'_> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        check_cell_limits(key, &value)?;
        let mut table = self.write().open_table(TABLE).map_err(io(StoreOp::Put))?;
        table
            .insert(key, value.as_slice())
            .map_err(io(StoreOp::Put))?;
        Ok(())
    }

    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        let mut table = self
            .write()
            .open_table(TABLE)
            .map_err(io(StoreOp::Remove))?;
        table.remove(key).map_err(io(StoreOp::Remove))?;
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
        match contain_panic(StoreOp::Commit, || {
            write.commit().map_err(io(StoreOp::Commit))
        }) {
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
        .map_err(io(StoreOp::ScanAfter))?;
    traversal::collect_after(range, prefix, io(StoreOp::ScanAfter))
}

/// Create, as a raw redb handle, a database for a test to seed or inspect.
///
/// Routed through [`open_past_lock_release`] so that every open of a redb database in
/// this crate goes through it without exception. The wait costs nothing on a fresh path.
#[cfg(test)]
pub(crate) fn create_raw(path: &Path, subject: &str) -> Database {
    open_past_lock_release(path, || Database::create(path))
        .unwrap_or_else(|error| panic!("create {subject}: {error:?}"))
}

/// Reopen, as a raw redb handle, a file whose previous handle was just dropped.
///
/// A store's advisory lock can still be held for a short interval after the handle that
/// took it is dropped (see [`open_past_lock_release`]). A test that opens directly
/// reintroduces that hazard and reports it as a store defect, so every raw reopen in this
/// crate's tests comes through here.
///
/// A genuinely held lock exhausts the backoff and this helper panics with that
/// [`StoreError`] rendered into the message, so a test sees the reason. It never reports
/// a held lock as success.
#[cfg(test)]
pub(crate) fn reopen_raw(path: &Path, subject: &str) -> Database {
    open_past_lock_release(path, || Database::open(path))
        .unwrap_or_else(|error| panic!("reopen {subject}: {error:?}"))
}

#[cfg(test)]
#[path = "redb_tests.rs"]
mod tests;
