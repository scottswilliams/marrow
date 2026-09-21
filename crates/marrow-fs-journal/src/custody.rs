//! Descriptor-rooted directory and file custody.
//!
//! Every operation is descriptor-relative from a retained admitted directory;
//! no operation resolves a multi-component path. Directories are admitted with
//! `DIRECTORY | NOFOLLOW | CLOEXEC`, files are created `CREATE | EXCL` with
//! mode `0600`, and every refusal is typed. No raw descriptor escapes.

use std::fmt;
use std::path::Path;

use crate::entry::EntryName;
use crate::sys;

/// The owner bits a read-write open requires of the entry it opens.
const REQUIRED_RW: u32 = 0o600;
/// The owner bits the read-only debris open requires.
const REQUIRED_READ: u32 = 0o400;
/// The owner bits a directory this owner works in requires: read to list it,
/// write to create and remove entries in it, execute to resolve names inside
/// it. Admission itself needs only read and execute, but every use it admits a
/// directory for needs write too, so admission names the whole requirement and
/// the missing-write half cannot surface later as a generic permission error.
const REQUIRED_DIR: u32 = 0o700;

/// A lossless filesystem identity: the platform's `st_dev` and `st_ino`
/// projected injectively into `u64` each.
///
/// The value distinguishes two objects only while both are live. An open
/// descriptor holds an inode number out of circulation, so a comparison
/// against a descriptor this process still holds is exact. A number that
/// outlived its descriptor — one decoded from a durable record after a
/// crash — may since have been recycled (ext4 and XFS reuse freed numbers;
/// APFS does not), so a durable record must carry evidence beyond the number,
/// and content establishes equivalence, never provenance.
///
/// Removal names a path: neither qualified platform unlinks through a
/// descriptor, so an interval separates validating an object from unlinking
/// the name that held it, and the name can be repointed inside it. No evidence
/// about the object closes that interval; only custody of the name does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FsIdentity {
    dev: u64,
    ino: u64,
}

impl FsIdentity {
    /// Assemble an identity from its projected fields.
    pub const fn new(dev: u64, ino: u64) -> Self {
        Self { dev, ino }
    }

    /// The frozen 16-byte layout: `u64_be(st_dev) || u64_be(st_ino)`.
    pub fn to_bytes(self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&self.dev.to_be_bytes());
        bytes[8..16].copy_from_slice(&self.ino.to_be_bytes());
        bytes
    }

    /// Decode the frozen 16-byte layout.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        let field = |from: usize| -> [u8; 8] {
            bytes[from..from + 8]
                .try_into()
                .expect("an 8-byte field of the fixed layout")
        };
        Self {
            dev: u64::from_be_bytes(field(0)),
            ino: u64::from_be_bytes(field(8)),
        }
    }
}

/// The filesystem node kinds custody distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A directory.
    Directory,
    /// A regular file.
    Regular,
    /// A symbolic link (never followed by custody).
    Symlink,
    /// Any other node kind.
    Other,
}

impl fmt::Display for NodeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Directory => "directory",
            Self::Regular => "regular file",
            Self::Symlink => "symbolic link",
            Self::Other => "other node",
        };
        formatter.write_str(name)
    }
}

/// A point-in-time stat witness for one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryStat {
    pub(crate) identity: FsIdentity,
    pub(crate) kind: NodeKind,
    pub(crate) nlink: u64,
    pub(crate) size: u64,
    pub(crate) mode: u32,
}

impl EntryStat {
    /// The node's identity.
    pub fn identity(&self) -> FsIdentity {
        self.identity
    }

    /// The node's kind.
    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    /// The node's hard-link count.
    pub fn nlink(&self) -> u64 {
        self.nlink
    }

    /// The node's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The node's permission bits.
    pub(crate) fn mode(&self) -> u32 {
        self.mode
    }
}

