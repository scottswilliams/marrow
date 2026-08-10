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
pub(crate) const REQUIRED_RW: u32 = 0o600;
/// The owner bits the read-only debris open requires.
const REQUIRED_READ: u32 = 0o400;

/// A lossless filesystem identity: the platform's `st_dev` and `st_ino`
/// projected injectively into `u64` each.
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

    /// The projected `st_dev`.
    pub const fn dev(self) -> u64 {
        self.dev
    }

    /// The projected `st_ino`.
    pub const fn ino(self) -> u64 {
        self.ino
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
    pub fn mode(&self) -> u32 {
        self.mode
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
    Unsupported { op: &'static str },
    /// The destination entry already exists and the operation refuses to
    /// replace it.
    AlreadyExists { op: &'static str },
    /// The named entry does not exist.
    NotFound { op: &'static str },
    /// The named entry is a symbolic link, which custody never follows.
    SymlinkRefused { op: &'static str },
    /// The named entry is not a directory where one is required.
    NotADirectory { op: &'static str },
    /// The named entry has the wrong node kind.
    WrongNodeKind { op: &'static str, found: NodeKind },
    /// The entry's identity changed between admission and use.
    IdentityDrift { op: &'static str },
    /// The entry exists as a regular file whose permission bits do not carry
    /// the owner access the operation requires, so no open can reach it. A
    /// crash inside the create-then-`fchmod` window under an owner-stripping
    /// umask leaves exactly this state; restoring `required` on the entry, or
    /// removing it, is an operator action.
    ModeDenied {
        /// The refused operation.
        op: &'static str,
        /// The entry's observed permission bits.
        found: u32,
        /// The owner bits the refused open required.
        required: u32,
    },
    /// An unclassified I/O failure.
    Io {
        op: &'static str,
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
                 an operator must restore mode {required:o} on the entry or remove it"
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
    pub(crate) handle: sys::DirHandle,
    pub(crate) identity: FsIdentity,
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
        Ok(Self {
            handle,
            identity: stat.identity,
        })
    }

    /// Admit one child directory of this directory.
    pub fn admit_child(&self, name: &EntryName) -> Result<Self, CustodyError> {
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
        let child = self.admit_child(name)?;
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
        witness_regular(handle)
    }

    /// Open one existing regular file read-only `NOFOLLOW`, witnessing its
    /// opened inode. Preclaim debris may carry any mode the crash left, and
    /// its only permitted mutation — the witnessed discard — needs the
    /// directory alone, so its custody must not demand write access.
    pub(crate) fn open_file_readonly(&self, name: &EntryName) -> Result<OpenedFile, CustodyError> {
        let handle = sys::open_file_readonly(&self.handle, name.as_str())
            .map_err(|refusal| refine_open_refusal(refusal, self.observe(name), REQUIRED_READ))?;
        witness_regular(handle)
    }

    /// A no-follow stat of `name` taken to name a refusal that has already
    /// been issued. A stat that itself fails leaves the refusal unrefined.
    pub(crate) fn observe(&self, name: &EntryName) -> Option<EntryStat> {
        self.stat_entry(name).ok().flatten()
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
}

/// Require an opened handle to be a regular file and witness its inode.
fn witness_regular(handle: sys::FileHandle) -> Result<OpenedFile, CustodyError> {
    let stat = sys::fstat_file(&handle)?;
    if stat.kind != NodeKind::Regular {
        return Err(CustodyError::WrongNodeKind {
            op: "open file",
            found: stat.kind,
        });
    }
    Ok(OpenedFile {
        handle,
        identity: stat.identity,
    })
}

/// One typed reading of a refused open: an entry that exists as a regular file
/// whose owner bits do not carry `required` is refused to every process those
/// bits bind, so the refusal names its observed mode and the mode to restore
/// rather than arriving as an unclassified I/O error. The reading rests on the
/// observed mode and on a permission-denied refusal together: an entry whose
/// bits do carry `required` was refused for some other reason — another user
/// owns it, say — and keeps its original refusal. A process holding the
/// mode-override capability (`root`, or `CAP_DAC_OVERRIDE` on Linux) is not
/// refused by those bits at all: its open succeeds and reaches no reading here.
/// Nothing was opened either way; the stat only names the refusal.
pub(crate) fn refine_open_refusal(
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
                op,
                found: stat.mode,
                required,
            })
        }
        _ => None,
    };
    denied.unwrap_or(refusal)
}

/// One typed reading of a refused directory admission on every qualified
/// platform: Darwin reports a symlink under `O_DIRECTORY | O_NOFOLLOW` as
/// `ENOTDIR` while Linux reports `ELOOP`, so a refusal from either family is
/// refined by one no-follow stat of the refused entry. Nothing was admitted
/// either way; the stat only names the refusal.
fn refine_dir_refusal(refusal: CustodyError, observed: Option<EntryStat>) -> CustodyError {
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
    pub(crate) handle: sys::FileHandle,
    pub(crate) identity: FsIdentity,
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
mod tests {
    use super::*;

    fn permission_denied(op: &'static str) -> CustodyError {
        CustodyError::Io {
            op,
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        }
    }

    fn observed(kind: NodeKind, mode: u32) -> Option<EntryStat> {
        Some(EntryStat {
            identity: FsIdentity::new(1, 2),
            kind,
            nlink: 1,
            size: 0,
            mode,
        })
    }

    /// The mode reading of a refused open, pinned per case. It is what a crash
    /// inside the create-then-`fchmod` window leaves behind on either
    /// qualified platform, so it is asserted here without depending on the
    /// running process's own override capability.
    #[test]
    fn a_stripped_mode_is_read_as_the_typed_mode_refusal() {
        for (found, required) in [
            (0o400, 0o600),
            (0o200, 0o600),
            (0o000, 0o600),
            (0o200, 0o400),
        ] {
            let refined = refine_open_refusal(
                permission_denied("open lock"),
                observed(NodeKind::Regular, found),
                required,
            );
            assert!(
                matches!(refined, CustodyError::ModeDenied { op: "open lock", found: seen, required: needed }
                    if seen == found && needed == required),
                "mode {found:o} against {required:o} was read as {refined:?}"
            );
        }
    }

    /// Every refusal the mode reading must leave alone: an entry that carries
    /// the required bits was refused for some other reason, a non-regular node
    /// and an absent entry carry no mode evidence, and a refusal that is not
    /// permission-denied is already classified.
    #[test]
    fn only_a_permission_denied_regular_entry_short_of_the_bits_is_reread() {
        let unrefined = [
            refine_open_refusal(
                permission_denied("open lock"),
                observed(NodeKind::Regular, 0o600),
                0o600,
            ),
            refine_open_refusal(
                permission_denied("open lock"),
                observed(NodeKind::Regular, 0o644),
                0o600,
            ),
            refine_open_refusal(
                permission_denied("open lock"),
                observed(NodeKind::Other, 0o000),
                0o600,
            ),
            refine_open_refusal(permission_denied("open lock"), None, 0o600),
            refine_open_refusal(
                CustodyError::NotFound { op: "open lock" },
                observed(NodeKind::Regular, 0o000),
                0o600,
            ),
        ];
        for refusal in unrefined {
            assert!(
                !matches!(refusal, CustodyError::ModeDenied { .. }),
                "{refusal:?} was reread as a mode refusal"
            );
        }
    }
}
