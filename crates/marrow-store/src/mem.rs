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
    /// Marrow order. Descendants sharing an immediate child collapse to a single
    /// entry.
    pub fn child_keys(&self, path: &[u8]) -> Vec<ChildSegment> {
        let mut children = Vec::new();
        let mut last: Option<Vec<u8>> = None;
        for key in self.subtree_keys(path) {
            if key.len() <= path.len() {
                continue; // the path's own entry, not a child
            }
            let rest = &key[path.len()..];
            let Some(len) = segment_len(rest) else {
                continue; // malformed encoding; skip defensively
            };
            let segment = &rest[..len];
            if last.as_deref() == Some(segment) {
                continue; // same immediate child as the previous descendant
            }
            last = Some(segment.to_vec());
            if let Some(child) = decode_child_segment(segment) {
                children.push(child);
            }
        }
        children
    }

    /// Every (encoded path, value) pair in the subtree at the encoded `path`, in
    /// Marrow order, including the value at `path` itself when present.
    pub fn scan(&self, path: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.entries
            .range(path.to_vec()..)
            .take_while(|(key, _)| key.starts_with(path))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// The distinct saved root names, in Marrow order.
    pub fn roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = Vec::new();
        for key in self.entries.keys() {
            if let Some(name) = root_name(key)
                && roots.last() != Some(&name)
            {
                roots.push(name);
            }
        }
        roots
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