/// The descriptor-relative operation a refusal names.
///
/// This crate owns the set: a consumer names one of these rather than minting
/// prose, and a test asserts the variant. A consumer whose own step is wider
/// than one call — a publication, a recheck — names the custody call the step
/// turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyOp {
    /// Admitting a directory, by trusted path or as a child.
    AdmitDirectory,
    /// Creating a child directory.
    CreateDirectory,
    /// Creating a regular file `CREATE | EXCL`.
    CreateFile,
    /// Opening an existing regular file.
    OpenFile,
    /// Opening or creating the lock entry.
    OpenLock,
    /// Taking the advisory lock itself.
    Lock,
    /// Hard-linking an entry.
    Link,
    /// Unlinking an entry.
    Unlink,
    /// Statting an entry, a path, or an open handle.
    Stat,
    /// `fsync` of a directory or a file.
    Sync,
    /// Truncating a file.
    Truncate,
    /// Atomically exchanging two entries.
    Exchange,
    /// Renaming an entry, refusing an existing destination.
    RenameNoreplace,
    /// Renaming an entry over its destination.
    RenameReplace,
    /// Appending to a file.
    Append,
    /// Reading a bounded prefix of a file.
    Read,
}

impl fmt::Display for CustodyOp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::AdmitDirectory => "admitting a directory",
            Self::CreateDirectory => "creating a directory",
            Self::CreateFile => "creating a file",
            Self::OpenFile => "opening a file",
            Self::OpenLock => "opening the lock entry",
            Self::Lock => "locking",
            Self::Link => "linking",
            Self::Unlink => "unlinking",
            Self::Stat => "statting",
            Self::Sync => "syncing",
            Self::Truncate => "truncating",
            Self::Exchange => "exchanging entries",
            Self::RenameNoreplace => "renaming without replacement",
            Self::RenameReplace => "renaming over a destination",
            Self::Append => "appending",
            Self::Read => "reading",
        };
        formatter.write_str(name)
    }
}

/// A typed custody refusal. `ENOSYS`, `ENOTSUP`, `EOPNOTSUPP`, unsupported
/// `EINVAL`, `EXDEV`, identity drift, and an unqualified platform all fail
/// closed here rather than degrading into a generic I/O error.
#[derive(Debug)]
pub enum CustodyError {
    /// This build is not running on a qualified platform; every operation
    /// refuses.
    UnqualifiedPlatform {
        /// The running operating system.
        os: &'static str,
        /// The running architecture.
        arch: &'static str,
    },
    /// The platform or filesystem does not support the operation's required
    /// semantics.
    Unsupported { op: CustodyOp },
    /// The destination entry already exists and the operation refuses to
    /// replace it.
    AlreadyExists { op: CustodyOp },
    /// The named entry does not exist.
    NotFound { op: CustodyOp },
    /// The named entry is a symbolic link, which custody never follows.
    SymlinkRefused { op: CustodyOp },
    /// The named entry is not a directory where one is required.
    NotADirectory { op: CustodyOp },
    /// The named entry has the wrong node kind.
    WrongNodeKind { op: CustodyOp, found: NodeKind },
    /// The entry's identity changed between admission and use.
    IdentityDrift { op: CustodyOp },
    /// The entry exists as a regular file whose owner bits do not carry the
    /// access the operation requires, so no process those bits bind can open
    /// it. An entry another user owns whose owner bits fall short is
    /// indistinguishable from one a crash left inside the crate's documented
    /// create-then-`fchmod` window, so the refusal reports what it saw and what
    /// the open required and leaves the repair — restoring `required`, or
    /// removing the entry — to whoever owns it.
    ModeDenied {
        /// The refused operation.
        op: CustodyOp,
        /// The entry's observed permission bits.
        found: u32,
        /// The owner bits the refused open required.
        required: u32,
    },
    /// An unclassified I/O failure.
    Io {
        op: CustodyOp,
        source: std::io::Error,
    },
}

