//! Private native persistent ordered-byte engine, over [redb](https://docs.rs/redb).
//!
//! redb's `&[u8]` keys order byte-lexicographically, the same order as the
//! in-memory `BTreeMap`, so range scans need no custom comparator.
//!
//! A transaction holds one redb write transaction for its whole life, so reads
//! inside it see their own writes. Nested `begin` calls join that transaction:
//! only the outermost `commit` persists it, and any `rollback` aborts it.

use std::fs;
use std::ops::Bound;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use redb::{
    Database, DatabaseError, Durability, ReadOnlyDatabase, ReadTransaction, ReadableDatabase,
    ReadableTable, StorageError, Table, TableDefinition, WriteTransaction,
};

use crate::backend::{Backend, ScanPage, StoreError, ValuePrefix};
use crate::traversal;

const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("marrow");

const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");

/// The on-disk format version this build writes and accepts. A file recording a
/// different version is refused rather than misread; there is no auto-migration.
const FORMAT_VERSION: u32 = 1;

const MARROW_REDB_DURABILITY: Durability = Durability::Immediate;

const DELETE_BATCH_LIMIT: usize = 256;
#[cfg(unix)]
const STORE_SYMLINK_HOP_LIMIT: usize = 40;
static STORE_PANIC_HOOK: Mutex<()> = Mutex::new(());

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
    /// `Some` for the whole live handle; [`Drop`] takes it to close the database
    /// under the panic backstop. redb flushes its allocator and region bitmaps as the
    /// handle drops, and a store damaged in that metadata drives redb's close into a
    /// slice-index panic; without the catch it would abort the process after a command
    /// had already returned. The open transaction and pinned snapshot borrow the
    /// database, so close drops them first.
    db: Option<DatabaseHandle>,
    /// The live write transaction while one is open.
    txn: Option<OpenTransaction>,
    /// A pinned read transaction while a snapshot is held, so reads observe one
    /// consistent version even as later write transactions commit.
    read_view: Option<ReadTransaction>,
}

impl Drop for RedbStore {
    fn drop(&mut self) {
        // Close the database under the panic backstop: redb persists its allocator
        // state as the handle drops, and a store corrupted in that metadata panics
        // there. The catch keeps a damaged store fail-closed rather than aborting the
        // process on teardown.
        let _ = catch_traversal(|| {
            self.txn = None;
            self.read_view = None;
            drop(self.db.take());
            Ok(())
        });
    }
}

struct OpenTransaction {
    write: WriteTransaction,
    depth: usize,
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

/// A redb storage backend that serves reads from an existing store file but absorbs
/// every write and length change in an in-memory overlay, so the engine's
/// write-capable open path runs against the real committed bytes without ever
/// touching the file or taking its write lock.
///
/// The engine reads pages lazily, so the overlay materializes only the header,
/// allocator-state, and system pages the open consults, never the whole store. The
/// overlay is keyed by byte offset; an unwritten position reads through to the file,
/// or zero past the file's original length, matching the zero-fill a real backend
/// gives a freshly extended region.
#[derive(Debug)]
struct RecoveryProbeBackend {
    file: Mutex<fs::File>,
    file_len: u64,
    overlay: Mutex<std::collections::BTreeMap<u64, Vec<u8>>>,
    len: Mutex<u64>,
}

impl RecoveryProbeBackend {
    fn open(path: &Path) -> Result<Self, StoreError> {
        let file = fs::File::open(path).map_err(io("open"))?;
        let file_len = file.metadata().map_err(io("open"))?.len();
        Ok(Self {
            file: Mutex::new(file),
            file_len,
            overlay: Mutex::new(std::collections::BTreeMap::new()),
            len: Mutex::new(file_len),
        })
    }
}

impl redb::StorageBackend for RecoveryProbeBackend {
    fn len(&self) -> Result<u64, std::io::Error> {
        Ok(*self
            .len
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), std::io::Error> {
        use std::io::{Read, Seek, SeekFrom};
        let file_span = out.len().min(self.file_len.saturating_sub(offset) as usize);
        if file_span > 0 {
            let mut file = self
                .file
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut out[..file_span])?;
        }
        out[file_span..].fill(0);
        let read_end = offset + out.len() as u64;
        let overlay = self
            .overlay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (&start, bytes) in overlay.range(..read_end) {
            let end = start + bytes.len() as u64;
            let copy_start = start.max(offset);
            let copy_end = end.min(read_end);
            if copy_start >= copy_end {
                continue;
            }
            let out_at = (copy_start - offset) as usize;
            let src_at = (copy_start - start) as usize;
            let span = (copy_end - copy_start) as usize;
            out[out_at..out_at + span].copy_from_slice(&bytes[src_at..src_at + span]);
        }
        Ok(())
    }

    fn set_len(&self, len: u64) -> Result<(), std::io::Error> {
        *self
            .len
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = len;
        Ok(())
    }

