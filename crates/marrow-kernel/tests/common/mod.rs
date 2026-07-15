// A shared test-support module: each integration test binary that includes it uses a
// different subset of the helpers, so unused-item warnings here are expected.
#![allow(dead_code)]

//! A counting byte engine shared by the kernel's engine-work witnesses.
//!
//! It wraps the in-memory backend and tallies store *opens* (`read_view`, `begin`,
//! `audit_integrity`) and transaction *writes* (`put`, `remove`) through independent
//! shared counters a test reads without owning the store. The tallies are the
//! evidence: an authority rejection performs zero opens, an in-memory-rejected write
//! performs zero writes, and exact field work is a constant number of writes
//! independent of the resource's declared width.

use std::cell::Cell;
use std::rc::Rc;

use marrow_store::{
    ByteEngine, Cell as StoreCell, CommitOutcome, MemoryEngine, ReadView, StoreError, WriteTxn,
};

/// The two shared counters: store opens and transaction writes.
#[derive(Clone, Default)]
pub struct Counters {
    pub opens: Rc<Cell<usize>>,
    pub writes: Rc<Cell<usize>>,
}

impl Counters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn opens(&self) -> usize {
        self.opens.get()
    }

    pub fn writes(&self) -> usize {
        self.writes.get()
    }
}

/// A byte engine that counts opens and writes, delegating to an in-memory backend.
pub struct CountingEngine {
    inner: MemoryEngine,
    counters: Counters,
}

impl CountingEngine {
    pub fn new(counters: Counters) -> Self {
        Self {
            inner: MemoryEngine::new(),
            counters,
        }
    }

    fn count_open(&self) {
        self.counters.opens.set(self.counters.opens.get() + 1);
    }
}

/// A transaction wrapper that counts every staged write and delegates reads and
/// writes to the in-memory backend's transaction.
pub struct CountingTxn<'a> {
    inner: <MemoryEngine as ByteEngine>::Txn<'a>,
    writes: Rc<Cell<usize>>,
}

impl ReadView for CountingTxn<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }
    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<StoreCell>, StoreError> {
        self.inner.scan_after(prefix, cursor)
    }
}

impl WriteTxn for CountingTxn<'_> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.writes.set(self.writes.get() + 1);
        self.inner.put(key, value)
    }
    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.writes.set(self.writes.get() + 1);
        self.inner.remove(key)
    }
    fn commit(self) -> CommitOutcome {
        self.inner.commit()
    }
}

impl ByteEngine for CountingEngine {
    type View<'a> = <MemoryEngine as ByteEngine>::View<'a>;
    type Txn<'a> = CountingTxn<'a>;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        self.count_open();
        self.inner.read_view()
    }

    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        self.count_open();
        Ok(CountingTxn {
            inner: self.inner.begin()?,
            writes: self.counters.writes.clone(),
        })
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        // A pure capability probe with no store I/O; deliberately uncounted.
        self.inner.require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.count_open();
        self.inner.audit_integrity()
    }
}