impl fmt::Display for CustodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnqualifiedPlatform { os, arch } => write!(
                formatter,
                "{os}/{arch} is not a qualified platform; filesystem publication refuses"
            ),
            Self::Unsupported { op } => write!(
                formatter,
                "the platform or filesystem does not support {op} semantics"
            ),
            Self::AlreadyExists { op } => {
                write!(formatter, "{op} refuses an existing destination entry")
            }
            Self::NotFound { op } => write!(formatter, "{op} found no such entry"),
            Self::SymlinkRefused { op } => {
                write!(formatter, "{op} refuses a symbolic link")
            }
            Self::NotADirectory { op } => write!(formatter, "{op} requires a directory"),
            Self::WrongNodeKind { op, found } => {
                write!(formatter, "{op} refuses a {found}")
            }
            Self::IdentityDrift { op } => write!(
                formatter,
                "the entry's identity changed under {op}; refusing"
            ),
            Self::ModeDenied {
                op,
                found,
                required,
            } => write!(
                formatter,
                "{op} found an entry whose mode {found:o} denies the required {required:o}; \
                 its owner must restore mode {required:o} on the entry or remove it"
            ),
            Self::Io { op, source } => write!(formatter, "{op} failed: {source}"),
        }
    }
}

impl std::error::Error for CustodyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A retained admitted directory: the descriptor root every operation is
/// relative to. The descriptor is private and cannot be detached.
pub struct AdmittedDir {
    handle: sys::DirHandle,
    identity: FsIdentity,
}

/// Whether this build's adapter qualifies the running platform at all, as the same typed
/// refusal every custody operation returns when it does not. Nothing is opened, created, or
/// stated about any path: this is a property of the build.
///
/// A caller that must not mutate before it learns the answer asks here first. Every custody
/// operation refuses on an unqualified platform anyway, so this is not a second gate — it is
/// the one gate, asked before the caller has anything to undo.
pub fn qualified_platform() -> Result<(), CustodyError> {
    sys::qualified_platform()
}

impl AdmittedDir {
    /// Admit a trusted root directory by path. This is the single path-based
    /// entry into custody: the caller vouches for the path's trust, and the
    /// final component is opened `DIRECTORY | NOFOLLOW | CLOEXEC`.
    pub fn admit_trusted_root(path: &Path) -> Result<Self, CustodyError> {
        let handle = match sys::open_dir_root(path) {
            Ok(handle) => handle,
            Err(refusal) => {
                let observed = sys::lstat_path(path).ok().flatten();
                return Err(refine_dir_refusal(refusal, observed));
            }
        };
        let stat = sys::fstat_dir(&handle)?;
        require_dir_mode(CustodyOp::AdmitDirectory, &stat)?;
        Ok(Self {
            handle,
            identity: stat.identity,
        })
    }

    /// Admit one child directory of this directory.
    pub fn admit_child(&self, name: &EntryName) -> Result<Self, CustodyError> {
        let child = self.open_child(name)?;
        require_dir_mode(CustodyOp::AdmitDirectory, &sys::fstat_dir(&child.handle)?)?;
        Ok(child)
    }

    /// Open a child directory without judging its mode.
    ///
    /// Only [`Self::create_child_dir`] uses this, and only because it restores
    /// the exact mode on the descriptor it opens: the umask may have masked the
    /// requested `0700` a moment earlier, so the mode at this instant is not
    /// the mode the caller will be left with. Every other admission goes
    /// through [`Self::admit_child`], which judges it.
    fn open_child(&self, name: &EntryName) -> Result<Self, CustodyError> {
        let handle = match sys::open_dir_child(&self.handle, name.as_str()) {
            Ok(handle) => handle,
            Err(refusal) => {
                let observed = self.stat_entry(name).ok().flatten();
                return Err(refine_dir_refusal(refusal, observed));
            }
        };
        let stat = sys::fstat_dir(&handle)?;
        Ok(Self {
            handle,
            identity: stat.identity,
        })
    }

    /// Create one child directory (mode `0700` exactly, umask-independent)
    /// and admit it.
    pub fn create_child_dir(&self, name: &EntryName) -> Result<Self, CustodyError> {
        sys::mkdir_child(&self.handle, name.as_str())?;
        let child = self.open_child(name)?;
        sys::restore_dir_mode(&child.handle)?;
        Ok(child)
    }

    /// The directory's identity at admission.
    pub fn identity(&self) -> FsIdentity {
        self.identity
    }