    fn sync_data(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> Result<(), std::io::Error> {
        self.overlay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(offset, data.to_vec());
        Ok(())
    }
}

/// Run redb's write-capable open detection against the store's committed bytes
/// without modifying the file, so a read-only inspection agrees with `recover` and
/// `run` on whether the store is recoverable.
///
/// A read-only open loads the allocator state recorded by the last clean commit and
/// reads through the committed roots, so god-block or commit-tracker damage that the
/// write-capable open rejects — a torn transaction slot, an aborted-repair state —
/// opens cleanly and a content walk reads straight past it. The write-capable open
/// consults that header region; this replays the same open over an overlay backend
/// that reads the real bytes and discards every write, surfacing the engine's
/// `RepairAborted` or corruption verdict while leaving the file and its lock
/// untouched.
///
/// The caller runs this inside [`catch_open`], so a redb open panic on a clobbered
/// header becomes typed corruption; the probe must not nest its own `catch_open`,
/// whose panic-hook mutex is not reentrant.
fn verify_committed_recoverable(path: &Path) -> Result<(), StoreError> {
    let backend = RecoveryProbeBackend::open(path)?;
    Database::builder()
        .create_with_backend(backend)
        .map(|_| ())
        .map_err(|error| map_open_error(path, error))
}

/// Run a read-only or existing-store open, absorbing the brief window in which a
/// concurrent first-run writer has created the store file but not yet written its
/// header under the lock. A store this build committed is never zero-length, so a
/// torn-body corruption reported while the file is still empty is a creation race,
/// not a settled fault: the open retries until the writer's header and lock appear.
/// A file that stays empty across the budget is genuinely truncated and its
/// corruption surfaces, so a settled writer-free empty file is still rejected.
fn open_tolerating_creation_race(
    path: &Path,
    open: impl Fn() -> Result<DatabaseHandle, StoreError>,
) -> Result<DatabaseHandle, StoreError> {
    const CREATION_RACE_BACKOFF: [u64; 4] = [1, 2, 4, 8];
    let mut attempt = 0;
    loop {
        match open() {
            Err(StoreError::Corruption { .. })
                if attempt < CREATION_RACE_BACKOFF.len() && store_file_is_empty(path) =>
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

/// Whether the store file is currently zero-length: a header-less body that a
/// concurrent creator is still filling, or a truncated file. A missing file is not
/// empty; its open reports its own not-found error.
fn store_file_is_empty(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.len() == 0)
        .unwrap_or(false)
}

/// Run a store open and its structural probe under the panic backstop.
///
/// redb does not return an error for every damaged file: a truncated or clobbered
/// body drives its open-and-repair path into a layout assertion or an
/// `unreachable!()`, which panics. The backstop catches that escape and converts it
/// into [`StoreError::Corruption`], with a message describing the open failure.
fn catch_open<T>(open: impl FnOnce() -> Result<T, StoreError>) -> Result<T, StoreError> {
    catch_store_panic(open, "the storage engine could not open the store file")
}

/// Run a store traversal under the panic backstop.
///
/// redb walks btree pages lazily as a command reads, gets, scans, seeks, inserts,
/// or removes. A clobbered interior or leaf page that opened cleanly drives that
/// walk into a slice-index assertion or an `unreachable!()`, panicking deep inside
/// redb. Both reads and writes run here so the panic becomes typed corruption:
/// every access shape — point get, forward and reverse scan, seek, snapshot pin,
/// and the insert/remove descent a write performs — fails closed with a stable code
/// instead of aborting the process on first traversal.
fn catch_traversal<T>(body: impl FnOnce() -> Result<T, StoreError>) -> Result<T, StoreError> {
    catch_store_panic(body, "the store contains a damaged page and cannot be read")
}

/// Run `body` under a panic backstop that converts an escaping panic into
/// [`StoreError::Corruption`] carrying `corruption_message`.
///
/// Marrow builds unwind on panic, so the catch leaves the process alive and the
/// fault fail-closed with a typed code instead of a redb backtrace on stderr. The
/// catch wraps only the store operation so it cannot mask an unrelated bug. A no-op
/// panic hook is installed for the operation's duration so an expected redb panic
/// does not print its message and backtrace, then the previous hook is restored. The
/// hook is process-global, so concurrent in-process store operations serialize the
/// swap, and panics from other threads still delegate to the hook that was installed
/// before the operation.
fn catch_store_panic<T>(
    body: impl FnOnce() -> Result<T, StoreError>,
    corruption_message: &'static str,
) -> Result<T, StoreError> {
    let hook_guard = STORE_PANIC_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    catch_store_panic_locked(body, corruption_message, &hook_guard)
}

fn catch_store_panic_locked<T>(
    body: impl FnOnce() -> Result<T, StoreError>,
    corruption_message: &'static str,
    _hook_guard: &MutexGuard<'_, ()>,
) -> Result<T, StoreError> {
    let body_thread = std::thread::current().id();
    let previous_hook = Arc::new(std::panic::take_hook());
    let delegate_hook = Arc::clone(&previous_hook);
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() != body_thread {
            delegate_hook(info);
        }
    }));
    let caught = std::panic::catch_unwind(AssertUnwindSafe(body));
    drop(std::panic::take_hook());
    let previous_hook =
        Arc::try_unwrap(previous_hook).unwrap_or_else(|hook| Box::new(move |info| hook(info)));
    std::panic::set_hook(previous_hook);
    match caught {
        Ok(result) => result,
        Err(_) => Err(StoreError::Corruption {
            message: corruption_message.into(),
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

/// The fixed prefix of redb's super-header this build reads to bound an open. It
/// spans the layout fields and both 128-byte commit slots.
const SUPERBLOCK_PREFIX_LEN: usize = 320;

/// Reject a store whose super-header points a committed btree root at a page lying
/// beyond the file, before redb maps that page.
///
/// redb reads a page by computing its byte range from the page number and reading
/// that span, so a root page number clobbered toward a huge region, index, or order
/// makes the open allocate a span far past the file — gigabytes of zeroes, an OOM or
/// hang — before any data is traversed. The engine's own length check guards the
/// region-layout fields but not the committed root, so this validates each non-null
/// root in both commit slots against the actual file length and reports a body that
/// over-reaches as corruption. A header shorter than the prefix is left to redb,
/// which rejects a truncated body itself.
fn guard_superblock_header(path: &Path) -> Result<(), StoreError> {
    let header = match read_file_prefix(path, SUPERBLOCK_PREFIX_LEN)? {
        Some(header) => header,
        None => return Ok(()),
    };
    let file_len = fs::metadata(path)
        .map_err(io("open"))
        .map(|meta| meta.len())?;

    let page_size = u64::from(read_u32(&header, 12));
    let region_header_pages = u64::from(read_u32(&header, 16));
    let region_max_data_pages = u64::from(read_u32(&header, 20));
    if page_size == 0 {
        return Ok(());
    }
    let region_size = region_header_pages
        .checked_add(region_max_data_pages)
        .and_then(|pages| pages.checked_mul(page_size));
    let region_pages_start = region_header_pages.checked_mul(page_size);
    let (Some(region_size), Some(region_pages_start)) = (region_size, region_pages_start) else {
        return Ok(());
    };

    // Each commit slot holds a user root at slot+8 and a system root at slot+40; a
    // root is live only when its non-null flag (slot+1, slot+2) is set.
    for slot in [64usize, 192] {
        for (flag_offset, root_offset) in [(slot + 1, slot + 8), (slot + 2, slot + 40)] {
            if header[flag_offset] == 0 {
                continue;
            }
            let root = u64::from_le_bytes(header[root_offset..root_offset + 8].try_into().unwrap());
            if root_page_overreaches(root, page_size, region_size, region_pages_start, file_len) {
                return Err(StoreError::Corruption {
                    message: "store header points a committed root page beyond the file".into(),
                });
            }
        }
    }
    Ok(())
}

/// Whether the byte range of a redb root page number falls outside the file. The
/// page number packs an order in its top bits and a region/index below; its span is
/// `2^order` pages. Any arithmetic that overflows a `u64` is itself out of range.
fn root_page_overreaches(
    root: u64,
    page_size: u64,
    region_size: u64,
    region_pages_start: u64,
    file_len: u64,
) -> bool {
    let order = (root >> 59) as u32;
    let region = (root >> 20) & 0x000F_FFFF;
    let page_index = root & (0x000F_FFFF >> order);
    let Some(page_bytes) = 1u64
        .checked_shl(order)
        .and_then(|p| p.checked_mul(page_size))
    else {
        return true;
    };
    let end = region
        .checked_mul(region_size)
        .and_then(|base| base.checked_add(region_pages_start))
        .and_then(|start| start.checked_add(page_index.checked_mul(page_bytes)?))
        .and_then(|start| start.checked_add(page_size)) // data section begins after the super-header page
        .and_then(|start| start.checked_add(page_bytes));
    match end {
        Some(end) => end > file_len,
        None => true,
    }
}

/// Read up to `len` bytes from the start of the file, or `None` when there is no
/// committed header to validate: a missing file (creation handles it) or a body
/// shorter than `len` (a truncated body redb rejects on its own).
fn read_file_prefix(path: &Path, len: usize) -> Result<Option<Vec<u8>>, StoreError> {
    use std::io::Read;

    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        // A denied store file is its own path-bearing state: the fix is to grant
        // access, not retry, so it never collapses into the transient I/O bucket.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(StoreError::PermissionDenied {
                path: path.to_path_buf(),
            });
        }
        Err(_) => return Err(transient_open_io()),
    };
    let mut buffer = vec![0u8; len];
    match file.read_exact(&mut buffer) {
        Ok(()) => Ok(Some(buffer)),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(StoreError::PermissionDenied {
                path: path.to_path_buf(),
            })
        }
        Err(_) => Err(transient_open_io()),
    }
}

