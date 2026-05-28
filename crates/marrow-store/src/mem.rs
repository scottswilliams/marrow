//! An in-memory saved-tree store.
//!
//! [`MemStore`] keeps encoded saved paths mapped to encoded values in a
//! `BTreeMap`, so iteration is already in Marrow order (the natural order of
//! encoded path bytes — see [`crate::path`]). It is the reference store for
//! tests and short runs; a persistent backend implements the same behavior.
//!
//! The store operates on already-encoded paths and value bytes: it is the
//! ordered-bytes backend layer, below the schema. Callers encode logical paths
//! with [`crate::path::encode_path`] before calling it, and values with
//! [`crate::value::encode_value`]. The store itself never parses schemas.

use std::collections::BTreeMap;

use crate::path::{ChildSegment, decode_child_segment, root_name, segment_len};

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
/// stored path it cannot decode; a persistent backend adds I/O and limit
/// variants atop this contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// A stored key is not a well-formed sequence of path segments.
    CorruptPath { path: Vec<u8> },
}

/// One page of a bounded scan: the entries found in Marrow order, and whether
/// more remained past the limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanPage {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
    pub truncated: bool,
}

/// An in-memory map of encoded saved paths to encoded values.
#[derive(Debug, Default)]
pub struct MemStore {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write `value` at the encoded `path`, replacing any value already there.
    pub fn write(&mut self, path: &[u8], value: Vec<u8>) {
        self.entries.insert(path.to_vec(), value);
    }

    /// The exact value at the encoded `path`, or `None` when no value is stored
    /// there. Absence is never a stored sentinel; an unpopulated path simply has
    /// no entry.
    pub fn read(&self, path: &[u8]) -> Option<&[u8]> {
        self.entries.get(path).map(Vec::as_slice)
    }

    /// Whether the encoded `path` holds a value, children, both, or neither.
    pub fn presence(&self, path: &[u8]) -> Presence {
        match (self.entries.contains_key(path), self.has_descendants(path)) {
            (false, false) => Presence::Absent,
            (true, false) => Presence::ValueOnly,
            (false, true) => Presence::ChildrenOnly,
            (true, true) => Presence::ValueAndChildren,
        }
    }

    /// Remove the value at the encoded `path` and every value below it. Deleting
    /// an absent path is a no-op.
    pub fn delete(&mut self, path: &[u8]) {
        self.entries
            .retain(|key, _| key.as_slice() != path && !key.starts_with(path));
    }

    /// The distinct immediate children directly below the encoded `path`, in
    /// Marrow order (descendants sharing an immediate child collapse to one).
    /// Returns [`StoreError::CorruptPath`] if a stored descendant key cannot be
    /// decoded.
    pub fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        let mut children = Vec::new();
        let mut last: Option<Vec<u8>> = None;
        for key in self.subtree_keys(path) {
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

    /// Up to `limit` (encoded path, value) pairs in the subtree at the encoded
    /// `path`, in Marrow order, including the value at `path` itself when
    /// present. `truncated` is set when more remained past the limit.
    pub fn scan(&self, path: &[u8], limit: usize) -> ScanPage {
        let mut page = ScanPage::default();
        for (key, value) in self
            .entries
            .range(path.to_vec()..)
            .take_while(|(key, _)| key.starts_with(path))
        {
            if page.entries.len() == limit {
                page.truncated = true;
                break;
            }
            page.entries.push((key.clone(), value.clone()));
        }
        page
    }

    /// The distinct saved root names, in Marrow order. Returns
    /// [`StoreError::CorruptPath`] if a stored key does not begin with a valid
    /// root segment.
    pub fn roots(&self) -> Result<Vec<String>, StoreError> {
        let mut roots: Vec<String> = Vec::new();
        for key in self.entries.keys() {
            let name = root_name(key).ok_or_else(|| corrupt(key))?;
            if roots.last() != Some(&name) {
                roots.push(name);
            }
        }
        Ok(roots)
    }

    /// Does any stored key lie strictly below `prefix`? An encoded ancestor is a
    /// byte-prefix of its descendants, and segment terminators keep unrelated
    /// paths from sharing the prefix, so a longer prefixed key is a descendant.
    fn has_descendants(&self, prefix: &[u8]) -> bool {
        self.subtree_keys(prefix)
            .any(|key| key.len() > prefix.len())
    }

    /// The stored keys in the subtree at `prefix` (the prefix entry and every
    /// descendant), in Marrow order.
    fn subtree_keys<'a>(&'a self, prefix: &'a [u8]) -> impl Iterator<Item = &'a Vec<u8>> {
        self.entries
            .range(prefix.to_vec()..)
            .map(|(key, _)| key)
            .take_while(move |key| key.starts_with(prefix))
    }
}

/// Build a [`StoreError::CorruptPath`] for a stored key that failed to decode.
fn corrupt(key: &[u8]) -> StoreError {
    StoreError::CorruptPath { path: key.to_vec() }
}
