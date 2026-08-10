//! Opaque ownership of one native engine and its process owner lock.
//!
//! The store directory is the unit of ownership, and it is also the node exclusion
//! rests on: the advisory lock is taken on the canonicalized directory itself
//! before any name inside it is opened, and the `lock` marker's own lock stands
//! behind it. A lock resting only on names inside the directory does not survive
//! their replacement, and the two replaceable nodes a holder locks — the marker
//! and the engine file — can be replaced together. This module alone derives the
//! `lock` and `store.redb` paths, acquires the advisory lock before admission or
//! engine open, and keeps that lock inseparable from the native engine. An
//! indeterminate commit irreversibly quarantines every node it rests on until
//! process exit.
//!
//! What that establishes and what it does not: while a holder is live, no second
//! owner of the same store directory node can be constructed, whatever a writer
//! inside that directory does to its children. It is not exclusion over a
//! *path*. A writer that replaces the store directory node itself — moving it
//! aside and publishing another directory under the same name — leaves two owners
//! of two different directories that one path reaches in turn, which is the
//! custody split the storage reference records.
//!
//! Acquisition is separate from binding so nothing above this module has to read
//! a byte of the store directory to decide exclusion. [`NativeEngineOwner::acquire_existing`]
//! canonicalizes the directory, takes the lock, and returns an affine
//! [`PendingNativeEngineOwner`] having made no engine call and without being told
//! which store instance it is about to hold. The owner above it reads whatever it
//! needs under that exclusion and names the instance afterwards, which is the only
//! ordering in which a malformed artifact cannot preempt contention.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use marrow_codes::Code;

use crate::engine::{ByteEngine, Cell, CommitOutcome, ReadView, WriteTxn};
use crate::error::StoreError;
use crate::redb::{NativeEngine, RedbTxn, RedbView};

/// The native engine file inside a Marrow store directory.
pub const NATIVE_ENGINE_FILE: &str = "store.redb";
/// The permanent owner-lock file inside a Marrow store directory.
pub const NATIVE_LOCK_FILE: &str = "lock";
/// The native engine format written and accepted by this build.
pub const NATIVE_ENGINE_FORMAT_VERSION: u32 = NativeEngine::FORMAT_VERSION;

const LOCK_MAGIC: &[u8; 4] = b"MWSL";
/// The marker layout this build writes: a state tag distinguishes a lock held
/// before its store instance is known from one bound to it.
const LOCK_VERSION: u8 = 1;
/// The layout this build still reads: a bound owner with no state tag, whose
/// fields sit in the order that layout froze.
const LEGACY_BOUND_VERSION: u8 = 0;
const LEGACY_BOUND_BYTES: usize = 4 + 1 + 4 + 16 + 8;
const PENDING_TAG: u8 = 0x01;
const BOUND_TAG: u8 = 0x02;
const PENDING_BYTES: usize = 4 + 1 + 1 + 4 + 8;
const BOUND_BYTES: usize = PENDING_BYTES + 16;

/// The best-effort identity recorded for a live native-store owner.
///
/// The instance is absent while a holder has taken the lock but has not yet named
/// the store it is opening. A contender is entitled to the exclusion verdict and
/// to whatever identity the marker carries, never to a stronger claim than the
/// holder has actually published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeLockOwner {
    /// The owning process id.
    pub pid: u32,
    /// The lifecycle store instance bytes, once the holder has bound them.
    pub instance: Option<[u8; 16]>,
    /// The acquisition time in Unix-epoch seconds. This is forensic only.
    pub acquired_unix_secs: u64,
}

impl NativeLockOwner {
    fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(BOUND_BYTES);
        bytes.extend_from_slice(LOCK_MAGIC);
        bytes.push(LOCK_VERSION);
        bytes.push(match self.instance {
            Some(_) => BOUND_TAG,
            None => PENDING_TAG,
        });
        bytes.extend_from_slice(&self.pid.to_be_bytes());
        bytes.extend_from_slice(&self.acquired_unix_secs.to_be_bytes());
        if let Some(instance) = self.instance {
            bytes.extend_from_slice(&instance);
        }
        bytes
    }

    /// The owner a marker names, or `None` for any byte string that is not a whole layout
    /// this build reads. Every access is bounds-checked: the bytes are a contender's input,
    /// so a length this decoder does not expect must reach a verdict rather than an abort.
    fn decode(bytes: &[u8]) -> Option<Self> {
        let field = |from: usize, to: usize| bytes.get(from..to);
        let byte = |at: usize| bytes.get(at).copied();
        if field(0, 4)? != LOCK_MAGIC {
            return None;
        }
        match byte(4)? {
            LEGACY_BOUND_VERSION if bytes.len() == LEGACY_BOUND_BYTES => Some(Self {
                pid: u32::from_be_bytes(field(5, 9)?.try_into().ok()?),
                instance: Some(field(9, 25)?.try_into().ok()?),
                acquired_unix_secs: u64::from_be_bytes(field(25, 33)?.try_into().ok()?),
            }),
            LOCK_VERSION => {
                let instance = match (byte(5)?, bytes.len()) {
                    (PENDING_TAG, PENDING_BYTES) => None,
                    (BOUND_TAG, BOUND_BYTES) => Some(field(18, 34)?.try_into().ok()?),
                    _ => return None,
                };
                Some(Self {
                    pid: u32::from_be_bytes(field(6, 10)?.try_into().ok()?),
                    instance,
                    acquired_unix_secs: u64::from_be_bytes(field(10, 18)?.try_into().ok()?),
                })
            }
            _ => None,
        }
    }
}

/// Why the native owner lock could not be acquired.
#[derive(Debug)]
pub enum NativeLockError {
    /// Another live owner holds the store.
    StoreInUse { owner: Option<NativeLockOwner> },
    /// The lock file or directory could not be read or synchronized.
    Io(std::io::Error),
}

impl NativeLockError {
    /// The stable diagnostic code for this lock failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::StoreInUse { .. } => Code::StoreLocked.as_str(),
            Self::Io(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for NativeLockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreInUse { owner: Some(owner) } => write!(
                formatter,
                "the store is already open by process {}; close it, then retry",
                owner.pid,
            ),
            Self::StoreInUse { owner: None } => write!(
                formatter,
                "the store is already open by another process; close it, then retry",
            ),
            Self::Io(error) => write!(formatter, "the store lock could not be taken: {error}"),
        }
    }
}

impl std::error::Error for NativeLockError {}

/// A failure while acquiring the owner lock over an existing store directory.
#[derive(Debug)]
pub enum NativeOwnerAcquireError {
    /// The store directory could not be pinned to a canonical path.
    Io(std::io::Error),
    /// The process owner lock could not be acquired.
    Lock(NativeLockError),
}

