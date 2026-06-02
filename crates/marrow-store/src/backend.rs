//! The saved-tree backend contract.
//!
//! Every backend serves the same ordered-tree operations over encoded saved
//! paths — read, write, delete, presence, child keys (forward and reversed),
//! ordered sibling and edge seeks, bounded scan, and roots — plus transaction
//! control modelled as a savepoint stack (`begin`/`commit`/`rollback`, with
//! nested `begin`s as savepoints). [`MemStore`](crate::mem::MemStore) is the
//! in-memory implementor; a persistent backend implements the same contract.
//!
//! One iteration invariant holds across every ordered op — `child_keys`,
//! `child_keys_rev`, `child_count`, `next_sibling`/`prev_sibling`, and
//! `first_child`/`last_child`: each visits only **stored** entries, in Marrow key
//! order, and skips holes. Deleting an entry removes it from every traversal;
//! there are no placeholder positions to step onto. A backend that merely orders
//! raw bytes inherits this for free, since the encoding makes byte order Marrow
//! order.
//!
//! Reads return owned bytes so a persistent backend can serve them from a
//! transaction guard, and every operation is fallible: a persistent store can
//! meet I/O and corruption errors the in-memory store never does.

use crate::path::ChildSegment;

/// What a saved path holds: a value, children, both, or neither. Mirrors the
/// four presence states the backend contract reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Absent,
    ValueOnly,
    ChildrenOnly,
    ValueAndChildren,
}

/// An error from the store. The in-memory store can only fail by meeting a
/// stored path it cannot decode; a persistent backend can also fail with the I/O,
/// locking, format, corruption, and limit variants. Variants carry only owned
/// data (never a backend-specific error) so the contract stays comparable.
///
/// Each variant maps to a stable dotted [`code`](StoreError::code) and renders a
/// human message through [`Display`](std::fmt::Display), so every tool above the
/// store reports storage failures the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// A stored key is not a well-formed sequence of path segments.
    CorruptPath { path: Vec<u8> },
    /// An I/O operation on a persistent backend failed.
    Io { op: &'static str, message: String },
    /// The store file is already held open by another writer.
    Locked { data_dir: std::path::PathBuf },
    /// The store's recorded format version is not the one this build supports.
    FormatVersion { found: u32, supported: u32 },
    /// The persistent store is corrupt and could not be opened or read.
    Corruption { message: String },
    /// A store-owned framing field could not hold a key, value, or metadata length.
    /// Backends enforce no key/value size limit; only Marrow framing layers produce
    /// this variant (`store.limit`).
    LimitExceeded { limit: &'static str },
    /// A bounded scan cursor does not belong to the scan being resumed.
    InvalidCursor { message: String },
    /// A write-capability operation was requested through a read-only store handle.
    ReadOnly { op: &'static str },
}

impl StoreError {
    /// The stable dotted code a tool reports for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::CorruptPath { .. } => "store.corrupt_path",
            Self::Io { .. } => "store.io",
            Self::Locked { .. } => "store.locked",
            Self::FormatVersion { .. } => "store.format_version",
            Self::Corruption { .. } => "store.corruption",
            Self::LimitExceeded { .. } => "store.limit",
            Self::InvalidCursor { .. } => "store.cursor",
            Self::ReadOnly { .. } => "store.read_only",
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CorruptPath { path } => {
                write!(f, "a stored path is malformed ({} bytes)", path.len())
            }
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
            Self::LimitExceeded { limit } => write!(f, "a storage limit was exceeded: {limit}"),
            Self::InvalidCursor { message } => write!(f, "storage cursor is invalid: {message}"),
            Self::ReadOnly { op } => write!(f, "cannot {op} through a read-only store handle"),
        }
    }
}

impl std::error::Error for StoreError {}

/// One page of a bounded scan: the entries found in Marrow order, and whether
/// more remained past the limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanPage {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
    pub truncated: bool,
}

