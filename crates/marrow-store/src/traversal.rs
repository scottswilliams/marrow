//! The post-range traversal algorithm, shared by every backend.
//!
//! A backend's only real difference is where its ordered (path, value) pairs come
//! from — a `BTreeMap::range` for the in-memory store, a redb table range for the
//! persistent one. Everything done with those pairs afterward (bounding to a
//! prefix, stripping the prefix to find the next segment, collapsing descendants
//! to their distinct immediate children, collecting distinct roots, reading the
//! highest integer key off a band) is identical regardless of the source.
//!
//! Backends reuse this logic in two shapes. When a native range can yield borrowed
//! pairs through an ordinary iterator, the free functions consume an [`Entries`]
//! stream. When the storage cursor owns each row guard, the backend drives one row
//! at a time through small state objects such as [`ChildCollapse`],
//! [`DescendantProbe`], and [`ScanAccumulator`].
//!
//! Both forms keep the semantic decisions here: subtree bounds, immediate-child
//! decoding and collapse, descendant detection, scan truncation, root collapse,
//! and integer-key decoding. The walk borrows each key and value only for the step
//! that handles it and copies out only what it keeps.

use crate::backend::{Presence, ScanPage, StoreError};
use crate::path::{
    ChildSegment, SavedKey, decode_child_segment, decode_key_value, root_name, segment_len,
};

/// An ordered stream of stored (encoded path, encoded value) pairs, each fallible.
/// A backend produces this by adapting its native range; the traversal functions
/// consume it once, in order.
pub(crate) trait Entries<'a>:
    Iterator<Item = Result<(&'a [u8], &'a [u8]), StoreError>>
{
}

impl<'a, I> Entries<'a> for I where I: Iterator<Item = Result<(&'a [u8], &'a [u8]), StoreError>> {}

pub(crate) enum ChildStep {
    Done,
    Skip,
    Child(ChildSegment),
}

pub(crate) enum NeighborStep {
    Done,
    Skip,
    Child(ChildSegment),
}

pub(crate) enum DescendantStep {
    Done,
    Skip,
    Found,
}

pub(crate) enum ScanStep {
    Done,
    Continue,
}

pub(crate) struct ChildCollapse<'a> {
    path: &'a [u8],
    last: Option<Vec<u8>>,
}

impl<'a> ChildCollapse<'a> {
    pub(crate) fn new(path: &'a [u8]) -> Self {
        Self { path, last: None }
    }

    pub(crate) fn step(&mut self, key: &[u8]) -> Result<ChildStep, StoreError> {
        if !key.starts_with(self.path) {
            return Ok(ChildStep::Done);
        }
        if key.len() <= self.path.len() {
            return Ok(ChildStep::Skip);
        }
        let rest = &key[self.path.len()..];
        let len = segment_len(rest).ok_or_else(|| corrupt(key))?;
        let segment = &rest[..len];
        if self.last.as_deref() == Some(segment) {
            return Ok(ChildStep::Skip);
        }
        let child = decode_child_segment(segment).ok_or_else(|| corrupt(key))?;
        self.last = Some(segment.to_vec());
        Ok(ChildStep::Child(child))
    }
}

pub(crate) struct NeighborSeek<'a> {
    parent_prefix: &'a [u8],
    bound: &'a [u8],
}

impl<'a> NeighborSeek<'a> {
    pub(crate) fn new(parent_prefix: &'a [u8], bound: &'a [u8]) -> Self {
        Self {
            parent_prefix,
            bound,
        }
    }

    pub(crate) fn step(&self, key: &[u8]) -> Result<NeighborStep, StoreError> {
        if !key.starts_with(self.parent_prefix) {
            return Ok(NeighborStep::Done);
        }
        if key.len() <= self.parent_prefix.len() {
            return Ok(NeighborStep::Skip);
        }
        let rest = &key[self.parent_prefix.len()..];
        let len = segment_len(rest).ok_or_else(|| corrupt(key))?;
        let segment = &rest[..len];
        if segment == self.bound {
            return Ok(NeighborStep::Skip);
        }
        match decode_child_segment(segment).ok_or_else(|| corrupt(key))? {
            ChildSegment::Name(_) => Ok(NeighborStep::Skip),
            key @ ChildSegment::Key(_) => Ok(NeighborStep::Child(key)),
        }
    }
}

pub(crate) struct DescendantProbe<'a> {
    path: &'a [u8],
}

impl<'a> DescendantProbe<'a> {
    pub(crate) fn new(path: &'a [u8]) -> Self {
        Self { path }
    }