impl NativeOwnerAcquireError {
    /// The stable diagnostic code for this acquisition failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => Code::StoreIo.as_str(),
            Self::Lock(error) => error.code(),
        }
    }
}

impl std::fmt::Display for NativeOwnerAcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(
                formatter,
                "the store directory could not be pinned: {error}"
            ),
            Self::Lock(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for NativeOwnerAcquireError {}

/// A failure while binding an acquired owner and opening its existing engine.
#[derive(Debug)]
pub enum NativeOwnerOpenError<R> {
    /// The owner marker could not be bound to this store instance.
    Lock(NativeLockError),
    /// The zero-capability admission callback refused the open.
    Refused(R),
    /// The existing native engine could not be opened or audited.
    Store(StoreError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropDisposition {
    PreserveUnclean,
    Clean,
    Quarantine,
}

struct OwnerLock {
    /// The store directory node's own advisory lock, taken before any name inside the
    /// directory is opened. See [`OwnerLock::acquire`].
    directory_node: Option<File>,
    file: Option<File>,
    disposition: DropDisposition,
}

struct AcquiredLock {
    lock: OwnerLock,
    prior_unclean: bool,
    acquired_unix_secs: u64,
}

impl OwnerLock {
    /// Take the directory's owner lock without naming a store instance. A prior
    /// nonempty marker — a crashed holder's, whether it had bound its instance or
    /// not, or bytes this build cannot read — is the inherited unclean obligation
    /// this acquisition carries until a full audit discharges it.
    ///
    /// Exclusion is taken on the store directory node itself, before any name inside that
    /// directory is opened. A lock resting only on names inside the directory does not
    /// survive their replacement: the marker and the engine file are each replaceable by
    /// unlinking the name and creating another node under it, which hands a contender a node
    /// no holder holds. Each replacement alone is still refused by the other node's lock,
    /// but replacing both leaves neither — the state a whole-directory restore over a live
    /// store also produces. The directory node is the one node in the store that no
    /// replacement of its own children changes, and acquisition pinned it by canonicalizing
    /// before asking for this lock, so exclusion rests on it and the marker's lock stands
    /// behind it.
    fn acquire(dir: &Path) -> Result<AcquiredLock, NativeLockError> {
        let directory_node = open_directory_node(dir).map_err(NativeLockError::Io)?;
        match directory_node.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                // A contender is owed the exclusion verdict and nothing else: a marker it
                // cannot read or that is not there costs it the holder's identity, never
                // the verdict itself.
                return Err(NativeLockError::StoreInUse {
                    owner: read_named_owner(dir),
                });
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(NativeLockError::Io(error));
            }
        }

        let mut file = open_marker(dir).map_err(NativeLockError::Io)?;

        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                // A contender is owed the exclusion verdict and nothing else: an
                // unreadable marker costs it the holder's identity, never the
                // verdict itself.
                return Err(NativeLockError::StoreInUse {
                    owner: read_owner(&mut file),
                });
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(NativeLockError::Io(error));
            }
        }

        // Only now that exclusion is settled. A second link to the marker leaves the bytes
        // this acquisition is about to publish rewritable under a name the owner does not
        // hold, so it is refused — but it does not divide exclusion (every opener of either
        // name locks the same node), and refusing on it ahead of the lock would hand a
        // contender an I/O verdict where the exclusion verdict applies.
        let held = file.metadata().map_err(NativeLockError::Io)?;
        admit_held_marker(&held).map_err(NativeLockError::Io)?;
        let prior_unclean = held.len() != 0;
        let acquired_unix_secs = now_unix_secs();
        write_owner(
            &mut file,
            NativeLockOwner {
                pid: std::process::id(),
                instance: None,
                acquired_unix_secs,
            },
        )
        .map_err(NativeLockError::Io)?;
        sync_dir(dir).map_err(NativeLockError::Io)?;

        Ok(AcquiredLock {
            lock: OwnerLock {
                directory_node: Some(directory_node),
                file: Some(file),
                disposition: DropDisposition::PreserveUnclean,
            },
            prior_unclean,
            acquired_unix_secs,
        })
    }

    /// Publish the store instance this held lock is now open against, so a
    /// contender and a crash forensic both name the exact store. Binding adds
    /// the instance to the record acquisition wrote and changes nothing else,
    /// so the acquisition time it carries is the one acquisition observed.
    fn bind(&mut self, instance: [u8; 16], acquired_unix_secs: u64) -> Result<(), NativeLockError> {
        let file = self
            .file
            .as_mut()
            .expect("a held owner lock retains its marker");
        write_owner(
            file,
            NativeLockOwner {
                pid: std::process::id(),
                instance: Some(instance),
                acquired_unix_secs,
            },
        )
        .map_err(NativeLockError::Io)
    }

    fn mark_clean(&mut self) {
        debug_assert_ne!(self.disposition, DropDisposition::Quarantine);
        if self.disposition != DropDisposition::Quarantine {
            self.disposition = DropDisposition::Clean;
        }
    }

    fn quarantine(&mut self) {
        self.disposition = DropDisposition::Quarantine;
    }
}

impl Drop for OwnerLock {
    fn drop(&mut self) {
        match self.disposition {
            DropDisposition::PreserveUnclean => {}
            DropDisposition::Clean => {
                if let Some(file) = &self.file {
                    let _ = file.set_len(0);
                    let _ = file.sync_all();
                }
            }
            // Quarantine is exclusion for the rest of this process's life, so every handle
            // the exclusion rests on is retained. Releasing the directory node while the
            // marker's lock is leaked would leave the quarantine standing on a name that a
            // writer inside the directory can replace.
            DropDisposition::Quarantine => {
                for handle in [self.directory_node.take(), self.file.take()]
                    .into_iter()
                    .flatten()
                {
                    std::mem::forget(handle);
                }
            }
        }
    }
}

/// The only public native-engine capability. The raw engine and owner lock are
/// private and cannot be detached or replaced by safe dependents.
///
/// ```compile_fail
/// use marrow_store::NativeEngineOwner;
/// fn detach(owner: NativeEngineOwner) {
///     let _raw_engine = owner.engine;
///     let _raw_lock = owner.lock;
/// }
/// ```
pub struct NativeEngineOwner {
    engine: Option<NativeEngine>,
    lock: OwnerLock,
    directory: PathBuf,
}

/// One store directory's owner lock, held before anything in that directory has
/// been read and before any engine call. It is affine: the single way to reach a
/// live engine consumes it, and dropping it instead releases the lock while
/// preserving whatever unclean obligation it inherited, so a refusal taken under
/// this owner leaves the next acquisition owing the same full audit.
///
/// The lock is private and cannot be detached or re-armed by safe dependents.
///
/// ```compile_fail
/// use marrow_store::PendingNativeEngineOwner;
/// fn detach(pending: PendingNativeEngineOwner) {
///     let _raw_lock = pending.lock;
/// }
/// ```
pub struct PendingNativeEngineOwner {
    lock: OwnerLock,
    prior_unclean: bool,
    acquired_unix_secs: u64,
    directory: PathBuf,
}