    /// Create one regular file `CREATE | EXCL | NOFOLLOW`, mode `0600`,
    /// witnessing its opened inode.
    pub fn create_file_excl(&self, name: &EntryName) -> Result<OpenedFile, CustodyError> {
        let handle = sys::create_file_excl(&self.handle, name.as_str())?;
        let stat = sys::fstat_file(&handle)?;
        Ok(OpenedFile {
            handle,
            identity: stat.identity,
        })
    }

    /// Open one existing regular file `NOFOLLOW`, witnessing its opened inode.
    pub fn open_file(&self, name: &EntryName) -> Result<OpenedFile, CustodyError> {
        let handle = sys::open_file(&self.handle, name.as_str())
            .map_err(|refusal| refine_open_refusal(refusal, self.observe(name), REQUIRED_RW))?;
        witness_regular(handle, CustodyOp::OpenFile)
    }

    /// Open one existing regular file read-only `NOFOLLOW`, witnessing its
    /// opened inode. Preclaim debris may carry any mode the crash left, and
    /// its only permitted mutation — the witnessed discard — needs the
    /// directory alone, so its custody must not demand write access.
    ///
    /// The same open is what any consumer takes to decide a read-only question
    /// about an entry it may not be able to write: an entry a checkout carries
    /// read-only is still readable, and demanding write access to answer a
    /// question that writes nothing would refuse custody over an entry that
    /// needs none. Open [`open_file`](Self::open_file) instead only where an
    /// append actually follows.
    pub fn open_file_readonly(&self, name: &EntryName) -> Result<OpenedFile, CustodyError> {
        let handle = sys::open_file_readonly(&self.handle, name.as_str())
            .map_err(|refusal| refine_open_refusal(refusal, self.observe(name), REQUIRED_READ))?;
        witness_regular(handle, CustodyOp::OpenFile)
    }

    /// A no-follow stat of `name` taken to name a refusal that has already
    /// been issued. A stat that itself fails leaves the refusal unrefined.
    fn observe(&self, name: &EntryName) -> Option<EntryStat> {
        self.stat_entry(name).ok().flatten()
    }

    /// Re-assert that `name` still maps to `identity`.
    ///
    /// Every mutation this crate performs through a retained handle is guarded
    /// by this one check: a name that has been unlinked and recreated maps to a
    /// different inode, and acting on the retained handle afterwards would act
    /// on an object no name holds. `op` names the operation the assertion
    /// guards, so a drift refusal says what it refused.
    pub fn reassert(
        &self,
        name: &EntryName,
        identity: FsIdentity,
        op: CustodyOp,
    ) -> Result<(), CustodyError> {
        match self.stat_entry(name)? {
            Some(entry) if entry.identity == identity => Ok(()),
            _ => Err(CustodyError::IdentityDrift { op }),
        }
    }

    /// Open this directory's lock entry `name`, creating it when absent, and
    /// witness that it is a regular file.
    ///
    /// Darwin's `openat` reports `ENOENT` for a create-if-absent open while
    /// another thread or process is creating the same entry, and the first
    /// publication of a fresh clone is exactly that race, so absence is
    /// retried `CREATION_RENDEZVOUS_PASSES` times before it is reported.
    ///
    /// The node kind is witnessed before any lock is attempted because `flock`
    /// classifies none: on Darwin it refuses a FIFO with the errno this crate
    /// reads as [`CustodyError::Unsupported`], which would name the platform's
    /// lock semantics rather than the planted node.
    pub(crate) fn open_or_create_lock_entry(
        &self,
        name: &EntryName,
    ) -> Result<OpenedFile, CustodyError> {
        let mut passes = 0;
        let handle = loop {
            match sys::open_lock_file(&self.handle, name.as_str()) {
                Ok(handle) => break handle,
                Err(CustodyError::NotFound { .. }) if passes < CREATION_RENDEZVOUS_PASSES => {
                    passes += 1;
                }
                Err(refusal) => {
                    return Err(refine_open_refusal(
                        refusal,
                        self.observe(name),
                        REQUIRED_RW,
                    ));
                }
            }
        };
        witness_regular(handle, CustodyOp::OpenLock)
    }

