//! The post-range traversal algorithm, shared by every backend.
//!
//! A backend's only real difference is where its ordered (path, value) pairs come
//! from — a `BTreeMap::range` for the in-memory store, a redb table range for the
//! persistent one. Everything done with those pairs afterward (bounding to a
//! prefix, stripping the prefix to find the next segment, collapsing descendants
//! to their distinct immediate children, collecting distinct roots, reading the
//! highest integer key off a band) is identical regardless of the source.
//!
//! These free functions hold that shared half once. A backend adapts its native
//! range into an [`Entries`] iterator — pairs in Marrow order, each fallible so a
//! persistent store can report an I/O fault mid-walk — and calls them. A new
//! backend gets the whole traversal for free by doing the same.
//!
//! The walk borrows each key and value only for the step that handles it (it
//! copies out only what it keeps), so a backend whose range hands back borrowed
//! bytes can yield them without owning a copy per row.

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
    let mut last: Option<Vec<u8>> = None;
    for entry in entries {
        let (key, _) = entry?;
        if !key.starts_with(path) {
            break; // past the subtree
        }
        if key.len() <= path.len() {
            continue; // the path's own entry, not a child
        }
        let rest = &key[path.len()..];
        let len = segment_len(rest).ok_or_else(|| corrupt(key))?;
        let segment = &rest[..len];
        if last.as_deref() == Some(segment) {
            continue; // same immediate child as the previous descendant
        }
        last = Some(segment.to_vec());
        children.push(decode_child_segment(segment).ok_or_else(|| corrupt(key))?);
    }
    Ok(children)
}

/// The single immediate child of `parent_prefix` adjacent to the child segment
/// `bound`, in the stream's direction, or `None` when `bound` is the edge child
/// (no neighbor that way). `bound` is one encoded child segment (kind tag + key).
/// The backend supplies a range bounded to `parent_prefix`'s subtree that begins
/// at `parent_prefix ++ bound`: forward for the next sibling, reversed for the
/// previous. The walk skips every row whose first post-prefix segment equals
/// `bound` — those are `bound`'s own entry and its descendants, which arrive
/// consecutively (the same consecutive-equal collapse [`child_keys`] uses) — and
/// returns the first distinct child past them, so a child with its own deep
/// subtree is stepped over in one go. The range is lazy, so a backend stops at
/// that first differing row rather than materializing the whole subtree.
pub(crate) fn neighbor_child<'a>(
    entries: impl Entries<'a>,
    parent_prefix: &[u8],
    bound: &[u8],
) -> Result<Option<ChildSegment>, StoreError> {
    for entry in entries {
        let (key, _) = entry?;
        if !key.starts_with(parent_prefix) {
            break; // past the subtree (the bound was the edge child)
        }
        if key.len() <= parent_prefix.len() {
            continue; // the parent's own entry, not a child
        }
        let rest = &key[parent_prefix.len()..];
        let len = segment_len(rest).ok_or_else(|| corrupt(key))?;
        let segment = &rest[..len];
        if segment == bound {
            continue; // the bound child's own entry or one of its descendants
        }
        return Ok(Some(decode_child_segment(segment).ok_or_else(|| corrupt(key))?));
    }
    Ok(None)
}

/// The edge immediate child of `parent_prefix` in the stream's direction — the
/// first for a forward range, the last for a reversed one — or `None` when the
/// parent has no children. The backend supplies a range bounded to the subtree.
/// Used for `next`/`prev` of a bare layer: the first stored entry forward, the
/// last reversed. The range is lazy, so only the first row is examined.
pub(crate) fn edge_child<'a>(
    entries: impl Entries<'a>,
    parent_prefix: &[u8],
) -> Result<Option<ChildSegment>, StoreError> {
    for entry in entries {
        let (key, _) = entry?;
        if !key.starts_with(parent_prefix) {
            break; // past the subtree
        }
        if key.len() <= parent_prefix.len() {
            continue; // the parent's own entry, not a child
        }
        let rest = &key[parent_prefix.len()..];
        let len = segment_len(rest).ok_or_else(|| corrupt(key))?;
        let segment = &rest[..len];
        return Ok(Some(decode_child_segment(segment).ok_or_else(|| corrupt(key))?));
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
    for entry in entries {
        let (key, _) = entry?;
        if !key.starts_with(path) {
            break; // past the subtree
        }
        if key.len() > path.len() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Combine whether `path` holds a value with whether it has descendants into the
/// four-way presence state. A backend supplies the cheap value check and the range
/// for the descendant walk.
pub(crate) fn presence<'a>(
    has_value: bool,
    entries: impl Entries<'a>,
    path: &[u8],
) -> Result<Presence, StoreError> {
    Ok(match (has_value, has_descendants(entries, path)?) {
        (false, false) => Presence::Absent,
        (true, false) => Presence::ValueOnly,
        (false, true) => Presence::ChildrenOnly,
        (true, true) => Presence::ValueAndChildren,
    })
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
    let mut page = ScanPage::default();
    for entry in entries {
        let (key, value) = entry?;
        if !key.starts_with(path) {
            break; // past the subtree
        }
        if page.entries.len() == limit {
            page.truncated = true;
            break;
        }
        page.entries.push((key.to_vec(), value.to_vec()));
    }
    Ok(page)
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
