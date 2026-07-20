//! Borrowed root-relative overlay input, its closed refusal vocabulary, and the one
//! bounded index/settlement owner.
//!
//! An [`OverlayEntry`] borrows one root-relative key and its replacement bytes; an
//! [`OverlaySnapshot`] validates a borrowed slice under the fixed production bounds,
//! rejects lexically non-canonical keys, and builds exactly one checked
//! `Vec<OverlayIndexRow>` sorted in unstable exact-key-then-original order so that
//! duplicate detection, membership lookup, and disposition are `O(n log n)`. Physical
//! membership is decided only during capture, by the post-admission owner: it accepts
//! an exact member, marks a fixed-role key wrong, and settles the lowest-original
//! unmatched entry after a successful pure capture.

use std::cmp::Ordering;
use std::fmt;

use marrow_project::FileIdentity;

use crate::failure::CaptureFailure;
use crate::limits::AdapterLimits;

/// One borrowed root-relative source overlay entry: a key and its replacement bytes.
/// It has no `Debug` implementation, so formatting can never copy or expose its
/// borrowed body bytes.
#[derive(Clone, Copy)]
pub struct OverlayEntry<'a> {
    relative_path: &'a str,
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

/// The exact zero-based position of an entry in the consumer's original input slice.
/// It carries no capability: a copied index may appear in a caller-fabricated
/// failure, so a producer proves index provenance separately. No public or unchecked
/// conversion creates one from a `usize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OverlayEntryIndex(usize);

impl OverlayEntryIndex {
    /// Brand one exact original-slice position. Crate-private: only the constructor
    /// mints one while enumerating the borrowed input.
    const fn new(index: usize) -> Self {
        Self(index)
    }

    /// Recover the exact zero-based position in the caller's original slice.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// The settlement disposition of one indexed overlay entry.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OverlayDisposition {
    Pending,
    Accepted,
    WrongRole,
    Nonmember,
}

/// One indexed overlay entry: its exact original position, its borrowed entry, and
/// its mutable disposition. Never a second map.
struct OverlayIndexRow<'a> {
    original: OverlayEntryIndex,
    entry: &'a OverlayEntry<'a>,
    disposition: OverlayDisposition,
}

/// An opaque borrowed overlay snapshot. Its redacted `Debug` reports only the entry
/// count, so formatting cannot traverse the borrowed entries or expose the bodies.
pub struct OverlaySnapshot<'a> {
    index: Vec<OverlayIndexRow<'a>>,
}

impl fmt::Debug for OverlaySnapshot<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlaySnapshot")
            .field("entries", &self.index.len())
            .finish()
    }
}

impl<'a> OverlaySnapshot<'a> {
    /// The empty overlay: no entry replaces any disk body. Allocation-free and
    /// infallible, and the only overlay every current CLI capture supplies.
    #[must_use]
    pub const fn empty() -> Self {
        Self { index: Vec::new() }
    }