fn read_u32(header: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap())
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
/// roots is not probed here: redb walks those pages lazily, so it surfaces when a
/// read traverses the tree, under [`catch_traversal`].
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

/// Collect up to one batch of keys under `prefix`, starting strictly after `after`
/// when one is given. Resuming from an excluded cursor — rather than re-opening the
/// range at `prefix` each batch — bounds the iterator the same way the read scans do,
/// so a damaged page that would spin redb's range walk on a re-entered region is
/// stepped over by an exclusive seek rather than looped on.
fn delete_key_batch<T>(
    table: &T,
    prefix: &[u8],
    after: Option<&[u8]>,
) -> Result<Vec<Vec<u8>>, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let lower = match after {
        Some(after) => Bound::Excluded(after),
        None => Bound::Included(prefix),
    };
    let mut keys = Vec::new();
    for entry in table
        .range::<&[u8]>((lower, Bound::Unbounded))
        .map_err(io("delete"))?
    {
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

fn streamed_scan_between<T>(
    table: &T,
    prefix: &[u8],
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    limit: usize,
) -> Result<ScanPage, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let lower = lower.map_or(Bound::Unbounded, Bound::Included);
    let upper = upper.map_or(Bound::Unbounded, Bound::Excluded);
    let range = table
        .range::<&[u8]>((lower, upper))
        .map_err(io("scan_between"))?;
    traversal::scan(range, prefix, limit, io("scan_between"))
}

