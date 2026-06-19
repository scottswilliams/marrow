//! Private in-memory ordered-byte engine behind the typed tree-cell store.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ops::Bound;

use crate::backend::{Backend, ScanPage, StoreError, ValuePrefix};
use crate::traversal;

#[derive(Debug, Default, Clone)]
pub(crate) struct MemStore {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
    transaction: Option<TransactionBackup>,
    /// A frozen copy of `entries` while a read snapshot is pinned. Reads observe
    /// it, and this handle rejects writes and write transactions until the
    /// snapshot is released.
    snapshot: Option<BTreeMap<Vec<u8>, Vec<u8>>>,
}

#[derive(Debug, Clone)]
struct TransactionBackup {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
    depth: usize,
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

    fn range_from<'a>(&'a self, prefix: &[u8]) -> impl Iterator<Item = (&'a [u8], &'a [u8])> {
        self.view()
            .range(prefix.to_vec()..)
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
    }

    fn range_after<'a>(&'a self, cursor: &[u8]) -> impl Iterator<Item = (&'a [u8], &'a [u8])> {
        self.view()
            .range((Bound::Excluded(cursor.to_vec()), Bound::Unbounded))
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
    }

    fn range_before<'a>(&'a self, cursor: &[u8]) -> impl Iterator<Item = (&'a [u8], &'a [u8])> {
        self.view()
            .range((Bound::Unbounded, Bound::Excluded(cursor.to_vec())))
            .rev()
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
    }

    fn range_between<'a>(
        &'a self,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        exclude_lower: bool,
    ) -> impl DoubleEndedIterator<Item = (&'a [u8], &'a [u8])> {
        let lower = match (lower, exclude_lower) {
            (Some(lower), true) => Bound::Excluded(lower.to_vec()),
            (Some(lower), false) => Bound::Included(lower.to_vec()),
            (None, _) => Bound::Unbounded,
        };
        let upper = match upper {
            Some(upper) => Bound::Excluded(upper.to_vec()),
            None => Bound::Unbounded,
        };
        self.view()
            .range((lower, upper))
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
    }
}

fn scan_memory<T>(
    entries: impl IntoIterator<Item = T>,
    prefix: &[u8],
    limit: usize,
) -> Result<ScanPage, StoreError>
where
    T: traversal::ScanEntry,
{
    traversal::scan(
        entries.into_iter().map(Result::<_, Infallible>::Ok),
        prefix,
        limit,
        |error| match error {},
    )
}

impl Backend for MemStore {
    fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(MemStore::read(self, key).map(<[u8]>::to_vec))
    }

    fn read_prefix(&self, key: &[u8], limit: usize) -> Result<Option<ValuePrefix>, StoreError> {
        Ok(MemStore::read(self, key).map(|value| {
            let copied = value.len().min(limit);
            ValuePrefix {
                bytes: value[..copied].to_vec(),
                truncated: value.len() > limit,
            }
        }))
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
        scan_memory(self.range_from(prefix), prefix, limit)
    }

    fn scan_after(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        scan_memory(self.range_after(cursor), prefix, limit)
    }

    fn scan_before(
        &self,
        prefix: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        scan_memory(self.range_before(cursor), prefix, limit)
    }

    fn scan_between(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        scan_memory(self.range_between(lower, upper, false), prefix, limit)
    }

    fn scan_between_after(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        let lower = Some(match lower {
            Some(lower) if lower > cursor => lower,
            _ => cursor,
        });
        scan_memory(self.range_between(lower, upper, true), prefix, limit)
    }

    fn scan_between_before(
        &self,
        prefix: &[u8],
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        cursor: &[u8],
        limit: usize,
    ) -> Result<ScanPage, StoreError> {
        let upper = Some(match upper {
            Some(upper) if upper < cursor => upper,
            _ => cursor,
        });
        scan_memory(self.range_between(lower, upper, false).rev(), prefix, limit)
    }

    fn begin(&mut self) -> Result<(), StoreError> {
        if self.snapshot.is_some() {
            return Err(StoreError::begin_while_snapshot_pinned());
        }
        match &mut self.transaction {
            Some(transaction) => transaction.depth += 1,
            None => {
                self.transaction = Some(TransactionBackup {
                    entries: self.entries.clone(),
                    depth: 1,
                });
            }
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        let Some(transaction) = &mut self.transaction else {
            return Ok(());
        };
        if transaction.depth > 1 {
            transaction.depth -= 1;
        } else {
            self.transaction = None;
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), StoreError> {
        if let Some(transaction) = self.transaction.take() {
            self.entries = transaction.entries;
        }
        Ok(())
    }

    fn begin_snapshot(&mut self) -> Result<(), StoreError> {
        if self.transaction.is_some() {
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
    use crate::backend::StoreError;
    use crate::conformance;

    #[test]
    fn mem_store_passes_the_substrate_conformance_suite() -> Result<(), StoreError> {
        conformance::run_all(|| Ok(MemStore::default()))
    }
}
