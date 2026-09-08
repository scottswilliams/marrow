// Each test executable uses a different subset of these shared helpers.
#![allow(dead_code)]

//! Operation counters over either production byte-engine implementation.
//!
//! Opens, gets, scans, staged writes and commits are separate. Returned page cells
//! and bytes measure the scan API's copies, not engine cache or allocation costs.

use std::cell::Cell;
use std::rc::Rc;

use marrow_store::{
    ByteEngine, Cell as StoreCell, CommitOutcome, MemoryEngine, ReadView, StoreError, WriteTxn,
};

#[derive(Clone, Default)]
pub struct Counters {
    pub opens: Rc<Cell<usize>>,
    pub writes: Rc<Cell<usize>>,
    pub reads: Rc<Cell<usize>>,
    pub gets: Rc<Cell<usize>>,
    pub scans: Rc<Cell<usize>>,
    pub commits: Rc<Cell<usize>>,
    pub returned_cells: Rc<Cell<usize>>,
    pub returned_bytes: Rc<Cell<usize>>,
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
    pub fn reads(&self) -> usize {
        self.reads.get()
    }
    pub fn gets(&self) -> usize {
        self.gets.get()
    }
    pub fn scans(&self) -> usize {
        self.scans.get()
    }
    pub fn commits(&self) -> usize {
        self.commits.get()
    }
    pub fn returned_cells(&self) -> usize {
        self.returned_cells.get()
    }
    pub fn returned_bytes(&self) -> usize {
        self.returned_bytes.get()
    }

    fn record_page(&self, page: &[StoreCell]) {
        self.returned_cells
            .set(self.returned_cells.get() + page.len());
        self.returned_bytes.set(
            self.returned_bytes.get()
                + page
                    .iter()
                    .map(|(key, value)| key.len() + value.len())
                    .sum::<usize>(),
        );
    }
}

pub struct CountingEngine<E = MemoryEngine> {
    inner: E,
    counters: Counters,
}

impl CountingEngine {
    pub fn new(counters: Counters) -> Self {
        Self::from_engine(MemoryEngine::new(), counters)
    }
}

impl<E> CountingEngine<E> {
    pub fn from_engine(inner: E, counters: Counters) -> Self {
        Self { inner, counters }
    }
}

pub struct CountingView<V> {
    inner: V,
    counters: Counters,
}

impl<V: ReadView> ReadView for CountingView<V> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.counters.reads.set(self.counters.reads.get() + 1);
        self.counters.gets.set(self.counters.gets.get() + 1);
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<StoreCell>, StoreError> {
        self.counters.reads.set(self.counters.reads.get() + 1);
        self.counters.scans.set(self.counters.scans.get() + 1);
        let page = self.inner.scan_after(prefix, cursor)?;
        self.counters.record_page(&page);
        Ok(page)
    }
}

pub struct CountingTxn<T> {
    inner: T,
    counters: Counters,
}

impl<T: ReadView> ReadView for CountingTxn<T> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.counters.reads.set(self.counters.reads.get() + 1);
        self.counters.gets.set(self.counters.gets.get() + 1);
        self.inner.get(key)
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<StoreCell>, StoreError> {
        self.counters.reads.set(self.counters.reads.get() + 1);
        self.counters.scans.set(self.counters.scans.get() + 1);
        let page = self.inner.scan_after(prefix, cursor)?;
        self.counters.record_page(&page);
        Ok(page)
    }
}

impl<T: WriteTxn> WriteTxn for CountingTxn<T> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.counters.writes.set(self.counters.writes.get() + 1);
        self.inner.put(key, value)
    }

    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.counters.writes.set(self.counters.writes.get() + 1);
        self.inner.remove(key)
    }

    fn commit(self) -> CommitOutcome {
        self.counters.commits.set(self.counters.commits.get() + 1);
        self.inner.commit()
    }
}

impl<E: ByteEngine> ByteEngine for CountingEngine<E> {
    type View<'a>
        = CountingView<E::View<'a>>
    where
        Self: 'a;
    type Txn<'a>
        = CountingTxn<E::Txn<'a>>
    where
        Self: 'a;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        self.counters.opens.set(self.counters.opens.get() + 1);
        Ok(CountingView {
            inner: self.inner.read_view()?,
            counters: self.counters.clone(),
        })
    }

    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        self.counters.opens.set(self.counters.opens.get() + 1);
        Ok(CountingTxn {
            inner: self.inner.begin()?,
            counters: self.counters.clone(),
        })
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        self.inner.require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.counters.opens.set(self.counters.opens.get() + 1);
        self.inner.audit_integrity()
    }
}
