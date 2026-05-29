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

use crate::backend::{Backend, Presence, ScanPage, StoreError};
use crate::path::{ChildSegment, int_index_key_band, int_record_key_band};
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

    /// Whether the encoded `path` holds a value, children, both, or neither.
    pub fn presence(&self, path: &[u8]) -> Result<Presence, StoreError> {
        traversal::presence(self.entries.contains_key(path), self.range_from(path), path)
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
        traversal::child_keys(self.range_from(path), path)
    }

    /// Up to `limit` (encoded path, value) pairs in the subtree at the encoded
    /// `path`, in Marrow order, including the value at `path` itself when
    /// present. `truncated` is set when more remained past the limit.
    pub fn scan(&self, path: &[u8], limit: usize) -> ScanPage {
        // The in-memory range never faults, so the shared scan cannot error here.
        traversal::scan(self.range_from(path), path, limit).expect("in-memory scan never faults")
    }

    /// The distinct saved root names, in Marrow order. Returns
    /// [`StoreError::CorruptPath`] if a stored key does not begin with a valid
    /// root segment.
    pub fn roots(&self) -> Result<Vec<String>, StoreError> {
        traversal::roots(self.range_from(&[]))
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

/// The in-memory store serves the [`Backend`] contract by forwarding to its
/// inherent methods (reads return owned copies), and models transactions as a
/// stack of whole-map savepoints. The inherent methods stay available for direct
/// callers; the trait is how a generic consumer reaches any backend.
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

    fn presence(&self, path: &[u8]) -> Result<Presence, StoreError> {
        MemStore::presence(self, path)
    }

    fn child_keys(&self, path: &[u8]) -> Result<Vec<ChildSegment>, StoreError> {
        MemStore::child_keys(self, path)
    }

    fn scan(&self, path: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        Ok(MemStore::scan(self, path, limit))
    }

    fn roots(&self) -> Result<Vec<String>, StoreError> {
        MemStore::roots(self)
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
