//! Private ordered-byte backend contract.

/// An error from the typed store or its private ordered-byte engines.
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
            Self::PermissionDenied { .. } => "store.permission_denied",
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValuePrefix {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

pub(crate) trait Backend {
    fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError>;
    fn read_prefix(&self, key: &[u8], limit: usize) -> Result<Option<ValuePrefix>, StoreError>;
    fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError>;
    fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError>;
    /// Fail closed with the operation's [`StoreError::ReadOnly`] when the handle
    /// cannot accept writes, so a caller can reject an unwritable mutation before
    /// opening a transaction bracket around it.
    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError>;
    fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError>;
    fn scan_after(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError>;
    fn scan_between(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        limit: usize,
    ) -> Result<ScanPage, StoreError>;
    fn scan_between_after(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError>;
    /// Return at most `limit` entries under `prefix` in reverse key order,
    /// strictly before `cursor`.
    fn scan_before(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError>;
    fn scan_between_before(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError>;
    fn begin(&mut self) -> Result<(), StoreError>;
    fn commit(&mut self) -> Result<(), StoreError>;
    fn rollback(&mut self) -> Result<(), StoreError>;
    fn transaction_depth(&self) -> usize;
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

#[cfg(test)]
pub(crate) mod counting {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::{Backend, ScanPage, StoreError, ValuePrefix};

    #[derive(Default)]
    struct CountCells {
        scans: Cell<usize>,
        scan_afters: Cell<usize>,
        scan_befores: Cell<usize>,
        entries_returned: Cell<usize>,
        bytes_moved: Cell<usize>,
        commits: Cell<usize>,
        fsyncs: Cell<usize>,
    }

    /// Shared operation counters for the private backend cost oracle.
    #[derive(Clone, Default)]
    pub(crate) struct BackendCounts {
        cells: Rc<CountCells>,
    }

    impl BackendCounts {
        pub(crate) fn reset(&self) {
            self.cells.scans.set(0);
            self.cells.scan_afters.set(0);
            self.cells.scan_befores.set(0);
            self.cells.entries_returned.set(0);
            self.cells.bytes_moved.set(0);
            self.cells.commits.set(0);
            self.cells.fsyncs.set(0);
        }

        pub(crate) fn total_scans(&self) -> usize {
            self.cells.scans.get() + self.cells.scan_afters.get() + self.cells.scan_befores.get()
        }

        pub(crate) fn entries_returned(&self) -> usize {
            self.cells.entries_returned.get()
        }

        pub(crate) fn bytes_moved(&self) -> usize {
            self.cells.bytes_moved.get()
        }

        pub(crate) fn commit_count(&self) -> usize {
            self.cells.commits.get()
        }

        pub(crate) fn fsync_count(&self) -> usize {
            self.cells.fsyncs.get()
        }

        fn add_bytes(&self, bytes: usize) {
            self.cells
                .bytes_moved
                .set(self.cells.bytes_moved.get().saturating_add(bytes));
        }

        fn count_page(&self, page: &ScanPage) {
            self.cells
                .entries_returned
                .set(self.cells.entries_returned.get() + page.entries.len());
            let bytes = page
                .entries
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum();
            self.add_bytes(bytes);
        }
    }

    /// Backend decorator used by store conformance tests to assert operation shape.
    pub(crate) struct CountingBackend<B> {
        inner: B,
        counts: BackendCounts,
        fsyncs_per_commit: usize,
    }

    impl<B> CountingBackend<B> {
        pub(crate) fn new(inner: B, counts: BackendCounts) -> Self {
            Self {
                inner,
                counts,
                fsyncs_per_commit: 0,
            }
        }

        pub(crate) fn with_fsyncs_per_commit(
            inner: B,
            counts: BackendCounts,
            fsyncs_per_commit: usize,
        ) -> Self {
            Self {
                inner,
                counts,
                fsyncs_per_commit,
            }
        }
    }

    impl<B: Backend> Backend for CountingBackend<B> {
        fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
            let value = self.inner.read(key)?;
            self.counts
                .add_bytes(key.len() + value.as_ref().map_or(0, Vec::len));
            Ok(value)
        }

        fn read_prefix(&self, key: &[u8], limit: usize) -> Result<Option<ValuePrefix>, StoreError> {
            let value = self.inner.read_prefix(key, limit)?;
            self.counts
                .add_bytes(key.len() + value.as_ref().map_or(0, |value| value.bytes.len()));
            Ok(value)
        }

        fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
            self.inner.require_write_access(op)
        }

        fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
            self.counts.add_bytes(key.len() + value.len());
            self.inner.write(key, value)
        }

        fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError> {
            self.counts.add_bytes(prefix.len());
            self.inner.delete(prefix)
        }

        fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
            self.cells().scans.set(self.cells().scans.get() + 1);
            let page = self.inner.scan(prefix, limit)?;
            self.counts.count_page(&page);
            Ok(page)
        }

        fn scan_after(
            &self,
            prefix: &[u8],
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.cells()
                .scan_afters
                .set(self.cells().scan_afters.get() + 1);
            self.counts.add_bytes(cursor.len());
            let page = self.inner.scan_after(prefix, cursor, limit)?;
            self.counts.count_page(&page);
            Ok(page)
        }

        fn scan_before(
            &self,
            prefix: &[u8],
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.cells()
                .scan_befores
                .set(self.cells().scan_befores.get() + 1);
            self.counts.add_bytes(cursor.len());
            let page = self.inner.scan_before(prefix, cursor, limit)?;
            self.counts.count_page(&page);
            Ok(page)
        }

        fn scan_between(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.cells().scans.set(self.cells().scans.get() + 1);
            self.counts
                .add_bytes(lower.map_or(0, <[u8]>::len) + upper.map_or(0, <[u8]>::len));
            let page = self.inner.scan_between(prefix, lower, upper, limit)?;
            self.counts.count_page(&page);
            Ok(page)
        }

        fn scan_between_after(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.cells()
                .scan_afters
                .set(self.cells().scan_afters.get() + 1);
            self.counts.add_bytes(
                cursor.len() + lower.map_or(0, <[u8]>::len) + upper.map_or(0, <[u8]>::len),
            );
            let page = self
                .inner
                .scan_between_after(prefix, lower, upper, cursor, limit)?;
            self.counts.count_page(&page);
            Ok(page)
        }

        fn scan_between_before(
            &self,
            prefix: &[u8],
            lower: Option<&[u8]>,
            upper: Option<&[u8]>,
            cursor: &[u8],
            limit: usize,
        ) -> Result<ScanPage, StoreError> {
            self.cells()
                .scan_befores
                .set(self.cells().scan_befores.get() + 1);
            self.counts.add_bytes(
                cursor.len() + lower.map_or(0, <[u8]>::len) + upper.map_or(0, <[u8]>::len),
            );
            let page = self
                .inner
                .scan_between_before(prefix, lower, upper, cursor, limit)?;
            self.counts.count_page(&page);
            Ok(page)
        }

        fn begin(&mut self) -> Result<(), StoreError> {
            self.inner.begin()
        }

        fn commit(&mut self) -> Result<(), StoreError> {
            self.cells().commits.set(self.cells().commits.get() + 1);
            self.cells()
                .fsyncs
                .set(self.cells().fsyncs.get() + self.fsyncs_per_commit);
            self.inner.commit()
        }

        fn rollback(&mut self) -> Result<(), StoreError> {
            self.inner.rollback()
        }

        fn transaction_depth(&self) -> usize {
            self.inner.transaction_depth()
        }

        fn begin_snapshot(&mut self) -> Result<(), StoreError> {
            self.inner.begin_snapshot()
        }

        fn end_snapshot(&mut self) {
            self.inner.end_snapshot();
        }
    }

    impl<B> CountingBackend<B> {
        fn cells(&self) -> &CountCells {
            &self.counts.cells
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Backend;
    use super::counting::{BackendCounts, CountingBackend};
    use crate::mem::MemStore;

    #[test]
    fn counting_backend_tracks_scan_bytes_commit_and_fsync_counts() {
        let counts = BackendCounts::default();
        let mut backend =
            CountingBackend::with_fsyncs_per_commit(MemStore::default(), counts.clone(), 1);

        backend.begin().expect("begin");
        backend
            .write(b"key", b"value".to_vec())
            .expect("write through decorator");
        let page = backend.scan(b"k", 10).expect("scan through decorator");
        assert_eq!(page.entries.len(), 1);
        backend.commit().expect("commit through decorator");

        assert_eq!(counts.total_scans(), 1);
        assert_eq!(counts.entries_returned(), 1);
        assert_eq!(counts.commit_count(), 1);
        assert_eq!(counts.fsync_count(), 1);
        assert!(
            counts.bytes_moved() >= b"key".len() + b"value".len(),
            "bytes moved should count scanned and written bytes"
        );

        counts.reset();
        assert_eq!(counts.total_scans(), 0);
        assert_eq!(counts.commit_count(), 0);
        assert_eq!(counts.fsync_count(), 0);
        assert_eq!(counts.bytes_moved(), 0);
    }
}