    pub(crate) fn step(&self, key: &[u8]) -> DescendantStep {
        if !key.starts_with(self.path) {
            return DescendantStep::Done;
        }
        if key.len() > self.path.len() {
            return DescendantStep::Found;
        }
        DescendantStep::Skip
    }
}

pub(crate) struct ScanAccumulator<'a> {
    path: &'a [u8],
    limit: usize,
    page: ScanPage,
}

impl<'a> ScanAccumulator<'a> {
    pub(crate) fn new(path: &'a [u8], limit: usize) -> Self {
        Self {
            path,
            limit,
            page: ScanPage::default(),
        }
    }

    pub(crate) fn step(&mut self, key: &[u8], value: &[u8]) -> ScanStep {
        if !key.starts_with(self.path) {
            return ScanStep::Done;
        }
        if self.page.entries.len() == self.limit {
            self.page.truncated = true;
            return ScanStep::Done;
        }
        self.page.entries.push((key.to_vec(), value.to_vec()));
        ScanStep::Continue
    }

    pub(crate) fn into_page(self) -> ScanPage {
        self.page
    }
}

/// Build a [`StoreError::CorruptPath`] for a stored key that failed to decode.
fn corrupt(key: &[u8]) -> StoreError {
    StoreError::CorruptPath { path: key.to_vec() }
}

/// The distinct immediate children directly below `path`, in the stream's order,
/// from a range that begins at `path`. Descendants sharing an immediate child
/// collapse to that one child (the range is ordered, so they arrive
/// consecutively). The collapse is direction-symmetric: a forward stream yields
/// the children in ascending Marrow order, a reversed one yields them descending
/// with the same collapsing (a child's first-seen descendant is simply its
/// highest rather than its lowest, still mapping to the one child segment). The
/// range is bounded here: it ends at the first key that no longer starts with
/// `path`. Returns [`StoreError::CorruptPath`] if a descendant key cannot be
/// decoded.
pub(crate) fn child_keys<'a>(
    entries: impl Entries<'a>,
    path: &[u8],
) -> Result<Vec<ChildSegment>, StoreError> {
    let mut children = Vec::new();
    let mut collapse = ChildCollapse::new(path);
    for entry in entries {
        let (key, _) = entry?;
        match collapse.step(key)? {
            ChildStep::Done => break,
            ChildStep::Skip => {}
            ChildStep::Child(child) => children.push(child),
        }
    }
    Ok(children)
}

/// Count the distinct immediate children directly below `path`, from a range
/// that begins at `path`. This is the same ordered walk and consecutive-child
/// collapse as [`child_keys`], but it tallies instead of building a child list.
pub(crate) fn child_count<'a>(entries: impl Entries<'a>, path: &[u8]) -> Result<usize, StoreError> {
    let mut count = 0;
    let mut collapse = ChildCollapse::new(path);
    for entry in entries {
        let (key, _) = entry?;
        match collapse.step(key)? {
            ChildStep::Done => break,
            ChildStep::Skip => {}
            ChildStep::Child(_) => count += 1,
        }
    }
    Ok(count)
}

/// The single immediate *key* child of `parent_prefix` adjacent to the child
/// segment `bound`, in the stream's direction, or `None` when `bound` is the edge
/// key child (no key neighbor that way). `bound` is one encoded child segment (kind
/// tag + key); pass `b""` to seek the parent's edge key child instead — the first
/// forward, the last reversed — since an empty bound never equals a real
/// kind-tagged segment, so no row is skipped and the first key child is returned
/// (the bare-layer `next`/`prev` entry point). The backend supplies a range bounded
/// to `parent_prefix`'s subtree that begins at `parent_prefix ++ bound`: forward for
/// the next sibling, reversed for the previous.
///
/// `next`/`prev` navigate one key level — record keys under a keyed root, index
/// keys under a keyed layer — so this skips any non-key child (a named member such
/// as a declared `index`, field, or child layer), mirroring the key-only filter the
/// forward enumeration applies. A named child sorts after the key children, so
/// without this skip a `next` past the last record would land on a trailing index
/// name; instead it walks past every named row and reports true exhaustion as
/// `None`, the catchable edge the caller turns into `run.absent_element`. The walk
/// also skips every row whose first post-prefix segment equals `bound` — those are
/// `bound`'s own entry and its descendants, which arrive consecutively (the same
/// consecutive-equal collapse [`child_keys`] uses) — so a child with its own deep
/// subtree is stepped over in one go. The range is lazy, so a backend stops at the
/// first key child past `bound` rather than materializing the whole subtree.
pub(crate) fn neighbor_child<'a>(
    entries: impl Entries<'a>,
    parent_prefix: &[u8],
    bound: &[u8],
) -> Result<Option<ChildSegment>, StoreError> {
    let seek = NeighborSeek::new(parent_prefix, bound);
    for entry in entries {
        let (key, _) = entry?;
        match seek.step(key)? {
            NeighborStep::Done => break,
            NeighborStep::Skip => {}
            NeighborStep::Child(child) => return Ok(Some(child)),
        }
    }
    Ok(None)
}

