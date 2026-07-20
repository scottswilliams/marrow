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
    // Read by the bounds/lexical/membership settlement introduced in target
    // hardening; the borrowed shape is fixed here and the baseline constructor
    // retains the slice without inspecting it.
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
/// replacement bodies.
///
/// The baseline constructor retains the borrowed slice in O(1) without iterating,
/// validating, indexing, or allocating; the raw bounds, canonical-path checks,
/// and bounded-index construction land in target hardening. [`empty`] is
/// allocation-free and infallible.
///
/// [`empty`]: OverlaySnapshot::empty
pub struct OverlaySnapshot<'a> {
    entries: &'a [OverlayEntry<'a>],
}

impl<'a> OverlaySnapshot<'a> {
    /// The empty overlay: no entry replaces any disk body. Allocation-free and
    /// infallible, and the only overlay every current CLI capture supplies.
    #[must_use]
    pub fn empty() -> Self {
        Self { entries: &[] }
    }

    /// Retain a borrowed overlay slice. In this baseline the retention is O(1) and
    /// does not inspect the entries; the raw bounds, canonical-path validation,
    /// duplicate check, and bounded borrowed-index construction land in target
    /// hardening, so this constructor is presently infallible on the input slice.
    pub fn try_new(entries: &'a [OverlayEntry<'a>]) -> Result<Self, OverlayFailure> {
        Ok(Self { entries })
    }

    /// The number of borrowed entries.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the overlay is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
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
    /// Build a typed overlay refusal from its reason.
    pub(crate) fn new(reason: OverlayReason) -> Self {
        Self { reason }
    }

    /// The typed refusal evidence.
    pub fn reason(&self) -> &OverlayReason {
        &self.reason
    }
}