impl PendingNativeEngineOwner {
    /// The canonical store directory this owner holds. The owner above reads the
    /// directory's own artifacts from here under the exclusion already taken.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Publish `instance` in the owner marker, run the zero-capability admission
    /// callback, then open and — when this owner inherited an unclean obligation
    /// — fully audit the existing engine. The callback runs after the marker
    /// names the store and before any engine call, so a refusal makes zero engine
    /// calls and hands the obligation on intact.
    pub fn bind_and_open_existing<R>(
        mut self,
        instance: [u8; 16],
        admit: impl FnOnce() -> Result<(), R>,
    ) -> Result<NativeEngineOwner, NativeOwnerOpenError<R>> {
        self.lock
            .bind(instance, self.acquired_unix_secs)
            .map_err(NativeOwnerOpenError::Lock)?;
        admit().map_err(NativeOwnerOpenError::Refused)?;

        let mut engine = NativeEngine::open_existing(&self.directory.join(NATIVE_ENGINE_FILE))
            .map_err(NativeOwnerOpenError::Store)?;
        if self.prior_unclean {
            engine
                .audit_integrity()
                .map_err(NativeOwnerOpenError::Store)?;
        }
        let Self {
            mut lock,
            directory,
            ..
        } = self;
        lock.mark_clean();
        Ok(NativeEngineOwner {
            engine: Some(engine),
            lock,
            directory,
        })
    }
}

impl NativeEngineOwner {
    /// Create and stamp a new native engine in `store_dir`, returning no live
    /// engine capability. An existing engine path is refused without opening or
    /// modifying it.
    pub fn provision(store_dir: &Path) -> Result<(), StoreError> {
        let directory = std::fs::canonicalize(store_dir).map_err(|error| StoreError::Io {
            op: "provision",
            message: error.to_string(),
        })?;
        let engine = NativeEngine::create_new(&directory.join(NATIVE_ENGINE_FILE))?;
        drop(engine);
        Ok(())
    }

    /// Pin `store_dir` to its canonical path and take its owner lock, making no
    /// engine call and requiring no store instance. Exclusion is decided here, so
    /// no byte of the store directory can be read — or fail to read — ahead of it.
    pub fn acquire_existing(
        store_dir: &Path,
    ) -> Result<PendingNativeEngineOwner, NativeOwnerAcquireError> {
        let directory = std::fs::canonicalize(store_dir).map_err(NativeOwnerAcquireError::Io)?;
        let acquired = OwnerLock::acquire(&directory).map_err(NativeOwnerAcquireError::Lock)?;
        Ok(PendingNativeEngineOwner {
            lock: acquired.lock,
            prior_unclean: acquired.prior_unclean,
            acquired_unix_secs: acquired.acquired_unix_secs,
            directory,
        })
    }

    /// Irreversibly quarantine this owner's lock, close the old engine, reopen
    /// the existing file under the same lock, and run a full integrity audit.
    /// No successful result can restore clean-on-drop behavior.
    pub fn reopen_existing_and_audit(mut self) -> Result<Self, StoreError> {
        self.lock.quarantine();
        drop(self.engine.take());
        let mut engine = NativeEngine::open_existing(&self.directory.join(NATIVE_ENGINE_FILE))?;
        engine.audit_integrity()?;
        self.engine = Some(engine);
        Ok(self)
    }

    fn engine(&self) -> &NativeEngine {
        self.engine
            .as_ref()
            .expect("a live native owner retains its engine")
    }

    fn engine_mut(&mut self) -> &mut NativeEngine {
        self.engine
            .as_mut()
            .expect("a live native owner retains its engine")
    }
}

/// A coherent read view that cannot outlive its native owner.
pub struct NativeOwnerView<'a> {
    inner: RedbView<'a>,
}

impl ReadView for NativeOwnerView<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        self.inner.scan_after(prefix, cursor)
    }
}

/// A native transaction whose commit verdict controls the physical owner lock.
pub struct NativeOwnerTxn<'a> {
    inner: RedbTxn<'a>,
    lock: &'a mut OwnerLock,
}

impl ReadView for NativeOwnerTxn<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        self.inner.scan_after(prefix, cursor)
    }
}

impl WriteTxn for NativeOwnerTxn<'_> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.inner.put(key, value)
    }

    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.inner.remove(key)
    }

    fn commit(self) -> CommitOutcome {
        let Self { inner, lock } = self;
        commit_and_latch(inner, lock)
    }
}

fn commit_and_latch<T: WriteTxn>(inner: T, lock: &mut OwnerLock) -> CommitOutcome {
    let outcome = inner.commit();
    if outcome == CommitOutcome::Indeterminate {
        lock.quarantine();
    }
    outcome
}

impl ByteEngine for NativeEngineOwner {
    type View<'a> = NativeOwnerView<'a>;
    type Txn<'a> = NativeOwnerTxn<'a>;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        Ok(NativeOwnerView {
            inner: self.engine().read_view()?,
        })
    }

    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        let Self { engine, lock, .. } = self;
        let inner = engine
            .as_mut()
            .expect("a live native owner retains its engine")
            .begin()?;
        Ok(NativeOwnerTxn { inner, lock })
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        self.engine().require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.engine_mut().audit_integrity()
    }
}

/// Open the store directory itself as the node exclusion is taken on. Nothing is read or
/// written through this handle: it exists so that the advisory lock rests on the one node in
/// the store that a writer inside the store directory cannot replace under its own name. The
/// path was canonicalized before acquisition asked for it, so the node this reaches is the
/// directory the owner is about to hold.
#[cfg(unix)]
fn open_directory_node(dir: &Path) -> std::io::Result<File> {
    File::open(dir)
}

/// The marker's custody rests on node identity and link counts, which this crate reads
/// through the Unix metadata it has, and the directory node is opened as an ordinary handle
/// only on platforms where that is defined. A platform without them is refused rather than
/// served by a weaker check.
#[cfg(not(unix))]
fn open_directory_node(_dir: &Path) -> std::io::Result<File> {
    Err(marker_refusal(
        "the store directory is admitted on Unix platforms only",
    ))
}

/// The holder identity the store directory's marker names, read without creating the entry
/// and without locking it. A contender refused at the directory node is owed the exclusion
/// verdict; the identity is the detail attached to it, so every failure to read one is an
/// absent identity rather than a different verdict.
fn read_named_owner(dir: &Path) -> Option<NativeLockOwner> {
    let mut file = File::open(dir.join(NATIVE_LOCK_FILE)).ok()?;
    read_owner(&mut file)
}