/// Whether any stored key lies strictly below `path`, from a range that begins at
/// `path`. An encoded ancestor is a byte-prefix of its descendants and segment
/// terminators keep unrelated paths from sharing the prefix, so the first prefixed
/// key longer than `path` is a descendant. Combined with whether `path` itself
/// holds a value, this gives the four [`Presence`] states.
pub(crate) fn has_descendants<'a>(
    entries: impl Entries<'a>,
    path: &[u8],
) -> Result<bool, StoreError> {
    let probe = DescendantProbe::new(path);
    for entry in entries {
        let (key, _) = entry?;
        match probe.step(key) {
            DescendantStep::Done => break,
            DescendantStep::Skip => {}
            DescendantStep::Found => return Ok(true),
        }
    }
    Ok(false)
}

pub(crate) fn presence_from_parts(has_value: bool, has_descendants: bool) -> Presence {
    match (has_value, has_descendants) {
        (false, false) => Presence::Absent,
        (true, false) => Presence::ValueOnly,
        (false, true) => Presence::ChildrenOnly,
        (true, true) => Presence::ValueAndChildren,
    }
}

/// Combine whether `path` holds a value with whether it has descendants into the
/// four-way presence state. A backend supplies the cheap value check and the range
/// for the descendant walk.
pub(crate) fn presence<'a>(
    has_value: bool,
    entries: impl Entries<'a>,
    path: &[u8],
) -> Result<Presence, StoreError> {
    Ok(presence_from_parts(
        has_value,
        has_descendants(entries, path)?,
    ))
}

/// Up to `limit` (encoded path, value) pairs from a range that begins at `path`,
/// in Marrow order, including the value at `path` itself when present. The range
/// is bounded here at the first key that no longer starts with `path`. `truncated`
/// is set when more remained past the limit. The pairs are copied out because a
/// page outlives the range that produced it.
pub(crate) fn scan<'a>(
    entries: impl Entries<'a>,
    path: &[u8],
    limit: usize,
) -> Result<ScanPage, StoreError> {
    let mut scan = ScanAccumulator::new(path, limit);
    for entry in entries {
        let (key, value) = entry?;
        match scan.step(key, value) {
            ScanStep::Done => break,
            ScanStep::Continue => {}
        }
    }
    Ok(scan.into_page())
}

/// The distinct saved root names, in Marrow order, from a range over the whole
/// store. Keys under one root are consecutive, so consecutive equal root names
/// collapse to one. Returns [`StoreError::CorruptPath`] if a stored key does not
/// begin with a valid root segment.
pub(crate) fn roots<'a>(entries: impl Entries<'a>) -> Result<Vec<String>, StoreError> {
    let mut roots: Vec<String> = Vec::new();
    for entry in entries {
        let (key, _) = entry?;
        let name = root_name(key).ok_or_else(|| corrupt(key))?;
        if roots.last() != Some(&name) {
            roots.push(name);
        }
    }
    Ok(roots)
}

/// The highest integer key in an integer-key band, decoded from `band_last`: the
/// last (highest) entry of the band, or `None` when the band is empty. A backend
/// ranges over the band and hands its final key here; the key after `prefix` is
/// the kind tag (one byte) then the integer key encoding, so the value decodes
/// from `prefix.len() + 1`. Returns [`StoreError::CorruptPath`] if that key is not
/// an integer key.
pub(crate) fn max_int_key(
    band_last: Option<Result<&[u8], StoreError>>,
    prefix: &[u8],
) -> Result<Option<i64>, StoreError> {
    let Some(key) = band_last else {
        return Ok(None);
    };
    let key = key?;
    match decode_key_value(key.get(prefix.len() + 1..).unwrap_or(&[])) {
        Some((SavedKey::Int(value), _)) => Ok(Some(value)),
        _ => Err(corrupt(key)),
    }
}