    /// Hard-link `existing` to `new_name`, refusing an existing destination.
    pub fn link(&self, existing: &EntryName, new_name: &EntryName) -> Result<(), CustodyError> {
        sys::link(&self.handle, existing.as_str(), new_name.as_str())
    }

    /// Unlink one entry.
    pub fn unlink(&self, name: &EntryName) -> Result<(), CustodyError> {
        sys::unlink(&self.handle, name.as_str())
    }

    /// Stat one entry without following symbolic links; `None` if absent.
    pub fn stat_entry(&self, name: &EntryName) -> Result<Option<EntryStat>, CustodyError> {
        sys::stat_entry(&self.handle, name.as_str())
    }

    /// `fsync` this directory: the durable commit of its entry mutations
    /// within the documented file-and-directory-`fsync` envelope.
    pub fn sync(&self) -> Result<(), CustodyError> {
        sys::sync_dir(&self.handle)
    }

    /// Atomically exchange two entries (`renameat` with `EXCHANGE`). A
    /// platform or filesystem without exchange semantics refuses with a typed
    /// [`CustodyError::Unsupported`], never a fallback.
    pub fn exchange(&self, first: &EntryName, second: &EntryName) -> Result<(), CustodyError> {
        sys::exchange(&self.handle, first.as_str(), second.as_str())
    }

    /// Rename `from` to `to`, refusing an existing destination (`renameat`
    /// with `NOREPLACE`). A platform without the semantics refuses with a
    /// typed [`CustodyError::Unsupported`], never a fallback.
    pub fn rename_noreplace(&self, from: &EntryName, to: &EntryName) -> Result<(), CustodyError> {
        sys::rename_noreplace(&self.handle, from.as_str(), to.as_str())
    }

    /// Atomically replace `to` with `from` within this retained directory.
    /// This changes the entry, not an already-open destination inode. The caller
    /// must sync this directory before claiming durable completion.
    pub fn rename_replace(&self, from: &EntryName, to: &EntryName) -> Result<(), CustodyError> {
        sys::rename_replace(&self.handle, from.as_str(), to.as_str())
    }
}

/// How many times a create-if-absent open may report absence before the
/// refusal is taken at face value.
const CREATION_RENDEZVOUS_PASSES: u32 = 8;

/// Require an opened handle to be a regular file and witness its inode.
fn witness_regular(handle: sys::FileHandle, op: CustodyOp) -> Result<OpenedFile, CustodyError> {
    let stat = sys::fstat_file(&handle)?;
    if stat.kind != NodeKind::Regular {
        return Err(CustodyError::WrongNodeKind {
            op,
            found: stat.kind,
        });
    }
    Ok(OpenedFile {
        handle,
        identity: stat.identity,
    })
}

/// One typed reading of a refused open: an entry that exists as a regular file
/// whose owner bits do not carry `required` names its observed mode and the
/// mode to restore rather than arriving as an unclassified I/O error. The
/// reading rests on the observed mode and on a permission-denied refusal
/// together, so an entry whose bits do carry `required` was refused for some
/// other reason and keeps its original refusal. Nothing was opened either way;
/// the stat only names the refusal.
fn refine_open_refusal(
    refusal: CustodyError,
    observed: Option<EntryStat>,
    required: u32,
) -> CustodyError {
    let denied = match (&refusal, observed) {
        (CustodyError::Io { op, source }, Some(stat))
            if source.kind() == std::io::ErrorKind::PermissionDenied
                && stat.kind == NodeKind::Regular
                && stat.mode & required != required =>
        {
            Some(CustodyError::ModeDenied {
                op: *op,
                found: stat.mode,
                required,
            })
        }
        _ => None,
    };
    denied.unwrap_or(refusal)
}

/// Refuse an admitted directory whose owner bits fall short of what working in
/// it needs. An open succeeds on read and execute alone, so this is what turns
/// a missing owner write into the same typed repair instruction rather than a
/// generic permission error from the first entry someone tries to create.
fn require_dir_mode(op: CustodyOp, stat: &EntryStat) -> Result<(), CustodyError> {
    if stat.mode & REQUIRED_DIR == REQUIRED_DIR {
        return Ok(());
    }
    Err(CustodyError::ModeDenied {
        op,
        found: stat.mode,
        required: REQUIRED_DIR,
    })
}

