//! The typed error the ordered-byte engines report.

use marrow_codes::Code;

/// The engine operation an error reports. Naming the operation as a variant keeps
/// the prose in one renderer and lets a caller match the step that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    /// Opening the store body, its lock, or its format stamp.
    Open,
    /// Creating a store directory and its body.
    Provision,
    /// Preparing the service state a write-capable open installs.
    ServicePreparation,
    /// Native recovery of an unclean store.
    Recovery,
    /// A read taken while recovery classifies the store.
    RecoveryRead,
    /// The engine's own integrity audit.
    Audit,
    /// Beginning a read transaction.
    BeginRead,
    /// Beginning a write transaction.
    BeginWrite,
    /// Reading one cell.
    Read,
    /// Scanning forward from a key.
    ScanAfter,
    /// Writing one cell.
    Put,
    /// Removing one cell.
    Remove,
    /// Committing a write transaction.
    Commit,
    /// `fsync` of the store directory.
    SyncParentDir,
    /// The cross-engine conformance suite.
    Conformance,
}

impl std::fmt::Display for StoreOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Open => "open",
            Self::Provision => "provision",
            Self::ServicePreparation => "service preparation",
            Self::Recovery => "recovery",
            Self::RecoveryRead => "recovery read",
            Self::Audit => "audit",
            Self::BeginRead => "begin read",
            Self::BeginWrite => "begin write",
            Self::Read => "read",
            Self::ScanAfter => "scan",
            Self::Put => "put",
            Self::Remove => "remove",
            Self::Commit => "commit",
            Self::SyncParentDir => "directory sync",
            Self::Conformance => "conformance",
        };
        f.write_str(name)
    }
}

/// A fixed representation bound a store operation exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreLimit {
    /// A key longer than the engine's key bound.
    KeyLength,
    /// A value longer than the engine's value bound.
    ValueLength,
    /// The commit-witness generation counter ran out of room.
    CommitWitnessGeneration,
}

impl std::fmt::Display for StoreLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::KeyLength => "key length",
            Self::ValueLength => "value length",
            Self::CommitWitnessGeneration => "commit witness generation",
        };
        f.write_str(name)
    }
}

/// An error from a native ordered-byte engine or the shared limits its batches
/// obey. It renders from a stable dotted [`Code`]; callers match the variant, not
/// the prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// An I/O operation on a persistent backend failed.
    Io { op: StoreOp, message: String },
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
    /// The redb engine found unclean state that its read-only opener cannot repair. A
    /// write-capable engine open may perform redb's internal log recovery; this never
    /// replays Marrow bytecode or retries an invocation. A store that cannot be opened
    /// after engine recovery surfaces [`Corruption`](Self::Corruption) instead.
    RecoveryRequired,
    /// An operation exhausted a fixed representation bound, including a key or value
    /// beyond its length limit and framing lengths, counts, or commit-ID allocation.
    LimitExceeded { limit: StoreLimit },
    /// A write-capability operation was requested through a read-only store handle.
    ReadOnly { op: StoreOp },
}

impl StoreError {
    /// The stable code a tool reports for this error.
    pub fn code(&self) -> Code {
        match self {
            Self::Io { .. } => Code::StoreIo,
            Self::PermissionDenied { .. } => Code::StorePermissionDenied,
            Self::Locked { .. } => Code::StoreLocked,
            Self::FormatVersion { .. } => Code::StoreFormatVersion,
            Self::Corruption { .. } => Code::StoreCorruption,
            Self::RecoveryRequired => Code::StoreRecoveryRequired,
            Self::LimitExceeded { .. } => Code::StoreLimit,
            Self::ReadOnly { .. } => Code::StoreReadOnly,
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
                "the redb engine found unclean state that this read-only open cannot repair; \
                 reopen the store through its normal write-capable lifecycle so redb can \
                 perform internal log recovery. This does not replay Marrow bytecode or retry \
                 an invocation; an unrecoverable store surfaces store.corruption"
            ),
            Self::LimitExceeded { limit } => write!(f, "a storage limit was exceeded: {limit}"),
            Self::ReadOnly { op } => write!(f, "cannot {op} through a read-only store handle"),
        }
    }
}

impl std::error::Error for StoreError {}