/// The operations every Marrow saved-tree store provides over encoded paths.
pub trait Backend {
    /// The exact value at `path`, or `None` when no value is stored there.
    fn read(&self, path: &[u8]) -> Result<Option<Vec<u8>>, StoreError>;
    /// Write `value` at `path`, replacing any value already there.
    fn write(&mut self, path: &[u8], value: Vec<u8>) -> Result<(), StoreError>;
    /// Remove the value at `path` and every value below it.
    fn delete(&mut self, path: &[u8]) -> Result<(), StoreError>;
    /// Whether `path` holds a value, children, both, or neither.
    fn presence(&self, path: &[u8]) -> Result<Presence, StoreError>;
    /// The distinct immediate children directly below `path`, in Marrow order.
    fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError>;
    /// The distinct immediate children directly below `path`, in **reverse**
    /// Marrow order — the exact reverse of [`child_keys`](Self::child_keys).
    /// Backed by a double-ended range, so it is the same O(k) walk run backward,
    /// not a forward walk reversed after the fact. Like every traversal op it
    /// visits only stored entries and skips holes.
    fn child_keys_rev(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError>;

    /// The number of distinct immediate children directly below `path`, matching
    /// `child_keys(path).len()` without allocating the child list.
    fn child_count(&self, path: &[u8]) -> Result<usize, StoreError>;

    /// The immediate *key* child of `parent` that directly follows `after` in
    /// Marrow order, or `None` when `after` is the last key child (`after` has no
    /// key successor under `parent`). `after` is one encoded child segment (a kind
    /// tag and the key, as produced by [`encode_path`](crate::path::encode_path)
    /// for one record- or index-key segment). The seek is O(k) over a double-ended
    /// range from just past `after`, early-breaking at the first distinct key
    /// child, and it steps over the whole subtree of `after` (a child with its own
    /// descendants is one stop, never a grandchild). It serves `next`/`prev`, which
    /// navigate one key level, so it skips any named member (a declared index,
    /// field, or child layer) that sorts past the key children rather than landing
    /// on one. Skips gaps too: a deleted child is absent, so the nearest *stored*
    /// key successor is returned.
    fn next_sibling(&self, parent: &[u8], after: &[u8])
    -> Result<Option<ChildSegment>, StoreError>;
    /// The immediate *key* child of `parent` that directly precedes `before` in
    /// Marrow order, or `None` when `before` is the first key child. The mirror of
    /// [`next_sibling`](Self::next_sibling) over a reversed range: same O(k), early
    /// break, subtree-skipping, named-member-skipping, and gap-skipping guarantees.
    fn prev_sibling(
        &self,
        parent: &[u8],
        before: &[u8],
    ) -> Result<Option<ChildSegment>, StoreError>;
    /// The first (lowest in Marrow order) immediate *key* child of `parent`, or
    /// `None` when `parent` has no key child. The bare-layer entry point for
    /// `next`: the first stored key position under a layer. Like the sibling seeks
    /// it navigates key positions only, skipping any named member. O(k) over the
    /// subtree's forward range.
    fn first_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError>;
    /// The last (highest in Marrow order) immediate *key* child of `parent`, or
    /// `None` when `parent` has no key child. The bare-layer entry point for
    /// `prev`: the last stored key position under a layer. Skips named members,
    /// which sort after the key children, to land on the last key. O(k) over the
    /// subtree's reversed range.
    fn last_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError>;

    /// Up to `limit` (path, value) pairs in the subtree at `path`, in Marrow
    /// order, including the value at `path` itself when present.
    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError>;
    /// Up to `limit` (path, value) pairs in the subtree at `path`, strictly after
    /// `cursor`, in Marrow order.
    fn scan_after(&self, path: &[u8], cursor: &[u8], limit: usize) -> Result<ScanPage, StoreError>;
    /// The distinct saved root names, in Marrow order.
    fn roots(&self) -> Result<Vec<String>, StoreError>;

    /// The highest integer record key among the immediate children of `prefix`,
    /// or `None` when none decodes to an integer record key. Integer record keys
    /// form one contiguous numeric-ordered band, so a backend answers this from
    /// the band's last entry without materializing every child.
    fn max_int_record_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError>;

    /// The highest integer index key among the immediate children of `prefix`
    /// (the positions inside a keyed child layer), or `None` when none decodes to
    /// one. The index-key analogue of [`max_int_record_key`](Self::max_int_record_key),
    /// answered the same bounded way.
    fn max_int_index_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError>;

    /// Open a savepoint. Nested `begin`s stack; writes after it stay visible to
    /// reads (read-your-writes) until the matching `commit` or `rollback`.
    fn begin(&mut self) -> Result<(), StoreError>;
    /// Discard the innermost savepoint, keeping its writes (a normal exit). With
    /// no open savepoint this is a no-op.
    fn commit(&mut self) -> Result<(), StoreError>;
    /// Roll back to the innermost savepoint, discarding its writes. With no open
    /// savepoint this is a no-op.
    fn rollback(&mut self) -> Result<(), StoreError>;
}