fn streamed_scan_between_after<T>(
    table: &T,
    prefix: &[u8],
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    cursor: &[u8],
    limit: usize,
) -> Result<ScanPage, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let lower = match lower {
        Some(lower) if lower > cursor => lower,
        _ => cursor,
    };
    let upper = upper.map_or(Bound::Unbounded, Bound::Excluded);
    let range = table
        .range::<&[u8]>((Bound::Excluded(lower), upper))
        .map_err(io("scan_between_after"))?;
    traversal::scan(range, prefix, limit, io("scan_between_after"))
}

fn streamed_scan_before<T>(
    table: &T,
    prefix: &[u8],
    cursor: &[u8],
    limit: usize,
) -> Result<ScanPage, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let range = table
        .range::<&[u8]>((Bound::Unbounded, Bound::Excluded(cursor)))
        .map_err(io("scan_before"))?;
    traversal::scan(range.rev(), prefix, limit, io("scan_before"))
}

fn streamed_scan_between_before<T>(
    table: &T,
    prefix: &[u8],
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    cursor: &[u8],
    limit: usize,
) -> Result<ScanPage, StoreError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let lower = lower.map_or(Bound::Unbounded, Bound::Included);
    let upper = match upper {
        Some(upper) if upper < cursor => upper,
        _ => cursor,
    };
    let range = table
        .range::<&[u8]>((lower, Bound::Excluded(upper)))
        .map_err(io("scan_between_before"))?;
    traversal::scan(range.rev(), prefix, limit, io("scan_between_before"))
}

/// Run a read `$body` over the current read view: the open write transaction's
/// table, the pinned snapshot, or a fresh read transaction. A macro rather than a
/// `&dyn` helper because redb's `ReadableTable` is not object-safe, so the body is
/// monomorphized per table type.
macro_rules! read_view {
    ($self:expr, $op:expr, |$table:ident| $body:expr) => {
        match (&$self.txn, &$self.read_view) {
            // An open write transaction reads its own writes.
            (Some(txn), _) => {
                let $table = txn.write.open_table(TABLE).map_err(io($op))?;
                $body
            }
            // A pinned snapshot reads its consistent version.
            (None, Some(read)) => {
                let $table = read.open_table(TABLE).map_err(io($op))?;
                $body
            }
            // Otherwise read the latest committed data.
            (None, None) => {
                let read = $self.handle().begin_read($op)?;
                let $table = read.open_table(TABLE).map_err(io($op))?;
                $body
            }
        }
    };
}

impl RedbStore {
    /// Open the redb-backed store at `path`, creating the file if needed. A
    /// concurrent read-only or read-write holder is rejected as [`StoreError::Locked`],
    /// and a file recording
    /// a different [`FORMAT_VERSION`] as [`StoreError::FormatVersion`]. A damaged
    /// body fails closed as [`StoreError::Corruption`] rather than panicking the
    /// process; the open and its structural probe run under [`catch_open`].
    pub(crate) fn open(path: &Path) -> Result<Self, StoreError> {
        let db = catch_open(|| {
            guard_regular_store_file(path)?;
            guard_superblock_header(path)?;
            let sync_parent_after_commit = prepare_new_store_file(path)?;
            let db = open_write_capable(path, || Database::create(path))?;
            stamp_or_verify_format_version(sync_parent_after_commit.as_deref(), &db)?;
            Ok(db)
        })?;
        Ok(Self::from_handle(DatabaseHandle::ReadWrite(db)))
    }

    fn from_handle(db: DatabaseHandle) -> Self {
        Self {
            db: Some(db),
            txn: None,
            read_view: None,
        }
    }

