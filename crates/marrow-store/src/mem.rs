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

use std::ops::Bound;

use crate::backend::{Backend, Presence, ScanPage, StoreError};
use crate::path::{ChildSegment, int_index_key_band, int_record_key_band, subtree_band};
use crate::traversal;

/// An in-memory map of encoded saved paths to encoded values. Transactions are a
/// stack of whole-map savepoints (see the [`Backend`] `begin`/`commit`/`rollback`
/// implementation): `begin` pushes a clone of the map, `commit` drops it keeping
/// the live map, and `rollback` restores it.
#[derive(Debug, Default, Clone)]
pub struct MemStore {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
    savepoints: Vec<BTreeMap<Vec<u8>, Vec<u8>>>,
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

    /// Remove the value at the encoded `path` and every value below it. Deleting
    /// an absent path is a no-op.
    pub fn delete(&mut self, path: &[u8]) {
        self.entries
            .retain(|key, _| key.as_slice() != path && !key.starts_with(path));
    }

    /// Up to `limit` (encoded path, value) pairs in the subtree at the encoded
    /// `path`, in Marrow order, including the value at `path` itself when
    /// present. `truncated` is set when more remained past the limit.
    pub fn scan(&self, path: &[u8], limit: usize) -> ScanPage {
        // The in-memory range never faults, so the shared scan cannot error here.
        traversal::scan(self.range_from(path), path, limit).expect("in-memory scan never faults")
    }

    /// The stored entries from `prefix` onward, in Marrow order, adapted to the
    /// shared [`traversal`] item shape. The in-memory range is infallible, so each
    /// pair is wrapped as `Ok`; the prefix bound is applied by the traversal
    /// functions themselves.
    fn range_from<'a>(
        &'a self,
        prefix: &[u8],
    ) -> impl Iterator<Item = Result<(&'a [u8], &'a [u8]), StoreError>> {
        self.entries
            .range(prefix.to_vec()..)
            .map(|(key, value)| Ok((key.as_slice(), value.as_slice())))
    }

    /// The subtree at `prefix` walked in reverse, adapted to the shared item
    /// shape. Reversing a `BTreeMap` range is free (`.rev()` on a double-ended
    /// iterator), but the range must be bounded to the subtree first: an unbounded
    /// reverse range starts at the global maximum, where the first rows lie
    /// outside the subtree and the prefix break would fire at once. `subtree_band`
    /// supplies the half-open `[prefix, successor)` bound; an open upper bound
    /// (`None`) means the subtree runs to the end of the store.
    fn range_band_rev<'a>(
        &'a self,
        prefix: &[u8],
    ) -> impl Iterator<Item = Result<(&'a [u8], &'a [u8]), StoreError>> {
        let (lo, hi) = subtree_band(prefix);
        let upper = match hi {
            Some(hi) => Bound::Excluded(hi),
            None => Bound::Unbounded,
        };
        self.entries
            .range((Bound::Included(lo), upper))
            .rev()
            .map(|(key, value)| Ok((key.as_slice(), value.as_slice())))
    }

    /// The highest integer key in the half-open byte `band` of integer-keyed
    /// children of `prefix`. The band is one contiguous numeric-ordered run, so
    /// its last entry is the highest; the shared decode reads the key just past the
    /// kind tag. `None` when the band is empty. O(log n), not a full child walk.
    fn max_int_in_band(
        &self,
        prefix: &[u8],
        (lo, hi): (Vec<u8>, Vec<u8>),
    ) -> Result<Option<i64>, StoreError> {
        let last = self
            .entries
            .range(lo..hi)
            .next_back()
            .map(|(key, _)| Ok(key.as_slice()));
        traversal::max_int_key(last, prefix)
    }
}

/// The in-memory store serves the [`Backend`] contract directly; the few inherent
/// methods with a more convenient shape (`read` borrows, `write`/`delete` are
/// infallible, `scan` cannot fault) forward here. Transactions are a stack of
/// whole-map savepoints. The trait is how a generic consumer reaches any backend.
impl Backend for MemStore {
    fn read(&self, path: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(MemStore::read(self, path).map(<[u8]>::to_vec))
    }

    fn write(&mut self, path: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        MemStore::write(self, path, value);
        Ok(())
    }

    fn delete(&mut self, path: &[u8]) -> Result<(), StoreError> {
        MemStore::delete(self, path);
        Ok(())
    }

