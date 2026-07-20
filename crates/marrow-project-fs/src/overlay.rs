//! Borrowed root-relative overlay input and its closed refusal vocabulary.
//!
//! An [`OverlayEntry`] borrows one root-relative key and its replacement bytes;
//! an [`OverlaySnapshot`] borrows a slice of them. Both are consumer-neutral: no
//! URI, version, module name, or language-semantic type appears here. The bounds
//! checking, canonical-path validation, and membership settlement that produce an
//! [`OverlayFailure`] are introduced in the capture baseline; this module fixes
//! the borrowed input types and the closed refusal vocabulary first.

use std::fmt;

/// One borrowed root-relative source overlay entry: a key and its replacement
/// bytes. It has no `Debug` implementation, so formatting can never copy or
/// expose its borrowed body bytes.
#[derive(Clone, Copy)]
pub struct OverlayEntry<'a> {
    // Read by the overlay constructor and membership settlement introduced in the
    // capture baseline; the borrowed shape is fixed here first.
    #[allow(dead_code)]
    relative_path: &'a str,
    #[allow(dead_code)]
    bytes: &'a [u8],
}

impl<'a> OverlayEntry<'a> {
    /// Borrow one root-relative key and its replacement bytes.
    #[must_use]
    pub const fn new(relative_path: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            relative_path,
            bytes,
        }
    }
}

/// The exact zero-based position of an entry in the consumer's original input
/// slice. It carries no capability: a copied index may appear in a
/// caller-fabricated failure, so a producer proves index provenance separately.
/// No public or unchecked conversion creates one from a `usize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OverlayEntryIndex(usize);

impl OverlayEntryIndex {
    /// Recover the exact zero-based position in the caller's original slice.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// An opaque borrowed overlay snapshot. Its redacted `Debug` reports only the
/// entry count, so formatting cannot traverse the borrowed entries or expose the
/// replacement bodies. Bounds checking and construction land in the capture
/// baseline.
pub struct OverlaySnapshot<'a> {
    entries: &'a [OverlayEntry<'a>],
}

impl fmt::Debug for OverlaySnapshot<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlaySnapshot")
            .field("entries", &self.entries.len())
            .finish()
    }
}

/// A bounded overlay resource enforced before physical membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayBound {
    /// Number of raw borrowed entries.
    Entries,
    /// Bytes in one root-relative key.
    KeyBytes,
    /// Bytes in one replacement body.
    FileBytes,
    /// Bytes across all replacement bodies.
    TotalBytes,
}

/// Why an overlay snapshot or membership was refused. Per-entry evidence uses the
/// zero-based original-slice [`OverlayEntryIndex`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayReason {
    /// A raw overlay resource exceeded its fixed limit.
    Bound {
        /// Resource that exceeded its limit.
        bound: OverlayBound,
        /// Inclusive limit.
        limit: usize,
        /// Observed amount at refusal.
        actual: usize,
        /// Original offender, or `None` for a whole-slice failure.
        entry: Option<OverlayEntryIndex>,
    },
    /// Checked adapter-owned storage reservation failed.
    Allocation {
        /// Body offender, or `None` for index allocation.
        entry: Option<OverlayEntryIndex>,
    },
    /// Two raw entries carry the same exact key.
    Duplicate {
        /// First original occurrence.
        first: OverlayEntryIndex,
        /// Second original occurrence.
        second: OverlayEntryIndex,
    },
    /// A key is not a canonical consumer-neutral root-relative spelling.
    Noncanonical {
        /// Original offender.
        entry: OverlayEntryIndex,
    },
    /// No physically admitted source has the exact key.
    Nonmember {
        /// Original offender.
        entry: OverlayEntryIndex,
    },
    /// The key names a physical project role that cannot carry source bytes.
    WrongRole {
        /// Original offender.
        entry: OverlayEntryIndex,
    },
}

/// A typed overlay refusal. All evidence is publicly observable through
/// [`OverlayFailure::reason`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayFailure {
    reason: OverlayReason,
}

impl OverlayFailure {
    /// The typed refusal evidence.
    pub fn reason(&self) -> &OverlayReason {
        &self.reason
    }
}