    /// Validate and index borrowed overlay entries under the fixed production bounds.
    ///
    /// # Errors
    ///
    /// Returns the first bounded, lexical, allocation, or duplicate refusal in the
    /// constructor's fixed precedence.
    pub fn try_new(entries: &'a [OverlayEntry<'a>]) -> Result<Self, OverlayFailure> {
        Self::try_new_with_limits(entries, &AdapterLimits::DEFAULT)
    }

    /// The limit-parameterized constructor that drives small-bound owner KATs.
    pub(crate) fn try_new_with_limits(
        entries: &'a [OverlayEntry<'a>],
        limits: &AdapterLimits,
    ) -> Result<Self, OverlayFailure> {
        if entries.len() > limits.overlay_entries {
            return Err(bound(
                OverlayBound::Entries,
                limits.overlay_entries,
                entries.len(),
                None,
            ));
        }

        let mut total_bytes = 0usize;
        for (position, entry) in entries.iter().enumerate() {
            let original = OverlayEntryIndex::new(position);
            if entry.relative_path.len() > limits.overlay_key_bytes {
                return Err(bound(
                    OverlayBound::KeyBytes,
                    limits.overlay_key_bytes,
                    entry.relative_path.len(),
                    Some(original),
                ));
            }
            if entry.bytes.len() > limits.overlay_file_bytes {
                return Err(bound(
                    OverlayBound::FileBytes,
                    limits.overlay_file_bytes,
                    entry.bytes.len(),
                    Some(original),
                ));
            }
            total_bytes = total_bytes.checked_add(entry.bytes.len()).ok_or_else(|| {
                bound(
                    OverlayBound::TotalBytes,
                    limits.overlay_total_bytes,
                    usize::MAX,
                    Some(original),
                )
            })?;
            if total_bytes > limits.overlay_total_bytes {
                return Err(bound(
                    OverlayBound::TotalBytes,
                    limits.overlay_total_bytes,
                    total_bytes,
                    Some(original),
                ));
            }
        }

        for (position, entry) in entries.iter().enumerate() {
            if !is_canonical_relative_path(entry.relative_path) {
                return Err(OverlayFailure::new(OverlayReason::Noncanonical {
                    entry: OverlayEntryIndex::new(position),
                }));
            }
        }

        let mut index = Vec::new();
        reserve_index(&mut index, entries.len())?;
        index.extend(
            entries
                .iter()
                .enumerate()
                .map(|(position, entry)| OverlayIndexRow {
                    original: OverlayEntryIndex::new(position),
                    entry,
                    disposition: OverlayDisposition::Pending,
                }),
        );
        index.sort_unstable_by(compare_rows);

        if let Some((first, second)) = first_duplicate(&index) {
            return Err(OverlayFailure::new(OverlayReason::Duplicate {
                first,
                second,
            }));
        }

        Ok(Self { index })
    }

    /// Accept the exact member whose key equals the admitted `identity` spelling,
    /// materializing its overlay body once. Returns `None` when no entry matches.
    pub(crate) fn accept_source(
        &mut self,
        identity: &FileIdentity,
    ) -> Result<Option<Vec<u8>>, CaptureFailure> {
        let Some(position) = self.find(identity.as_str()) else {
            return Ok(None);
        };
        let row = &mut self.index[position];
        row.disposition = OverlayDisposition::Accepted;
        let bytes = materialize_body(row.original, row.entry.bytes)?;
        Ok(Some(bytes))
    }

    /// Mark any entry whose key equals `relative_path` as naming a physical role that
    /// cannot carry source bytes.
    pub(crate) fn mark_wrong_role(&mut self, relative_path: &str) {
        if let Some(position) = self.find(relative_path) {
            self.index[position].disposition = OverlayDisposition::WrongRole;
        }
    }

    /// Settle every remaining entry after successful pure capture: pending becomes
    /// nonmember, then the lowest-original wrong-role or nonmember entry refuses.
    pub(crate) fn settle(mut self) -> Result<(), OverlayFailure> {
        for row in &mut self.index {
            if row.disposition == OverlayDisposition::Pending {
                row.disposition = OverlayDisposition::Nonmember;
            }
        }
        self.index
            .iter()
            .filter_map(|row| match row.disposition {
                OverlayDisposition::WrongRole => Some((
                    row.original,
                    OverlayReason::WrongRole {
                        entry: row.original,
                    },
                )),
                OverlayDisposition::Nonmember => Some((
                    row.original,
                    OverlayReason::Nonmember {
                        entry: row.original,
                    },
                )),
                OverlayDisposition::Pending | OverlayDisposition::Accepted => None,
            })
            .min_by_key(|(original, _)| *original)
            .map_or(Ok(()), |(_, reason)| Err(OverlayFailure::new(reason)))
    }

    fn find(&self, relative_path: &str) -> Option<usize> {
        self.index
            .binary_search_by(|row| row.entry.relative_path.cmp(relative_path))
            .ok()
    }
}

fn compare_rows(left: &OverlayIndexRow<'_>, right: &OverlayIndexRow<'_>) -> Ordering {
    left.entry
        .relative_path
        .cmp(right.entry.relative_path)
        .then_with(|| left.original.cmp(&right.original))
}

fn first_duplicate(
    index: &[OverlayIndexRow<'_>],
) -> Option<(OverlayEntryIndex, OverlayEntryIndex)> {
    index.windows(2).find_map(|pair| {
        (pair[0].entry.relative_path == pair[1].entry.relative_path)
            .then_some((pair[0].original, pair[1].original))
    })
}

/// Whether a key is a canonical consumer-neutral root-relative spelling: nonempty,
/// no root/prefix/empty/`.`/`..` component, no trailing separator, backslash, control,
/// or drive prefix. Case and Unicode-normalization variants are not excluded.
fn is_canonical_relative_path(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.chars().any(|character| character.is_ascii_control())
    {
        return false;
    }
    let mut components = path.split('/');
    let Some(first) = components.next() else {
        return false;
    };
    if is_drive_prefix(first) || matches!(first, "" | "." | "..") {
        return false;
    }
    components.all(|component| !matches!(component, "" | "." | ".."))
}

fn is_drive_prefix(component: &str) -> bool {
    let bytes = component.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn reserve_index<'a>(
    index: &mut Vec<OverlayIndexRow<'a>>,
    additional: usize,
) -> Result<(), OverlayFailure> {
    index
        .try_reserve_exact(additional)
        .map_err(|_| OverlayFailure::new(OverlayReason::Allocation { entry: None }))
}

fn materialize_body(original: OverlayEntryIndex, bytes: &[u8]) -> Result<Vec<u8>, CaptureFailure> {
    let mut owned = Vec::new();
    owned.try_reserve_exact(bytes.len()).map_err(|_| {
        CaptureFailure::from_overlay_input(OverlayFailure::new(OverlayReason::Allocation {
            entry: Some(original),
        }))
    })?;
    owned.extend_from_slice(bytes);
    Ok(owned)
}

fn bound(
    bound: OverlayBound,
    limit: usize,
    actual: usize,
    entry: Option<OverlayEntryIndex>,
) -> OverlayFailure {
    OverlayFailure::new(OverlayReason::Bound {
        bound,
        limit,
        actual,
        entry,
    })
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