/// One typed reading of a refused directory admission on every qualified
/// platform.
///
/// Darwin reports a symlink under `O_DIRECTORY | O_NOFOLLOW` as `ENOTDIR`
/// while Linux reports `ELOOP`, so a refusal from either family is refined by
/// one no-follow stat of the refused entry.
///
/// A directory whose owner bits fall short is refined the same way a regular
/// file's is, into the mode-repair refusal. The refinement covers the whole
/// requirement, not the half that stops the open: a mode missing owner read or
/// execute refuses the admission itself, while a mode missing only owner write
/// admits and would refuse the first entry created in it, so the admitted
/// directory's own stat is checked too.
///
/// Nothing was admitted either way; the stat only names the refusal.
fn refine_dir_refusal(refusal: CustodyError, observed: Option<EntryStat>) -> CustodyError {
    if let CustodyError::Io { op, source } = &refusal
        && source.kind() == std::io::ErrorKind::PermissionDenied
        && let Some(stat) = observed
        && stat.kind == NodeKind::Directory
        && stat.mode & REQUIRED_DIR != REQUIRED_DIR
    {
        return CustodyError::ModeDenied {
            op: *op,
            found: stat.mode,
            required: REQUIRED_DIR,
        };
    }
    let op = match &refusal {
        CustodyError::NotADirectory { op } | CustodyError::SymlinkRefused { op } => *op,
        _ => return refusal,
    };
    match observed {
        Some(stat) if stat.kind == NodeKind::Symlink => CustodyError::SymlinkRefused { op },
        Some(stat) if stat.kind != NodeKind::Directory => CustodyError::NotADirectory { op },
        _ => refusal,
    }
}

impl fmt::Debug for AdmittedDir {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdmittedDir")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// One opened regular file witnessing its inode identity. The descriptor is
/// private; writes append, reads are bounded, and sync is a plain `fsync`.
pub struct OpenedFile {
    handle: sys::FileHandle,
    identity: FsIdentity,
}

/// Whether a non-blocking exclusive lock attempt took the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockAcquisition {
    /// This handle now holds the lock.
    Taken,
    /// Another open file description holds it.
    Held,
}

impl OpenedFile {
    /// The inode identity witnessed at open.
    pub fn identity(&self) -> FsIdentity {
        self.identity
    }

    /// Stat the file through its own handle.
    pub fn stat(&self) -> Result<EntryStat, CustodyError> {
        sys::fstat_file(&self.handle)
    }

    /// Append `bytes` at the end of the file.
    pub fn append(&mut self, bytes: &[u8]) -> Result<(), CustodyError> {
        sys::append(&mut self.handle, bytes)
    }

    /// Read at most `max` bytes from the start of the file.
    pub fn read_prefix(&self, max: usize) -> Result<Vec<u8>, CustodyError> {
        sys::read_prefix(&self.handle, max)
    }

    /// `fsync` the file within the documented envelope.
    pub fn sync(&self) -> Result<(), CustodyError> {
        sys::sync_file(&self.handle)
    }

    pub(crate) fn truncate(&self, len: u64) -> Result<(), CustodyError> {
        sys::truncate_file(&self.handle, len)
    }

    /// Attempt the non-blocking exclusive advisory lock on this handle.
    pub(crate) fn try_lock_exclusive(&self) -> Result<LockAcquisition, CustodyError> {
        if sys::try_lock_exclusive(&self.handle)? {
            Ok(LockAcquisition::Taken)
        } else {
            Ok(LockAcquisition::Held)
        }
    }

    /// Restore the fixed `0600` mode a umask may have masked at creation.
    pub(crate) fn restore_lock_mode(&self) -> Result<(), CustodyError> {
        sys::restore_lock_mode(&self.handle)
    }
}

impl fmt::Debug for OpenedFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenedFile")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "custody_tests.rs"]
mod tests;
