//! Private in-memory ordered-byte engine behind the typed tree-cell store.

use std::collections::BTreeMap;
use std::ops::Bound;

use crate::backend::{Backend, ScanPage, StoreError};
use crate::traversal;

#[derive(Debug, Default, Clone)]
pub(crate) struct MemStore {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
    savepoints: Vec<BTreeMap<Vec<u8>, Vec<u8>>>,
    /// A frozen copy of `entries` while a read snapshot is pinned. Reads observe
    /// it, and this handle rejects writes and write transactions until the
    /// snapshot is released.
    snapshot: Option<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl MemStore {
    fn write(&mut self, key: &[u8], value: Vec<u8>) {
        self.entries.insert(key.to_vec(), value);
    }

    /// The map reads observe: the pinned snapshot if one is held, else live data.
    fn view(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> {
        self.snapshot.as_ref().unwrap_or(&self.entries)
    }

    fn read(&self, key: &[u8]) -> Option<&[u8]> {
        self.view().get(key).map(Vec::as_slice)
    }

    fn delete(&mut self, prefix: &[u8]) {
        self.entries
            .retain(|key, _| key.as_slice() != prefix && !key.starts_with(prefix));
    }

    fn scan(&self, prefix: &[u8], limit: usize) -> ScanPage {
        traversal::scan(self.range_from(prefix), prefix, limit).expect("memory scan is infallible")
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8], limit: usize) -> ScanPage {
        traversal::scan(self.range_after(cursor), prefix, limit).expect("memory scan is infallible")
    }

    fn range_from<'a>(
        &'a self,
        prefix: &[u8],
    ) -> impl Iterator<Item = Result<(&'a [u8], &'a [u8]), StoreError>> {
        self.view()
            .range(prefix.to_vec()..)
            .map(|(key, value)| Ok((key.as_slice(), value.as_slice())))
    }

    fn range_after<'a>(
        &'a self,
        cursor: &[u8],
    ) -> impl Iterator<Item = Result<(&'a [u8], &'a [u8]), StoreError>> {
        self.view()
            .range((Bound::Excluded(cursor.to_vec()), Bound::Unbounded))
            .map(|(key, value)| Ok((key.as_slice(), value.as_slice())))
    }
}

impl Backend for MemStore {
    fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(MemStore::read(self, key).map(<[u8]>::to_vec))
    }

    fn write(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        if self.snapshot.is_some() {
            return Err(StoreError::write_while_snapshot_pinned());
        }
        MemStore::write(self, key, value);
        Ok(())
    }

    fn delete(&mut self, prefix: &[u8]) -> Result<(), StoreError> {
        if self.snapshot.is_some() {
            return Err(StoreError::delete_while_snapshot_pinned());
        }
        MemStore::delete(self, prefix);
        Ok(())
    }

    fn scan(&self, prefix: &[u8], limit: usize) -> Result<ScanPage, StoreError> {
        Ok(MemStore::scan(self, prefix, limit))
    }

    fn scan_after(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        Ok(MemStore::scan_after(self, prefix, cursor, limit))
    }

    fn begin(&mut self) -> Result<(), StoreError> {
        if self.snapshot.is_some() {
            return Err(StoreError::begin_while_snapshot_pinned());
        }
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

    fn begin_snapshot(&mut self) -> Result<(), StoreError> {
        if !self.savepoints.is_empty() {
            return Err(StoreError::snapshot_while_transaction_open());
        }
        if self.snapshot.is_some() {
            return Err(StoreError::snapshot_already_pinned());
        }
        self.snapshot = Some(self.entries.clone());
        Ok(())
    }

    fn end_snapshot(&mut self) {
        self.snapshot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::MemStore;
    use crate::conformance;

    #[test]
    fn mem_store_passes_the_substrate_conformance_suite() {
        conformance::run_all(MemStore::default);
    }
}
