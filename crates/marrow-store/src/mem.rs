//! The in-memory ordered-byte engine: the differential proving ground for the
//! path kernel. Not durable across processes.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ops::Bound;

use crate::engine::{ByteEngine, Cell, CommitOutcome, ReadView, WriteTxn, check_cell_limits};
use crate::error::StoreError;
use crate::traversal;

type Map = BTreeMap<Vec<u8>, Vec<u8>>;

/// An in-memory ordered-byte engine.
#[derive(Debug, Default)]
pub struct MemoryEngine {
    entries: Map,
}

impl MemoryEngine {
    /// A fresh empty in-memory engine.
    pub fn new() -> Self {
        Self::default()
    }
}

fn collect_after(map: &Map, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
    let range = map
        .range((Bound::Excluded(cursor.to_vec()), Bound::Unbounded))
        .map(|(key, value)| Result::<_, Infallible>::Ok((key.as_slice(), value.as_slice())));
    traversal::collect_after(range, prefix, |error| match error {})
}

impl ByteEngine for MemoryEngine {
    type View<'a> = MemView<'a>;
    type Txn<'a> = MemTxn<'a>;

    fn read_view(&self) -> Result<MemView<'_>, StoreError> {
        Ok(MemView {
            entries: &self.entries,
        })
    }

    fn begin(&mut self) -> Result<MemTxn<'_>, StoreError> {
        let working = self.entries.clone();
        Ok(MemTxn {
            base: &mut self.entries,
            working,
        })
    }

    fn require_write_access(&self, _op: &'static str) -> Result<(), StoreError> {
        Ok(())
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        // The in-memory engine has no durable substrate beneath it: its cells are
        // the only representation, so there is nothing to walk and no external
        // mutation to detect.
        Ok(())
    }
}

/// A coherent read view: the engine cannot be mutated while it borrows the map,
/// so the view it reads is stable for its life.
pub struct MemView<'a> {
    entries: &'a Map,
}

impl ReadView for MemView<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.entries.get(key).cloned())
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        collect_after(self.entries, prefix, cursor)
    }
}

/// A write transaction over a working copy. Reads observe its own staged writes;
/// [`commit`](WriteTxn::commit) swaps the working copy into the engine, and a
/// dropped transaction discards it.
pub struct MemTxn<'a> {
    base: &'a mut Map,
    working: Map,
}

impl ReadView for MemTxn<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.working.get(key).cloned())
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
        collect_after(&self.working, prefix, cursor)
    }
}

impl WriteTxn for MemTxn<'_> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        check_cell_limits(key, &value)?;
        self.working.insert(key.to_vec(), value);
        Ok(())
    }

    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.working.remove(key);
        Ok(())
    }

    fn commit(self) -> CommitOutcome {
        // An in-memory swap is atomic and cannot half-apply, so the outcome is
        // always confirmed; the aborted/indeterminate arms exist for native
        // durability, not this engine.
        *self.base = self.working;
        CommitOutcome::Confirmed
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryEngine;
    use crate::conformance;
    use crate::error::StoreError;

    #[test]
    fn memory_engine_passes_the_conformance_suite() -> Result<(), StoreError> {
        conformance::run_all(|| Ok(MemoryEngine::new()))
    }
}
