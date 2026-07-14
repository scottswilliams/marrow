//! The typed error the ordered-byte engines report.

use marrow_codes::Code;

/// An error from a native ordered-byte engine or the shared limits its batches
/// obey. It renders from a stable dotted [`Code`]; callers match the variant, not
/// the prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// An I/O operation on a persistent backend failed.
    Io { op: &'static str, message: String },
    /// The process lacks read/write access to the store directory or file. A distinct,
    /// path-bearing state rather than a raw errno, since the fix (grant access) differs from a
    /// transient I/O fault.
    PermissionDenied { path: std::path::PathBuf },
    /// The store file is already held open by another process, either with write
    /// capability or as a read-only inspection.
    Locked { data_dir: std::path::PathBuf },
    /// The store's recorded format version is not the one this build supports.
    FormatVersion { found: u32, supported: u32 },
    /// The persistent store or a tree-cell payload is corrupt.
    Corruption { message: String },
    /// The store was not shut down cleanly, so a read-only open is refused until a
    /// write-capable open replays the interrupted commit. The replay is attempted, not
    /// guaranteed: it reports whether the store opened, and a store damaged beyond
    /// replay surfaces [`Corruption`](Self::Corruption) instead.
    RecoveryRequired,
    /// An operation exhausted a fixed representation bound, including a key or value
    /// beyond its length limit and framing lengths, counts, or commit-ID allocation.
    LimitExceeded { limit: &'static str },
    /// A write-capability operation was requested through a read-only store handle.
    ReadOnly { op: &'static str },
}

impl StoreError {
    /// The stable dotted code a tool reports for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => Code::StoreIo.as_str(),
            Self::PermissionDenied { .. } => Code::StorePermissionDenied.as_str(),
            Self::Locked { .. } => Code::StoreLocked.as_str(),
            Self::FormatVersion { .. } => Code::StoreFormatVersion.as_str(),
            Self::Corruption { .. } => Code::StoreCorruption.as_str(),
            Self::RecoveryRequired => Code::StoreRecoveryRequired.as_str(),
            Self::LimitExceeded { .. } => Code::StoreLimit.as_str(),
            Self::ReadOnly { .. } => Code::StoreReadOnly.as_str(),
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { op, message } => write!(f, "storage {op} failed: {message}"),
            Self::PermissionDenied { path } => write!(
                f,
                "cannot open the store at {}: permission denied. Check that you have read/write \
                 access to that directory",
                path.display()
            ),
            Self::Locked { data_dir } => write!(
                f,
                "the store file is held open by another process (a writer or a read-only \
                 inspection): {}. Close the other process, then retry",
                data_dir.display()
            ),
            Self::FormatVersion { found, supported } => write!(
                f,
                "store format version {found} is unsupported (this build uses {supported})"
            ),
            Self::Corruption { message } => write!(f, "the store is corrupt: {message}"),
            Self::RecoveryRequired => write!(
                f,
                "the store was not shut down cleanly and needs a write-capable recovery before \
                 a read-only open. Run `marrow data recover` so a write open can replay the \
                 interrupted commit; it reports whether the store opened, and a store damaged \
                 beyond replay surfaces store.corruption"
            ),
            Self::LimitExceeded { limit } => write!(f, "a storage limit was exceeded: {limit}"),
            Self::ReadOnly { op } => write!(f, "cannot {op} through a read-only store handle"),
        }
    }
}

impl std::error::Error for StoreError {}