    /// The open database handle. Always present while the store is live; only
    /// [`Drop`] takes it to close the database under the panic backstop.
    fn handle(&self) -> &DatabaseHandle {
        self.db.as_ref().expect("store handle is live until drop")
    }

    /// Open an existing redb-backed store with write capability, without creating
    /// or adopting missing/non-store data. This is the repair path for a file that
    /// already carries Marrow metadata.
    pub(crate) fn open_existing(path: &Path) -> Result<Self, StoreError> {
        let db = open_tolerating_creation_race(path, || {
            catch_open(|| {
                guard_regular_store_file(path)?;
                guard_superblock_header(path)?;
                let db = open_write_capable(path, || Database::open(path))?;
                verify_existing_store_shape(&db)?;
                Ok(DatabaseHandle::ReadWrite(db))
            })
        })?;
        Ok(Self::from_handle(db))
    }

    /// Open an existing store read-only. Unlike [`open`](Self::open) it never
    /// creates the file and only verifies the recorded [`FORMAT_VERSION`] rather
    /// than stamping it; write-capability operations fail before any write
    /// transaction begins.
    ///
    /// A read-only open is more permissive than the write-capable open `recover` and
    /// `run` use: it reads through the last clean commit's roots, so god-block or
    /// commit-tracker damage that the write open rejects opens cleanly here and a
    /// content walk reads past it. So before the handle is returned,
    /// [`verify_committed_recoverable`] replays the write-capable open detection over
    /// the committed bytes without touching the file, surfacing the same
    /// [`StoreError::RecoveryRequired`] or corruption the write path reports. The open
    /// and its probes run under [`catch_open`].
    pub(crate) fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let db = open_tolerating_creation_race(path, || {
            catch_open(|| {
                guard_regular_store_file(path)?;
                guard_superblock_header(path)?;
                let db =
                    ReadOnlyDatabase::open(path).map_err(|error| map_open_error(path, error))?;
                verify_existing_store_shape(&db)?;
                verify_committed_recoverable(path)?;
                Ok(DatabaseHandle::ReadOnly(db))
            })
        })?;
        Ok(Self::from_handle(db))
    }

    /// Require a writable handle and no pinned read snapshot. `on_snapshot` names
    /// the snapshot-conflict error for this operation.
    fn ensure_writable(
        &self,
        op: &'static str,
        on_snapshot: fn() -> StoreError,
    ) -> Result<(), StoreError> {
        self.handle().require_write_access(op)?;
        if self.read_view.is_some() {
            return Err(on_snapshot());
        }
        Ok(())
    }

    /// Run `mutate` against the current write table. Outside a transaction it is
    /// its own short, immediately durable redb transaction; inside one it joins
    /// the open write transaction.
    fn mutate(
        &mut self,
        op: &'static str,
        mutate: impl FnOnce(&mut Table<&[u8], &[u8]>) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        catch_traversal(|| {
            let Some(txn) = &self.txn else {
                let write = self.handle().begin_write(op)?;
                {
                    let mut table = write.open_table(TABLE).map_err(io(op))?;
                    mutate(&mut table)?;
                }
                return write.commit().map_err(io(op));
            };
            let mut table = txn.write.open_table(TABLE).map_err(io(op))?;
            mutate(&mut table)?;
            Ok(())
        })
    }
}