    /// Whether the encoded `path` holds a value, children, both, or neither.
    fn presence(&self, path: &[u8]) -> Result<Presence, StoreError> {
        traversal::presence(self.entries.contains_key(path), self.range_from(path), path)
    }

    /// The distinct immediate children directly below the encoded `path`, in
    /// Marrow order (descendants sharing an immediate child collapse to one).
    /// Returns [`StoreError::CorruptPath`] if a stored descendant key cannot be
    /// decoded.
    fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        traversal::child_keys(self.range_from(path), path)
    }

    /// The distinct immediate children directly below the encoded `path`, in
    /// reverse Marrow order — the exact reverse of [`child_keys`](Self::child_keys).
    /// A `BTreeMap` range is double-ended, so reversing it is `.rev()`, not a
    /// forward walk reversed afterward; the shared collapse is direction-symmetric.
    fn child_keys_rev(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        traversal::child_keys(self.range_band_rev(path), path)
    }

    /// The immediate *key* child of the encoded `parent` directly after `after` in
    /// Marrow order, or `None` when `after` is the last key child. `after` is one
    /// encoded child segment. The range begins at `parent ++ after` (inclusive) and
    /// the shared seek skips `after`'s own subtree, and any named member, to the
    /// first distinct key child.
    fn next_sibling(
        &self,
        parent: &[u8],
        after: &[u8],
    ) -> Result<Option<ChildSegment>, StoreError> {
        let mut from = parent.to_vec();
        from.extend_from_slice(after);
        traversal::neighbor_child(self.range_from(&from), parent, after)
    }

    /// The immediate *key* child of the encoded `parent` directly before `before`,
    /// or `None` when `before` is the first key child. The mirror of
    /// [`next_sibling`](Self::next_sibling) over a reversed range ending at
    /// `parent ++ before` (inclusive).
    fn prev_sibling(
        &self,
        parent: &[u8],
        before: &[u8],
    ) -> Result<Option<ChildSegment>, StoreError> {
        let mut to = parent.to_vec();
        to.extend_from_slice(before);
        // Range `[parent, to]` reversed: the first reversed row at or below
        // `before` is `before`'s deepest descendant, which the seek skips along
        // with the rest of `before`'s subtree to the first distinct prior child.
        let rev = self
            .entries
            .range((Bound::Included(parent.to_vec()), Bound::Included(to)))
            .rev()
            .map(|(key, value)| Ok((key.as_slice(), value.as_slice())));
        traversal::neighbor_child(rev, parent, before)
    }

    /// The first immediate *key* child of the encoded `parent` in Marrow order, or
    /// `None` when it has none — the bare-layer entry point for `next`. Named
    /// members are skipped, as the shared seek navigates key positions only.
    fn first_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError> {
        traversal::neighbor_child(self.range_from(parent), parent, b"")
    }

    /// The last immediate *key* child of the encoded `parent` in Marrow order, or
    /// `None` when it has none — the bare-layer entry point for `prev`. Named
    /// members, which sort after the key children, are skipped.
    fn last_child(&self, parent: &[u8]) -> Result<Option<ChildSegment>, StoreError> {
        traversal::neighbor_child(self.range_band_rev(parent), parent, b"")
    }

    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        Ok(MemStore::scan(self, path, limit))
    }

    /// The distinct saved root names, in Marrow order. Returns
    /// [`StoreError::CorruptPath`] if a stored key does not begin with a valid
    /// root segment.
    fn roots(&self) -> Result<Vec<String>, StoreError> {
        traversal::roots(self.range_from(&[]))
    }

    fn max_int_record_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError> {
        self.max_int_in_band(prefix, int_record_key_band(prefix))
    }

    fn max_int_index_key(&self, prefix: &[u8]) -> Result<Option<i64>, StoreError> {
        self.max_int_in_band(prefix, int_index_key_band(prefix))
    }

    /// Cloning the whole map per savepoint is intentional: this is the
    /// reference/test store for small stores and short runs, where a wholesale
    /// snapshot stays dead-simple and obviously correct. Large-store efficiency
    /// is the persistent backend's job (redb keeps a per-key undo journal).
    fn begin(&mut self) -> Result<(), StoreError> {
        self.savepoints.push(self.entries.clone());
        Ok(())
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        self.savepoints.pop();
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), StoreError> {
        if let Some(snapshot) = self.savepoints.pop() {
            self.entries = snapshot;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MemStore;
    use crate::conformance;

    #[test]
    fn mem_store_passes_the_conformance_suite() {
        conformance::run_all(MemStore::new);
    }
}