/// Open the store directory's owner marker, creating it when absent, as that directory's
/// own regular file.
///
/// The entry is classified before the open, so in the ordinary case a link standing in for
/// the marker is refused rather than created through; a link planted between that
/// classification and the open can still be created through, and the comparison of the
/// opened node against the entry afterwards is what refuses it. Either way the handle every
/// later read, write, and lock call uses is the node the directory names. How many names
/// reach that node is deliberately not decided here: a second link does not divide
/// exclusion, so it is admitted after the lock. This is one process's custody of its own
/// store directory, not a defence against a hostile writer inside it: that actor already
/// holds the store's bytes.
#[cfg(unix)]
fn open_marker(dir: &Path) -> std::io::Result<File> {
    let path = dir.join(NATIVE_LOCK_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(named) => admit_marker_node(&named)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    let opened = file.metadata()?;
    admit_marker_node(&opened)?;
    if !names_same_node(&std::fs::symlink_metadata(&path)?, &opened) {
        return Err(marker_refusal(
            "the store lock entry does not name the opened marker",
        ));
    }
    Ok(file)
}

/// The marker's custody rests on link counts and node identity, which this crate reads
/// through the Unix metadata it has. A platform without them is refused rather than served
/// by a weaker check.
#[cfg(not(unix))]
fn open_marker(_dir: &Path) -> std::io::Result<File> {
    Err(marker_refusal(
        "the store lock is admitted on Unix platforms only",
    ))
}

#[cfg(unix)]
fn admit_marker_node(entry: &Metadata) -> std::io::Result<()> {
    if entry.file_type().is_file() {
        Ok(())
    } else {
        Err(marker_refusal("the store lock is not a regular file"))
    }
}

/// The marker admission that may run only once exclusion is settled: the bytes an owner
/// publishes must be reachable under exactly the name it holds.
#[cfg(unix)]
fn admit_held_marker(entry: &Metadata) -> std::io::Result<()> {
    if std::os::unix::fs::MetadataExt::nlink(entry) == 1 {
        Ok(())
    } else {
        Err(marker_refusal("the store lock carries more than one link"))
    }
}

#[cfg(not(unix))]
fn admit_held_marker(_entry: &Metadata) -> std::io::Result<()> {
    Err(marker_refusal(
        "the store lock is admitted on Unix platforms only",
    ))
}

fn marker_refusal(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

#[cfg(unix)]
fn names_same_node(named: &Metadata, opened: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    named.dev() == opened.dev() && named.ino() == opened.ino()
}

/// The holder identity a marker carries, or `None` when it carries none this build
/// can read. Every failure reads as an absent identity: this is the detail attached
/// to a contention verdict, never the verdict itself.
fn read_owner(file: &mut File) -> Option<NativeLockOwner> {
    let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
    if len == 0 || len > BOUND_BYTES.max(LEGACY_BOUND_BYTES) {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes).ok()?;
    NativeLockOwner::decode(&bytes)
}

fn write_owner(file: &mut File, owner: NativeLockOwner) -> std::io::Result<()> {
    let bytes = owner.encode();
    file.set_len(bytes.len() as u64)?;
    file.sync_all()?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&bytes)?;
    file.sync_all()
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(any(unix, windows))]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(any(unix, windows)))]
fn sync_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::redb::{Database, ReadableDatabase, TableDefinition};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "marrow-native-owner-{tag}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Acquire, bind, and open in the one order production uses.
    fn open_existing(
        dir: &Path,
        instance: [u8; 16],
    ) -> Result<NativeEngineOwner, NativeOwnerOpenError<std::convert::Infallible>> {
        NativeEngineOwner::acquire_existing(dir)
            .expect("acquire the owner lock")
            .bind_and_open_existing(instance, || Ok(()))
    }

    fn marker_bytes(dir: &Path) -> Vec<u8> {
        std::fs::read(dir.join(NATIVE_LOCK_FILE)).expect("read the owner marker")
    }

    fn contend(dir: &Path) -> NativeOwnerAcquireError {
        match NativeEngineOwner::acquire_existing(dir) {
            Err(error) => error,
            Ok(_) => panic!("a contender acquired a held store"),
        }
    }

    #[test]
    fn provision_is_create_only_and_existing_open_holds_the_lock() {
        let scratch = Scratch::new("provision");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        assert!(NativeEngineOwner::provision(&scratch.0).is_err());

        let owner = open_existing(&scratch.0, [7; 16]).expect("open owner");
        assert!(matches!(
            contend(&scratch.0),
            NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
        ));
        drop(owner);
        open_existing(&scratch.0, [8; 16]).expect("clean close releases lock");
    }

    /// Exclusion is decided before the store directory is read, and the marker
    /// names the holder as precisely as the holder has published: a lock held
    /// before its instance is known carries none, and binding publishes it. A
    /// contender is told the store is locked in both states.
    #[test]
    fn a_contender_is_locked_out_before_and_after_the_holder_binds_its_instance() {
        let scratch = Scratch::new("pending-and-bound-contention");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let pending =
            NativeEngineOwner::acquire_existing(&scratch.0).expect("acquire without an instance");

        match contend(&scratch.0) {
            NativeOwnerAcquireError::Lock(error @ NativeLockError::StoreInUse { .. }) => {
                assert_eq!(error.code(), Code::StoreLocked.as_str());
                let NativeLockError::StoreInUse { owner: Some(owner) } = error else {
                    panic!("a pending holder must still be named");
                };
                assert_eq!(owner.pid, std::process::id());
                assert_eq!(
                    owner.instance, None,
                    "a holder that has not bound an instance must not claim one",
                );
            }
            other => panic!("a pending holder must exclude a contender: {other}"),
        }

        let owner = pending
            .bind_and_open_existing([0x5B; 16], || Ok::<_, std::convert::Infallible>(()))
            .expect("bind and open");
        match contend(&scratch.0) {
            NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { owner: Some(named) }) => {
                assert_eq!(named.pid, std::process::id());
                assert_eq!(
                    named.instance,
                    Some([0x5B; 16]),
                    "a bound holder names its store"
                );
            }
            other => panic!("a bound holder must exclude a contender: {other}"),
        }
        drop(owner);
    }

    /// A marker this build cannot read costs a contender the holder's identity and
    /// nothing else: the exclusion verdict never degrades into an I/O or decode
    /// error, which is the whole reason exclusion is decided ahead of any read.
    #[test]
    fn an_unreadable_marker_still_yields_exactly_the_exclusion_verdict() {
        for (tag, body) in [
            ("empty", b"".as_slice()),
            ("garbage", b"not a marker"),
            (
                "wrong-magic",
                b"XXXX\x01\x01\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x01",
            ),
            ("truncated-bound", b"MWSL\x01\x02\x00\x00\x00\x01"),
        ] {
            let scratch = Scratch::new(&format!("unreadable-marker-{tag}"));
            NativeEngineOwner::provision(&scratch.0).expect("provision");
            let held = open_existing(&scratch.0, [0x5C; 16]).expect("open owner");
            std::fs::write(scratch.0.join(NATIVE_LOCK_FILE), body).expect("overwrite the marker");

            match contend(&scratch.0) {
                NativeOwnerAcquireError::Lock(error @ NativeLockError::StoreInUse { .. }) => {
                    assert_eq!(error.code(), Code::StoreLocked.as_str(), "marker {tag}");
                }
                other => panic!("an unreadable {tag} marker changed the verdict: {other}"),
            }
            drop(held);
        }
    }

    /// The decoder admits before it indexes. A contender reads a marker it did not
    /// write, so a byte pattern that aborts the decode would replace the exclusion
    /// verdict with a process abort. Every prefix length of both layouts, at every
    /// version and state tag, either decodes to a whole layout or reports no identity.
    #[test]
    fn no_marker_byte_pattern_can_abort_the_decoder() {
        let widest = BOUND_BYTES.max(LEGACY_BOUND_BYTES);
        for version in [LEGACY_BOUND_VERSION, LOCK_VERSION, 0x02, 0xFF] {
            for tag in [0x00, PENDING_TAG, BOUND_TAG, 0xFF] {
                let mut bytes = Vec::with_capacity(widest);
                bytes.extend_from_slice(LOCK_MAGIC);
                bytes.push(version);
                bytes.push(tag);
                bytes.resize(widest, 0xA5);
                for len in 0..=widest {
                    assert!(
                        NativeLockOwner::decode(&bytes[..len]).is_none()
                            || matches!(len, PENDING_BYTES | BOUND_BYTES | LEGACY_BOUND_BYTES),
                        "a {len}-byte marker at version {version:#04x} tag {tag:#04x} decoded \
                         outside a whole layout",
                    );
                }
            }
        }
    }

    /// Every marker a contender can meet under a live holder still yields exactly the
    /// exclusion verdict. The contender did not write these bytes and is owed no verdict
    /// about them: each truncation of both layouts, each state tag, a foreign magic, plain
    /// garbage, and a marker carrying a second link all read as `store.locked`. A second
    /// link in particular does not divide exclusion — every opener of either name locks the
    /// same node — so refusing on it would convert contention into an I/O verdict.
    #[cfg(unix)]
    #[test]
    fn every_marker_a_contender_can_meet_still_yields_the_exclusion_verdict() {
        let scratch = Scratch::new("contender-marker-sweep");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let held = open_existing(&scratch.0, [0x5D; 16]).expect("open owner");
        let marker = scratch.0.join(NATIVE_LOCK_FILE);

        let mut bodies: Vec<Vec<u8>> = Vec::new();
        for instance in [None, Some([0x5E; 16])] {
            let encoded = NativeLockOwner {
                pid: 4242,
                instance,
                acquired_unix_secs: 7,
            }
            .encode();
            for len in 0..=encoded.len() {
                bodies.push(encoded[..len].to_vec());
            }
        }
        for tag in 0..=u8::MAX {
            bodies.push(vec![
                LOCK_MAGIC[0],
                LOCK_MAGIC[1],
                LOCK_MAGIC[2],
                LOCK_MAGIC[3],
                LOCK_VERSION,
                tag,
            ]);
        }
        bodies.push(b"XXXX\x01\x02".to_vec());
        bodies.push(b"not a marker at all".to_vec());

        for body in &bodies {
            std::fs::write(&marker, body).expect("rewrite the marker under the holder");
            match contend(&scratch.0) {
                NativeOwnerAcquireError::Lock(error @ NativeLockError::StoreInUse { .. }) => {
                    assert_eq!(
                        error.code(),
                        Code::StoreLocked.as_str(),
                        "a {}-byte marker changed the verdict",
                        body.len(),
                    );
                }
                other => panic!("a {}-byte marker changed the verdict: {other}", body.len(),),
            }
        }

        std::fs::hard_link(&marker, scratch.0.join("marker-alias")).expect("add a second link");
        match contend(&scratch.0) {
            NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }) => {}
            other => panic!("a multiply-linked marker preempted the exclusion verdict: {other}"),
        }
        drop(held);
    }

    /// The marker layout this build writes round-trips, and the layout it replaced
    /// still reads as the bound owner it recorded. A stored marker outlives the
    /// process that wrote it, so an older holder's bytes must stay legible.
    #[test]
    fn the_marker_round_trips_and_still_reads_the_layout_it_replaced() {
        for instance in [None, Some([0x6A; 16])] {
            let owner = NativeLockOwner {
                pid: 4321,
                instance,
                acquired_unix_secs: 0x0102_0304_0506_0708,
            };
            let encoded = owner.encode();
            assert_eq!(
                encoded.len(),
                match instance {
                    Some(_) => BOUND_BYTES,
                    None => PENDING_BYTES,
                },
            );
            assert_eq!(NativeLockOwner::decode(&encoded), Some(owner));
        }

        let mut legacy = [0u8; LEGACY_BOUND_BYTES];
        legacy[0..4].copy_from_slice(LOCK_MAGIC);
        legacy[4] = LEGACY_BOUND_VERSION;
        legacy[5..9].copy_from_slice(&7u32.to_be_bytes());
        legacy[9..25].copy_from_slice(&[0x6B; 16]);
        legacy[25..33].copy_from_slice(&99u64.to_be_bytes());
        assert_eq!(
            NativeLockOwner::decode(&legacy),
            Some(NativeLockOwner {
                pid: 7,
                instance: Some([0x6B; 16]),
                acquired_unix_secs: 99,
            }),
            "a marker written by the layout this build replaced must still name its owner",
        );
    }

    /// The unclean obligation a crashed holder leaves is inherited by the next
    /// acquisition and survives every outcome short of a completed open: a holder
    /// that dies pending, a holder that dies bound, and an open refused at
    /// admission all leave the next acquisition owing the same full audit. Only a
    /// clean close discharges it.
    #[test]
    fn an_inherited_unclean_obligation_survives_refusal_and_drop() {
        for (tag, bind_before_death) in [("pending-death", false), ("bound-death", true)] {
            let scratch = Scratch::new(tag);
            NativeEngineOwner::provision(&scratch.0).expect("provision");

            // A holder that never closes cleanly: the marker keeps its body.
            let pending =
                NativeEngineOwner::acquire_existing(&scratch.0).expect("acquire the owner");
            if bind_before_death {
                let refused = pending
                    .bind_and_open_existing([0x6C; 16], || Err::<(), _>("refused"))
                    .err()
                    .expect("the admission refusal is the death point");
                assert!(matches!(refused, NativeOwnerOpenError::Refused("refused")));
            } else {
                drop(pending);
            }
            assert!(
                !marker_bytes(&scratch.0).is_empty(),
                "{tag} must leave the unclean obligation behind",
            );

            // Inheriting it and refusing again hands the same obligation on.
            let inherited = NativeEngineOwner::acquire_existing(&scratch.0)
                .expect("inherit the obligation")
                .bind_and_open_existing([0x6D; 16], || Err::<(), _>("refused again"))
                .err()
                .expect("the second admission also refuses");
            assert!(matches!(
                inherited,
                NativeOwnerOpenError::Refused("refused again"),
            ));
            assert!(
                !marker_bytes(&scratch.0).is_empty(),
                "{tag} must not let a refusal discharge an inherited obligation",
            );

            // Only a completed open and clean close discharges it.
            drop(open_existing(&scratch.0, [0x6E; 16]).expect("a full open discharges it"));
            assert!(
                marker_bytes(&scratch.0).is_empty(),
                "{tag} must be discharged by a clean close",
            );
        }
    }

    /// The owner marker is admitted as the store directory's own regular
    /// single-link entry: a link standing in for it is refused rather than read or
    /// created through, and a second hard link to it is refused outright.
    #[cfg(unix)]
    #[test]
    fn the_owner_marker_refuses_a_substituted_or_multiply_linked_entry() {
        let scratch = Scratch::new("marker-substitution");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let elsewhere = scratch.0.join("elsewhere");
        let marker = scratch.0.join(NATIVE_LOCK_FILE);

        std::os::unix::fs::symlink(&elsewhere, &marker).expect("link the marker name away");
        match contend(&scratch.0) {
            NativeOwnerAcquireError::Lock(NativeLockError::Io(_)) => {}
            other => panic!("a linked marker must be refused, not followed: {other}"),
        }
        assert!(
            !elsewhere.exists(),
            "a refused marker open must not create the node the link named",
        );
        std::fs::remove_file(&marker).expect("remove the link");

        std::fs::write(&marker, b"").expect("create a real marker");
        std::fs::hard_link(&marker, scratch.0.join("marker-alias")).expect("add a second link");
        match contend(&scratch.0) {
            NativeOwnerAcquireError::Lock(NativeLockError::Io(_)) => {}
            other => panic!("a multiply-linked marker must be refused: {other}"),
        }
    }

    #[test]
    fn admission_runs_under_lock_before_engine_open() {
        let scratch = Scratch::new("admission");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let error = NativeEngineOwner::acquire_existing(&scratch.0)
            .expect("acquire the owner")
            .bind_and_open_existing([9; 16], || {
                assert!(matches!(
                    contend(&scratch.0),
                    NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
                ));
                Err::<(), _>("refused")
            });
        assert!(matches!(
            error,
            Err(NativeOwnerOpenError::Refused("refused"))
        ));
        open_existing(&scratch.0, [10; 16])
            .expect("a pre-engine refusal releases its non-quarantined lock");
    }

    #[test]
    fn existing_owner_open_refuses_missing_and_invalid_bodies_without_adopting_them() {
        let missing = Scratch::new("missing-existing");
        let missing_path = missing.0.join(NATIVE_ENGINE_FILE);
        for _ in 0..2 {
            assert!(matches!(
                open_existing(&missing.0, [0x21; 16]),
                Err(NativeOwnerOpenError::Store(_))
            ));
            assert!(
                !missing_path.exists(),
                "an owner open must leave a missing engine path absent",
            );
        }

        for (tag, bytes) in [
            ("empty-existing", b"".as_slice()),
            ("bad-existing", b"not redb"),
        ] {
            let scratch = Scratch::new(tag);
            let path = scratch.0.join(NATIVE_ENGINE_FILE);
            std::fs::write(&path, bytes).expect("write invalid engine body");
            assert!(matches!(
                open_existing(&scratch.0, [0x22; 16]),
                Err(NativeOwnerOpenError::Store(_))
            ));
            assert_eq!(
                std::fs::read(&path).expect("read refused engine body"),
                bytes,
                "an owner open must not rewrite or stamp an invalid engine body",
            );
        }

        let unstamped = Scratch::new("unstamped-existing");
        let path = unstamped.0.join(NATIVE_ENGINE_FILE);
        drop(Database::create(&path).expect("create an unstamped redb database"));
        assert!(matches!(
            open_existing(&unstamped.0, [0x23; 16]),
            Err(NativeOwnerOpenError::Store(_))
        ));
        let db = Database::open(&path).expect("reopen refused unstamped database");
        let read = db.begin_read().expect("read unstamped database");
        const META: TableDefinition<&str, u32> = TableDefinition::new("marrow.meta");
        assert!(
            matches!(
                read.open_table(META),
                Err(::redb::TableError::TableDoesNotExist(_))
            ),
            "an owner open must not stamp an otherwise valid foreign database",
        );
    }

    #[test]
    fn recovery_reopen_is_irreversibly_quarantined_after_success() {
        let scratch = Scratch::new("quarantine-success");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let owner = open_existing(&scratch.0, [11; 16]).expect("open owner");
        let mut owner = owner
            .reopen_existing_and_audit()
            .expect("reopen and audit under retained lock");
        let mut txn = owner
            .begin()
            .expect("known recovery owner remains writable");
        txn.put(b"known", b"usable".to_vec())
            .expect("write through recovered owner");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        assert_eq!(
            owner
                .read_view()
                .expect("known recovery read view")
                .get(b"known")
                .expect("read through recovered owner"),
            Some(b"usable".to_vec()),
        );
        drop(owner);

        assert!(matches!(
            contend(&scratch.0),
            NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
        ));
        assert_ne!(
            std::fs::metadata(scratch.0.join(NATIVE_LOCK_FILE))
                .expect("lock metadata")
                .len(),
            0,
            "quarantine retains the nonempty owner marker",
        );
    }

    /// Quarantine retains every node the exclusion rests on, not only the marker. A
    /// quarantine standing on the marker alone would end the moment that name is unlinked
    /// and another node created under it — the replacement the directory node exists to
    /// survive — so the store would become openable again before this process exits.
    #[cfg(unix)]
    #[test]
    fn quarantine_survives_the_replacement_of_the_marker_it_leaked() {
        let scratch = Scratch::new("quarantine-replaced-marker");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let owner = open_existing(&scratch.0, [17; 16]).expect("open owner");
        drop(
            owner
                .reopen_existing_and_audit()
                .expect("reopen and audit under retained lock"),
        );

        std::fs::remove_file(scratch.0.join(NATIVE_LOCK_FILE))
            .expect("remove the quarantined marker");
        assert!(matches!(
            contend(&scratch.0),
            NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
        ));
    }

    #[test]
    fn failed_recovery_reopen_never_recreates_and_remains_quarantined() {
        let scratch = Scratch::new("quarantine-missing");
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        let owner = open_existing(&scratch.0, [13; 16]).expect("open owner");
        let engine_path = scratch.0.join(NATIVE_ENGINE_FILE);
        std::fs::remove_file(&engine_path).expect("remove engine");
        assert!(owner.reopen_existing_and_audit().is_err());
        assert!(
            !engine_path.exists(),
            "recovery must not recreate the engine"
        );
        assert!(matches!(
            contend(&scratch.0),
            NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
        ));
    }

    #[test]
    fn failed_recovery_reopen_never_adopts_invalid_replacements() {
        for (tag, replacement) in [
            ("quarantine-empty", b"".as_slice()),
            ("quarantine-malformed", b"not redb"),
        ] {
            let scratch = Scratch::new(tag);
            NativeEngineOwner::provision(&scratch.0).expect("provision");
            let owner = open_existing(&scratch.0, [0x31; 16]).expect("open owner");
            let engine_path = scratch.0.join(NATIVE_ENGINE_FILE);
            std::fs::remove_file(&engine_path).expect("remove live engine path");
            std::fs::write(&engine_path, replacement).expect("install invalid replacement");

            assert!(owner.reopen_existing_and_audit().is_err());
            assert_eq!(
                std::fs::read(&engine_path).expect("read refused replacement"),
                replacement,
                "recovery must not rewrite or stamp an invalid replacement",
            );
            assert!(matches!(
                contend(&scratch.0),
                NativeOwnerAcquireError::Lock(NativeLockError::StoreInUse { .. }),
            ));
        }
    }

    struct VerdictTxn(CommitOutcome);

    impl ReadView for VerdictTxn {
        fn get(&self, _key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
            Ok(None)
        }

        fn scan_after(&self, _prefix: &[u8], _cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
            Ok(Vec::new())
        }
    }

    impl WriteTxn for VerdictTxn {
        fn put(&mut self, _key: &[u8], _value: Vec<u8>) -> Result<(), StoreError> {
            Ok(())
        }

        fn remove(&mut self, _key: &[u8]) -> Result<(), StoreError> {
            Ok(())
        }

        fn commit(self) -> CommitOutcome {
            self.0
        }
    }

    #[test]
    fn transaction_wrapper_latches_only_an_indeterminate_engine_outcome() {
        for (tag, outcome, quarantined) in [
            ("confirmed", CommitOutcome::Confirmed, false),
            ("aborted", CommitOutcome::Aborted, false),
            ("indeterminate", CommitOutcome::Indeterminate, true),
        ] {
            let scratch = Scratch::new(tag);
            NativeEngineOwner::provision(&scratch.0).expect("provision");
            let mut owner = open_existing(&scratch.0, [17; 16]).expect("open owner");
            assert_eq!(
                commit_and_latch(VerdictTxn(outcome), &mut owner.lock),
                outcome,
                "the transaction wrapper commits once and preserves the engine verdict",
            );
            drop(owner);
            assert_eq!(
                matches!(
                    NativeEngineOwner::acquire_existing(&scratch.0),
                    Err(NativeOwnerAcquireError::Lock(
                        NativeLockError::StoreInUse { .. }
                    ))
                ),
                quarantined,
                "only Indeterminate may retain exclusion",
            );
        }
    }

    #[cfg(unix)]
    struct ChildGuard(Option<std::process::Child>);

    #[cfg(unix)]
    impl ChildGuard {
        fn spawn(directory: &Path, mode: &str) -> Self {
            let child =
                std::process::Command::new(std::env::current_exe().expect("test executable"))
                    .args([
                        "--exact",
                        "native_owner::tests::coordinated_quarantine_child_helper",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("MARROW_NATIVE_OWNER_COORDINATED_DIR", directory)
                    .env("MARROW_NATIVE_OWNER_COORDINATED_MODE", mode)
                    .spawn()
                    .expect("spawn coordinated quarantine child");
            Self(Some(child))
        }

        fn id(&self) -> u32 {
            self.0.as_ref().expect("live child").id()
        }

        fn wait_success(mut self) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                let status = self
                    .0
                    .as_mut()
                    .expect("live child")
                    .try_wait()
                    .expect("poll coordinated child exit");
                if let Some(status) = status {
                    self.0.take();
                    assert!(status.success(), "coordinated child failed: {status}");
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    let mut child = self.0.take().expect("live child");
                    let _ = child.kill();
                    let status = child.wait().expect("reap timed-out coordinated child");
                    panic!("coordinated child did not exit before the deadline: {status}");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    #[cfg(unix)]
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    #[cfg(unix)]
    fn phase_path(directory: &Path, mode: &str, phase: &str, kind: &str) -> PathBuf {
        directory.join(format!(".quarantine-{mode}-{phase}-{kind}"))
    }

    #[cfg(unix)]
    fn wait_for_phase(child: &mut ChildGuard, directory: &Path, mode: &str, phase: &str) {
        let ready = phase_path(directory, mode, phase, "ready");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if ready.exists() {
                return;
            }
            if let Some(status) = child
                .0
                .as_mut()
                .expect("live child")
                .try_wait()
                .expect("poll coordinated child")
            {
                panic!("coordinated child exited before {mode}/{phase}: {status}");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for coordinated phase {mode}/{phase}",
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn release_phase(directory: &Path, mode: &str, phase: &str) {
        std::fs::write(phase_path(directory, mode, phase, "release"), b"release")
            .expect("release coordinated phase");
    }

    #[cfg(unix)]
    fn child_barrier(directory: &Path, mode: &str, phase: &str) {
        std::fs::write(phase_path(directory, mode, phase, "ready"), phase)
            .expect("publish coordinated phase");
        let release = phase_path(directory, mode, phase, "release");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !release.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out awaiting release for {mode}/{phase}",
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn assert_competing_open_is_exactly_lock_refused(
        directory: &Path,
        child_pid: u32,
        phase: &str,
    ) {
        match NativeEngineOwner::acquire_existing(directory) {
            Err(NativeOwnerAcquireError::Lock(error @ NativeLockError::StoreInUse { .. })) => {
                assert_eq!(error.code(), Code::StoreLocked.as_str(), "phase {phase}");
                match error {
                    NativeLockError::StoreInUse { owner: Some(owner) } => {
                        assert_eq!(owner.pid, child_pid, "phase {phase} owner pid");
                        assert_eq!(owner.instance, Some([0x71; 16]), "phase {phase} instance");
                    }
                    NativeLockError::StoreInUse { owner: None } => {
                        panic!("phase {phase} lost the exact owner detail")
                    }
                    NativeLockError::Io(_) => unreachable!(),
                }
            }
            Err(NativeOwnerAcquireError::Lock(NativeLockError::Io(error))) => {
                panic!("phase {phase} produced lock I/O instead of contention: {error}")
            }
            Err(NativeOwnerAcquireError::Io(error)) => {
                panic!("phase {phase} failed to canonicalize: {error}")
            }
            Ok(_) => panic!("phase {phase} admitted a competing owner"),
        }
    }

    #[cfg(unix)]
    fn seed_audit_body(directory: &Path) {
        let mut owner = open_existing(directory, [0x70; 16]).expect("open audit seed owner");
        let mut txn = owner.begin().expect("begin audit seed transaction");
        for index in 0..64u32 {
            txn.put(format!("k{index:03}").as_bytes(), vec![index as u8; 32])
                .expect("seed audit cell");
        }
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }

    #[cfg(unix)]
    fn corrupt_live_engine_for_audit(directory: &Path) {
        let path = directory.join(NATIVE_ENGINE_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open live engine for hostile mutation");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read live engine");
        for offset in (0..bytes.len()).step_by(97) {
            bytes[offset] ^= 0xff;
        }
        file.seek(SeekFrom::Start(0)).expect("rewind live engine");
        file.write_all(&bytes).expect("write hostile mutation");
        file.sync_all().expect("sync hostile mutation");
    }

    #[cfg(unix)]
    fn run_coordinated_quarantine_case(mode: &str) {
        let scratch = Scratch::new(mode);
        NativeEngineOwner::provision(&scratch.0).expect("provision");
        if mode == "audit-failure" {
            seed_audit_body(&scratch.0);
        }
        let pristine =
            std::fs::read(scratch.0.join(NATIVE_ENGINE_FILE)).expect("read pristine engine");
        let mut child = ChildGuard::spawn(&scratch.0, mode);

        wait_for_phase(&mut child, &scratch.0, mode, "before-recovery");
        assert_competing_open_is_exactly_lock_refused(&scratch.0, child.id(), "before-recovery");

        let backup = scratch.0.join("store.redb.before-recovery");
        if mode == "reopen-failure" {
            std::fs::rename(scratch.0.join(NATIVE_ENGINE_FILE), &backup)
                .expect("remove engine before recovery reopen");
        }
        release_phase(&scratch.0, mode, "before-recovery");

        match mode {
            "success" => {
                wait_for_phase(&mut child, &scratch.0, mode, "recovered-live");
                assert_competing_open_is_exactly_lock_refused(
                    &scratch.0,
                    child.id(),
                    "recovered-live",
                );
                release_phase(&scratch.0, mode, "recovered-live");

                wait_for_phase(&mut child, &scratch.0, mode, "recovered-dropped");
                assert_competing_open_is_exactly_lock_refused(
                    &scratch.0,
                    child.id(),
                    "recovered-dropped",
                );
                release_phase(&scratch.0, mode, "recovered-dropped");
            }
            "reopen-failure" => {
                wait_for_phase(&mut child, &scratch.0, mode, "reopen-refused");
                assert_competing_open_is_exactly_lock_refused(
                    &scratch.0,
                    child.id(),
                    "reopen-refused",
                );
                std::fs::rename(&backup, scratch.0.join(NATIVE_ENGINE_FILE))
                    .expect("restore valid engine before child exit");
                release_phase(&scratch.0, mode, "reopen-refused");
            }
            "audit-failure" => {
                wait_for_phase(&mut child, &scratch.0, mode, "reopened-before-audit");
                assert_competing_open_is_exactly_lock_refused(
                    &scratch.0,
                    child.id(),
                    "reopened-before-audit",
                );
                corrupt_live_engine_for_audit(&scratch.0);
                release_phase(&scratch.0, mode, "reopened-before-audit");

                wait_for_phase(&mut child, &scratch.0, mode, "audit-refused");
                assert_competing_open_is_exactly_lock_refused(
                    &scratch.0,
                    child.id(),
                    "audit-refused",
                );
                std::fs::write(scratch.0.join(NATIVE_ENGINE_FILE), &pristine)
                    .expect("restore valid engine before child exit");
                release_phase(&scratch.0, mode, "audit-refused");
            }
            other => panic!("unknown coordinated mode {other}"),
        }

        child.wait_success();
        open_existing(&scratch.0, [0x73; 16]).expect("process exit is the sole quarantine release");
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_is_observed_across_success_and_failed_recovery_phases() {
        for mode in ["success", "reopen-failure", "audit-failure"] {
            run_coordinated_quarantine_case(mode);
        }
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "child-process helper for coordinated quarantine phases"]
    fn coordinated_quarantine_child_helper() {
        let Ok(path) = std::env::var("MARROW_NATIVE_OWNER_COORDINATED_DIR") else {
            return;
        };
        let mode = std::env::var("MARROW_NATIVE_OWNER_COORDINATED_MODE").expect("coordinated mode");
        let directory = Path::new(&path);
        let owner = open_existing(directory, [0x71; 16]).expect("child opens owner");
        child_barrier(directory, &mode, "before-recovery");

        match mode.as_str() {
            "success" => {
                let owner = owner
                    .reopen_existing_and_audit()
                    .expect("successful reopen and audit");
                child_barrier(directory, &mode, "recovered-live");
                drop(owner);
                child_barrier(directory, &mode, "recovered-dropped");
            }
            "reopen-failure" => {
                let error = match owner.reopen_existing_and_audit() {
                    Ok(_) => panic!("a missing recovery engine unexpectedly reopened"),
                    Err(error) => error,
                };
                assert_eq!(error.code(), Code::StoreIo.as_str());
                assert!(
                    matches!(error, StoreError::Io { op: "open", .. }),
                    "missing recovery must fail in the existing-open phase: {error}",
                );
                child_barrier(directory, &mode, "reopen-refused");
            }
            "audit-failure" => {
                let mut owner = owner;
                owner.lock.quarantine();
                drop(owner.engine.take());
                owner.engine = Some(
                    NativeEngine::open_existing(&directory.join(NATIVE_ENGINE_FILE))
                        .expect("fresh existing-only reopen before audit"),
                );
                child_barrier(directory, &mode, "reopened-before-audit");
                let error = owner
                    .engine_mut()
                    .audit_integrity()
                    .expect_err("hostile live mutation must fail the full audit");
                assert_eq!(error.code(), Code::StoreCorruption.as_str());
                drop(owner);
                child_barrier(directory, &mode, "audit-refused");
            }
            other => panic!("unknown coordinated mode {other}"),
        }
    }
}
