//! Private ordered-byte backend contract.

/// An error from the typed store or its private ordered-byte engines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// An I/O operation on a persistent backend failed.
    Io { op: &'static str, message: String },
    /// The store file is already held open by another writer.
    Locked { data_dir: std::path::PathBuf },
    /// The store's recorded format version is not the one this build supports.
    FormatVersion { found: u32, supported: u32 },
    /// The persistent store or a tree-cell payload is corrupt.
    Corruption { message: String },
    /// The store was not shut down cleanly, so a read-only open is refused until a
    /// write-capable run replays the interrupted commit. The replay is attempted, not
    /// guaranteed: it reports whether the data survived, and a store damaged beyond
    /// replay surfaces [`Corruption`](Self::Corruption) instead.
    RecoveryRequired,
    /// A store-owned framing field could not hold a key, value, or metadata length.
    LimitExceeded { limit: &'static str },
    /// A bounded scan cursor does not belong to the scan being resumed.
    InvalidCursor { message: String },
    /// A transaction or snapshot operation was requested in an invalid store state.
    InvalidTransaction { message: String },
    /// A write-capability operation was requested through a read-only store handle.
    ReadOnly { op: &'static str },
}

impl StoreError {
    /// A write was attempted while a read snapshot pinned the handle.
    pub(crate) fn write_while_snapshot_pinned() -> Self {
        Self::snapshot_conflict("cannot write while a read snapshot is pinned")
    }

    /// A delete was attempted while a read snapshot pinned the handle.
    pub(crate) fn delete_while_snapshot_pinned() -> Self {
        Self::snapshot_conflict("cannot delete while a read snapshot is pinned")
    }

    /// A write transaction was begun while a read snapshot pinned the handle.
    pub(crate) fn begin_while_snapshot_pinned() -> Self {
        Self::snapshot_conflict("cannot begin a write transaction while a read snapshot is pinned")
    }

    /// A read snapshot was requested while a write transaction was open.
    pub(crate) fn snapshot_while_transaction_open() -> Self {
        Self::snapshot_conflict("cannot pin a read snapshot while a write transaction is open")
    }

    /// A second read snapshot was requested on a handle that already pinned one.
    pub(crate) fn snapshot_already_pinned() -> Self {
        Self::snapshot_conflict("cannot pin a second read snapshot on the same store handle")
    }

    fn snapshot_conflict(message: &'static str) -> Self {
        Self::InvalidTransaction {
            message: message.to_string(),
        }
    }

    /// The stable dotted code a tool reports for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "store.io",
            Self::Locked { .. } => "store.locked",
            Self::FormatVersion { .. } => "store.format_version",
            Self::Corruption { .. } => "store.corruption",
            Self::RecoveryRequired => "store.recovery_required",
            Self::LimitExceeded { .. } => "store.limit",
            Self::InvalidCursor { .. } => "store.cursor",
            Self::InvalidTransaction { .. } => "store.transaction",
            Self::ReadOnly { .. } => "store.read_only",
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { op, message } => write!(f, "storage {op} failed: {message}"),
            Self::Locked { data_dir } => write!(
                f,
                "the store is already open by another process: {}",
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
                 a read-only open. Run `marrow run` so a write open can replay the interrupted \
                 commit; it reports whether the data survived, and a store damaged beyond replay \
                 surfaces store.corruption"
            ),
            Self::LimitExceeded { limit } => write!(f, "a storage limit was exceeded: {limit}"),
            Self::InvalidCursor { message } => write!(f, "storage cursor is invalid: {message}"),
            Self::InvalidTransaction { message } => {
                write!(f, "storage transaction state is invalid: {message}")
            }
            Self::ReadOnly { op } => write!(f, "cannot {op} through a read-only store handle"),
        }
    }
}

impl std::error::Error for StoreError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ScanPage {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
    pub truncated: bool,
}

pub(crate) trait Backend {
    fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError>;
    fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError>;
    fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError>;
    fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError>;
    fn scan_after(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError>;
    fn begin(&mut self) -> Result<(), StoreError>;
    fn commit(&mut self) -> Result<(), StoreError>;
    fn rollback(&mut self) -> Result<(), StoreError>;
    /// Pin a consistent read view so a multi-call traversal observes one
    /// snapshot. Reads and scans route through the pinned view until
    /// [`end_snapshot`](Backend::end_snapshot) releases it, and this handle
    /// rejects overlapping writes and write transactions while the snapshot is
    /// pinned.
    fn begin_snapshot(&mut self) -> Result<(), StoreError>;
    /// Release the pinned read view, so later reads observe the latest committed
    /// data. Idempotent, so a guard's `Drop` can call it unconditionally.
    fn end_snapshot(&mut self);
}