impl Backend for RedbStore {
    fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        catch_traversal(|| {
            read_view!(self, "read", |table| Ok(table
                .get(key)
                .map_err(io("read"))?
                .map(|guard| guard.value().to_vec())))
        })
    }

    fn read_prefix(&self, key: &[u8], limit: usize) -> Result<Option<ValuePrefix>, StoreError> {
        catch_traversal(|| {
            read_view!(self, "read_prefix", |table| Ok(table
                .get(key)
                .map_err(io("read_prefix"))?
                .map(|guard| {
                    let value = guard.value();
                    let copied = value.len().min(limit);
                    ValuePrefix {
                        bytes: value[..copied].to_vec(),
                        truncated: value.len() > limit,
                    }
                })))
        })
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        self.handle().require_write_access(op)
    }

    fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.ensure_writable("write", StoreError::write_while_snapshot_pinned)?;
        self.mutate("write", |table| {
            table.insert(key, value.as_slice()).map_err(io("write"))?;
            Ok(())
        })
    }

    fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError> {
        self.ensure_writable("delete", StoreError::delete_while_snapshot_pinned)?;
        self.mutate("delete", |table| {
            // Collect and remove keys in bounded batches so a large prefix subtree
            // never materializes every key at once, resuming each batch strictly
            // after the prior one so the scan advances monotonically across the
            // subtree rather than re-reading from its start.
            let mut after: Option<Vec<u8>> = None;
            loop {
                let keys = delete_key_batch(&*table, prefix, after.as_deref())?;
                let Some(last) = keys.last().cloned() else {
                    break;
                };
                for key in keys {
                    table.remove(key.as_slice()).map_err(io("delete"))?;
                }
                after = Some(last);
            }
            Ok(())
        })
    }

    fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        catch_traversal(|| read_view!(self, "scan", |table| streamed_scan(&table, prefix, limit)))
    }

    fn scan_after(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        catch_traversal(|| {
            read_view!(self, "scan_after", |table| {
                streamed_scan_after(&table, prefix, cursor, limit)
            })
        })
    }

    fn scan_before(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        catch_traversal(|| {
            read_view!(self, "scan_before", |table| {
                streamed_scan_before(&table, prefix, cursor, limit)
            })
        })
    }

    fn scan_between(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        catch_traversal(|| {
            read_view!(self, "scan_between", |table| {
                streamed_scan_between(&table, prefix, lower, upper, limit)
            })
        })
    }

    fn scan_between_after(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        catch_traversal(|| {
            read_view!(self, "scan_between_after", |table| {
                streamed_scan_between_after(&table, prefix, lower, upper, cursor, limit)
            })
        })
    }

    fn scan_between_before(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        catch_traversal(|| {
            read_view!(self, "scan_between_before", |table| {
                streamed_scan_between_before(&table, prefix, lower, upper, cursor, limit)
            })
        })
    }

    fn begin(&mut self) -> Result<(), StoreError> {
        self.ensure_writable("begin", StoreError::begin_while_snapshot_pinned)?;
        match &mut self.txn {
            Some(txn) => txn.depth += 1,
            None => {
                self.txn = Some(OpenTransaction {
                    write: catch_traversal(|| self.handle().begin_write("begin"))?,
                    depth: 1,
                });
            }
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        // No open transaction is a harmless no-op, matching the in-memory store.
        let Some(mut txn) = self.txn.take() else {
            return Ok(());
        };
        txn.depth -= 1;
        if txn.depth == 0 {
            catch_traversal(|| txn.write.commit().map_err(io("commit")))?;
        } else {
            self.txn = Some(txn);
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), StoreError> {
        // No open transaction is a harmless no-op, matching the in-memory store.
        let Some(txn) = self.txn.take() else {
            return Ok(());
        };
        catch_traversal(|| txn.write.abort().map_err(io("rollback")))?;
        Ok(())
    }

    fn transaction_depth(&self) -> usize {
        self.txn.as_ref().map_or(0, |txn| txn.depth)
    }

    fn begin_snapshot(&mut self) -> Result<(), StoreError> {
        if self.txn.is_some() {
            return Err(StoreError::snapshot_while_transaction_open());
        }
        if self.read_view.is_some() {
            return Err(StoreError::snapshot_already_pinned());
        }
        // A redb read transaction is a stable version unaffected by later writes.
        self.read_view = Some(self.handle().begin_read("snapshot")?);
        Ok(())
    }

    fn end_snapshot(&mut self) {
        self.read_view = None;
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use redb::{Database, ReadableDatabase, TableDefinition};

    use super::{
        DELETE_BATCH_LIMIT, FORMAT_VERSION, META, RedbStore, STORE_PANIC_HOOK, TABLE, catch_open,
        catch_store_panic_locked, map_open_error,
    };
    use crate::backend::{Backend, StoreError};
    use crate::conformance;

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

    #[test]
    fn catch_open_preserves_the_panic_hook_for_other_threads() {
        let hook_guard = STORE_PANIC_HOOK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let seen = Arc::new(AtomicUsize::new(0));
        let previous_hook = std::panic::take_hook();
        let seen_hook = Arc::clone(&seen);
        std::panic::set_hook(Box::new(move |_| {
            seen_hook.fetch_add(1, Ordering::SeqCst);
        }));

        let result: Result<(), StoreError> = catch_store_panic_locked(
            || {
                std::thread::spawn(|| {
                    let _ = std::panic::catch_unwind(|| {
                        panic!("unrelated test panic");
                    });
                })
                .join()
                .expect("panic was caught in the thread");
                Ok(())
            },
            "the storage engine could not open the store file",
            &hook_guard,
        );

        let installed_hook = std::panic::take_hook();
        drop(installed_hook);
        std::panic::set_hook(previous_hook);
        drop(hook_guard);

        result.expect("catch open succeeds");
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "unrelated thread panics must keep the existing hook"
        );
    }

    /// A non-panicking closure passes its result through unchanged, so the backstop
    /// adds no behavior beyond catching a panic.
    #[test]
    fn catch_open_passes_a_clean_result_through() {
        assert_eq!(catch_open(|| Ok(7)).expect("clean open"), 7);
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
    /// I/O blip. The header probe runs before the engine open, so the denied prefix
    /// read must carry the typed `store.permission_denied` code and name the path on
    /// every open path, never collapse into the `store.io` catch-all with a raw errno.
    #[cfg(unix)]
    #[test]
    fn opening_a_denied_store_file_is_permission_denied_on_every_open_path() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("marrow-store-redb-denied-file").expect("temp dir");
        let path = dir.path().join("marrow.redb");
        {
            let mut store = RedbStore::open(&path).expect("create fresh store");
            Backend::write(&mut store, b"k", b"v".to_vec()).expect("write");
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
            .expect("deny access to the store file");

        for result in [
            RedbStore::open(&path).map(|_| ()),
            RedbStore::open_existing(&path).map(|_| ()),
            RedbStore::open_read_only(&path).map(|_| ()),
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
        expect_io(RedbStore::open(&loop_a).map(|_| ()), "loop open");
        expect_io(
            RedbStore::open_existing(&loop_a).map(|_| ()),
            "loop existing",
        );
        expect_io(
            RedbStore::open_read_only(&loop_a).map(|_| ()),
            "loop read-only",
        );

        // A dangling target is a missing store to the non-creating opens; `open`
        // creates the target, so only inspection and repair surface the fault.
        expect_io(
            RedbStore::open_existing(&dangling).map(|_| ()),
            "dangling existing",
        );
        expect_io(
            RedbStore::open_read_only(&dangling).map(|_| ()),
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
            RedbStore::open(&path)
        })
    }

    /// A byte flip below the table roots — invisible to a table-open probe — must
    /// surface as typed corruption when a read traverses the tree, never as a process
    /// panic. The read backstop catches redb's slice-index panic on a damaged page
    /// and maps it to corruption; an offset redb tolerates simply reads through.
    /// Either way no read panics, across a point get, a forward scan, and a reverse
    /// scan. The seed spans interior btree pages so the damage hides below the roots.
    #[test]
    fn reads_over_a_damaged_page_report_corruption_not_a_panic() {
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
        let seed = dir.path().join("seed.redb");
        {
            let mut store = RedbStore::open(&seed).expect("open fresh");
            for n in 0..4000u32 {
                let key = format!("k/{n:08}").into_bytes();
                Backend::write(&mut store, &key, vec![0u8; 64]).expect("write");
            }
        }
        let body = std::fs::read(&seed).expect("read store body");

        for offset in [8192usize, 12288, 16384, 20484, 24576] {
            let mut bytes = body.clone();
            bytes[offset] ^= 0xff;
            let path = dir.path().join(format!("corrupt-{offset}.redb"));
            std::fs::write(&path, &bytes).expect("write corrupted body");

            let store = match RedbStore::open(&path) {
                Ok(store) => store,
                // A flip that damages the file header is caught at open; still no panic.
                Err(StoreError::Corruption { .. }) => continue,
                Err(other) => panic!("offset {offset}: unexpected open error {other:?}"),
            };
            for outcome in [
                Backend::read(&store, b"k/00000001").map(|_| ()),
                Backend::scan(&store, b"k/", 8000).map(|_| ()),
                Backend::scan_before(&store, b"k/", b"k/99999999", 8000).map(|_| ()),
            ] {
                match outcome {
                    Ok(()) | Err(StoreError::Corruption { .. }) => {}
                    Err(other) => panic!("offset {offset}: a read must not fault as {other:?}"),
                }
            }
        }
    }

    #[test]
    fn delete_removes_more_than_one_bounded_batch() {
        let dir = TempDir::new("marrow-store-redb-test").expect("create a temp dir");
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
        let dir = TempDir::new("marrow-store-redb-test").expect("create a temp dir");
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
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
        let path = dir.path().join("snapshot.redb");
        let key: &[u8] = b"k";
        let old: &[u8] = b"old";
        let new: &[u8] = b"new";

        let mut store = RedbStore::open(&path).expect("open");
        Backend::write(&mut store, key, old.to_vec()).expect("seed old value");
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

        drop(RedbStore::open(&path).expect("open"));
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

        drop(RedbStore::open(&path).expect("open"));
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

        match RedbStore::open(&path) {
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
        let dir = TempDir::new("marrow-store-redb-test").expect("temp dir");
        let path = dir.path().join("fresh.redb");
        {
            let mut store = RedbStore::open(&path).expect("create fresh");
            store.write(b"k", b"v".to_vec()).expect("write");
        }
        let store = RedbStore::open(&path).expect("reopen stamped store");
        assert_eq!(store.read(b"k").expect("read"), Some(b"v".to_vec()));
    }

    /// A read-only inspection racing a writer's first-run store creation must never see
    /// a false corruption: it observes either no store yet, the writer's lock, or a
    /// healthy store. The header-less empty-file window between the file appearing and
    /// the writer committing its header under the lock once surfaced as corruption to a
    /// racing reader; the reader now tolerates that transient empty body and so only
    /// ever resolves to a healthy store, the lock, or a not-yet-created store.
    #[test]
    fn read_only_open_racing_first_run_creation_never_false_corrupts() {
        for _ in 0..16 {
            let dir = TempDir::new("marrow-store-redb-race").expect("temp dir");
            let path = dir.path().join("marrow.redb");

            let writer_path = path.clone();
            let writer = std::thread::spawn(move || {
                let mut store = RedbStore::open(&writer_path).expect("create fresh store");
                store.write(b"k", b"v".to_vec()).expect("write");
            });

            let reader_path = path.clone();
            let reader = std::thread::spawn(move || {
                loop {
                    match RedbStore::open_read_only(&reader_path) {
                        Ok(_) => return,
                        Err(StoreError::Locked { .. }) | Err(StoreError::Io { .. }) => {
                            std::thread::yield_now();
                            continue;
                        }
                        Err(StoreError::Corruption { message }) => {
                            panic!("reader saw false corruption mid-creation: {message}")
                        }
                        Err(other) => {
                            // A not-yet-created store surfaces as a not-found-style error;
                            // anything else is an unexpected classification.
                            if format!("{other:?}").contains("NotFound")
                                || format!("{other:?}").contains("not found")
                            {
                                std::thread::yield_now();
                                continue;
                            }
                            return;
                        }
                    }
                }
            });

            writer.join().expect("writer thread");
            reader.join().expect("reader thread");
        }
    }

    /// A settled, writer-free zero-length store file is genuine corruption and must
    /// still be reported as such: the creation-race tolerance retries only while the
    /// file stays empty, so an empty file that never fills surfaces corruption once the
    /// brief budget is spent.
    #[test]
    fn read_only_open_rejects_a_settled_empty_store_as_corruption() {
        let dir = TempDir::new("marrow-store-redb-empty").expect("temp dir");
        let path = dir.path().join("marrow.redb");
        std::fs::File::create(&path).expect("create empty file");

        match RedbStore::open_read_only(&path).map(|_| ()) {
            Err(StoreError::Corruption { .. }) => {}
            other => panic!("a settled empty store must be corruption, got {other:?}"),
        }
    }

    /// A read-only open that begins while the store file is a header-less empty body —
    /// the window a first-run writer briefly leaves — must resolve to the healthy store
    /// once the writer fills and unlocks it, never a false corruption.
    #[test]
    fn read_only_open_tolerates_a_mid_creation_empty_file() {
        use std::time::Duration;

        let dir = TempDir::new("marrow-store-redb-fill").expect("temp dir");
        let path = dir.path().join("marrow.redb");
        std::fs::File::create(&path).expect("create empty file");

        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(3));
            let mut store = RedbStore::open(&writer_path).expect("create store");
            store.write(b"k", b"v".to_vec()).expect("write");
        });

        // The reader starts against the empty placeholder; the tolerance window must
        // outlast the writer so the reader sees the finished store, not corruption.
        let result = RedbStore::open_read_only(&path).map(|_| ());
        writer.join().expect("writer thread");
        match result {
            Ok(_) | Err(StoreError::Locked { .. }) | Err(StoreError::Io { .. }) => {}
            Err(StoreError::Corruption { message }) => {
                panic!("mid-creation empty file misread as corruption: {message}")
            }
            other => panic!("unexpected mid-creation classification: {other:?}"),
        }
    }

    /// A commit slot's root page number, clobbered in its high bit, decodes to a page
    /// whose byte range lies far past the file. redb maps that page by allocating its
    /// span, so the corrupt root drives an unbounded allocation before any data is read.
    /// The open must reject such a header as corruption promptly, never allocate toward
    /// it. Each flip runs on a worker thread with a deadline so a regression surfaces as
    /// a timeout (the unbounded allocation) rather than a hung or OOM-killed suite.
    #[test]
    fn open_rejects_a_corrupt_root_page_number_without_unbounded_allocation() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = TempDir::new("marrow-store-redb-root").expect("temp dir");
        let seed = dir.path().join("seed.redb");
        {
            let mut store = RedbStore::open(&seed).expect("open fresh");
            Backend::write(&mut store, b"k", b"v".to_vec()).expect("write");
        }
        let body = std::fs::read(&seed).expect("read store body");

        // The high byte of the little-endian u64 root page number in commit slot 0:
        // the user root at file offset 72 and the system root at offset 104.
        for offset in [79usize, 111] {
            let mut bytes = body.clone();
            bytes[offset] = 0xff;
            let path = dir.path().join(format!("root-{offset}.redb"));
            std::fs::write(&path, &bytes).expect("write corrupted body");

            for label in ["open", "open_existing", "open_read_only"] {
                let path = path.clone();
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = match label {
                        "open" => RedbStore::open(&path),
                        "open_existing" => RedbStore::open_existing(&path),
                        _ => RedbStore::open_read_only(&path),
                    };
                    let _ = sender.send(result.map(|_| ()));
                });
                match receiver.recv_timeout(Duration::from_secs(20)) {
                    Ok(Err(StoreError::Corruption { .. })) => {}
                    Ok(other) => panic!(
                        "offset {offset} {label}: a corrupt root must be corruption, got {other:?}"
                    ),
                    Err(_) => panic!(
                        "offset {offset} {label}: open did not return (unbounded allocation on a corrupt root)"
                    ),
                }
            }
        }
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

        for label in ["open", "open_existing", "open_read_only"] {
            let path = path.clone();
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let result = match label {
                    "open" => RedbStore::open(&path),
                    "open_existing" => RedbStore::open_existing(&path),
                    _ => RedbStore::open_read_only(&path),
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
