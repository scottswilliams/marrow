//! A fault-injecting byte engine shared by the kernel's commit-recovery tests and the VM's
//! private commit-poison tests: it delegates to an in-memory backend but reports a
//! test-chosen commit verdict independently of whether the staged bytes land, and can fail
//! a chosen mid-transaction write. Test source included by `#[path]` from both sites; it is
//! not a crate, a production module, or a second engine implementation.

// Each including test module uses a different subset of the helpers, so unused-item
// warnings here are expected.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ops::Bound;
use std::rc::Rc;

use marrow_kernel::codec::value::ScalarKind;
use marrow_kernel::durable::{
    DemandCoverage, DurableStore, SiteTarget, StoreProjection, StoreSchema, StoreSchemaBuilder,
};
use marrow_store::{ByteEngine, Cell as StoreCell, CommitOutcome, ReadView, StoreError, WriteTxn};

/// The single-root projection a case opens under: the root, plus its sites resolved against
/// it. Every site here names root 0 — the store's only root.
pub(super) fn project(schema: &StoreSchema, sites: Vec<SiteTarget>) -> StoreProjection {
    let mut projection = StoreProjection::builder();
    projection.root(schema.clone());
    for target in sites {
        projection.site(0, target);
    }
    projection
        .finish()
        .expect("every site names the one declared root")
}

/// What the double's next `commit` does: the reported durability verdict is chosen
/// independently of whether the staged bytes actually land, so an indeterminate verdict
/// can accompany either a persisted or a discarded write.
#[derive(Clone, Copy, Debug)]
pub(super) enum Mode {
    /// Persist the staged bytes and report `Confirmed` (the honest in-memory path).
    Confirm,
    /// Persist the staged bytes but report `Indeterminate`: the write landed, yet the
    /// caller cannot know it — a reopen must classify `KnownNew`.
    IndeterminatePersist,
    /// Discard the staged bytes and report `Indeterminate`: the write did not land, and
    /// a reopen must classify `KnownOld`.
    IndeterminateDrop,
    /// Discard the staged bytes and report `Aborted`: a clean abort that leaves the
    /// store unchanged and unpoisoned.
    Abort,
}

/// A shared, test-controlled commit mode. A transaction snapshots it at `begin`, so a
/// test flips the mode between sessions to model, e.g., a recovered store committing
/// cleanly after an earlier abort.
#[derive(Clone)]
pub(super) struct ModeHandle(Rc<Cell<Mode>>);

impl ModeHandle {
    pub(super) fn new(mode: Mode) -> Self {
        Self(Rc::new(Cell::new(mode)))
    }
    pub(super) fn set(&self, mode: Mode) {
        self.0.set(mode);
    }
    pub(super) fn get(&self) -> Mode {
        self.0.get()
    }
}

/// A test-controlled mid-transaction write fault. A transaction snapshots the target at
/// `begin`; the double then returns a [`StoreError`] from the Nth write op (`put` or
/// `remove`, counted together, 1-based) that transaction issues — modelling an engine
/// write that fails partway through a commit or an apply plan, before the transaction's
/// own `commit`. `None` never faults.
#[derive(Clone)]
pub(super) struct WriteFaultHandle(Rc<Cell<Option<u32>>>);

impl WriteFaultHandle {
    pub(super) fn inert() -> Self {
        Self(Rc::new(Cell::new(None)))
    }
    pub(super) fn set(&self, target: Option<u32>) {
        self.0.set(target);
    }
    pub(super) fn get(&self) -> Option<u32> {
        self.0.get()
    }
}

/// A byte engine that delegates to an in-memory backend but resolves each commit per a
/// test-chosen [`Mode`] and may fail a chosen mid-transaction write per a
/// [`WriteFaultHandle`].
#[derive(Clone)]
pub(super) struct FaultEngine {
    inner: Rc<RefCell<BTreeMap<Vec<u8>, Vec<u8>>>>,
    mode: ModeHandle,
    write_fault: WriteFaultHandle,
}

impl FaultEngine {
    /// A double that only ever misreports the commit verdict; no write faults.
    pub(super) fn new(mode: ModeHandle) -> Self {
        Self::with_write_fault(mode, WriteFaultHandle::inert())
    }
    /// A double that both resolves commit per `mode` and fails the write `write_fault`
    /// selects, so a test can exercise an engine write that fails mid-transaction.
    pub(super) fn with_write_fault(mode: ModeHandle, write_fault: WriteFaultHandle) -> Self {
        Self {
            inner: Rc::new(RefCell::new(BTreeMap::new())),
            mode,
            write_fault,
        }
    }
}

/// An owned coherent snapshot. The test engine shares durable bytes across separately
/// constructed handles so recovery KATs can actually drop the poisoned handle and reopen;
/// cloning the map here keeps each read view stable for its lifetime.
pub(super) struct FaultView {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

fn scan_map(entries: &BTreeMap<Vec<u8>, Vec<u8>>, prefix: &[u8], cursor: &[u8]) -> Vec<StoreCell> {
    const MAX_RECORDS: usize = 64;
    const MAX_BYTES: usize = 1 << 20;

    let mut out = Vec::new();
    let mut aggregate = 0usize;
    for (key, value) in entries.range((Bound::Excluded(cursor.to_vec()), Bound::Unbounded)) {
        if !key.starts_with(prefix) {
            if key.as_slice() < prefix {
                continue;
            }
            break;
        }
        if out.len() == MAX_RECORDS {
            break;
        }
        let next_aggregate = aggregate.saturating_add(key.len() + value.len());
        if next_aggregate > MAX_BYTES && !out.is_empty() {
            break;
        }
        aggregate = next_aggregate;
        out.push((key.clone(), value.clone()));
    }
    out
}

impl ReadView for FaultView {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.entries.get(key).cloned())
    }

    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<StoreCell>, StoreError> {
        Ok(scan_map(&self.entries, prefix, cursor))
    }
}

/// The double's transaction: the backend transaction plus the mode captured at `begin`
/// and the mid-transaction write fault (the 1-based write index to fail, and a running
/// count of the writes issued so far).
pub(super) struct FaultTxn {
    base: Rc<RefCell<BTreeMap<Vec<u8>, Vec<u8>>>>,
    working: BTreeMap<Vec<u8>, Vec<u8>>,
    mode: Mode,
    fail_on_write: Option<u32>,
    writes: u32,
}

impl FaultTxn {
    /// Count this write and, if it is the one the test chose, report the injected fault
    /// instead of performing it.
    fn maybe_fault(&mut self, op: &'static str) -> Result<(), StoreError> {
        self.writes += 1;
        if self.fail_on_write == Some(self.writes) {
            return Err(StoreError::Io {
                op,
                message: "injected mid-transaction write fault".into(),
            });
        }
        Ok(())
    }
}

impl ReadView for FaultTxn {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.working.get(key).cloned())
    }
    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<StoreCell>, StoreError> {
        Ok(scan_map(&self.working, prefix, cursor))
    }
}

impl WriteTxn for FaultTxn {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.maybe_fault("put")?;
        self.working.insert(key.to_vec(), value);
        Ok(())
    }
    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.maybe_fault("remove")?;
        self.working.remove(key);
        Ok(())
    }
    fn commit(self) -> CommitOutcome {
        let FaultTxn {
            base,
            working,
            mode,
            ..
        } = self;
        match mode {
            Mode::Confirm => {
                *base.borrow_mut() = working;
                CommitOutcome::Confirmed
            }
            // Persist then misreport: the in-memory swap lands the bytes (including the
            // witness cell the kernel staged), but the verdict hides that.
            Mode::IndeterminatePersist => {
                *base.borrow_mut() = working;
                CommitOutcome::Indeterminate
            }
            // Drop the working copy: nothing lands.
            Mode::IndeterminateDrop => CommitOutcome::Indeterminate,
            Mode::Abort => CommitOutcome::Aborted,
        }
    }
}

impl ByteEngine for FaultEngine {
    type View<'a> = FaultView;
    type Txn<'a> = FaultTxn;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        Ok(FaultView {
            entries: self.inner.borrow().clone(),
        })
    }
    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        Ok(FaultTxn {
            base: Rc::clone(&self.inner),
            working: self.inner.borrow().clone(),
            mode: self.mode.get(),
            fail_on_write: self.write_fault.get(),
            writes: 0,
        })
    }
    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        let _ = op;
        Ok(())
    }
    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        Ok(())
    }
}

pub(super) fn schema() -> StoreSchema {
    let mut builder = StoreSchemaBuilder::root("counters", vec![ScalarKind::Str]);
    builder.scalar_field("value", ScalarKind::Int, true);
    builder.finish().expect("a bounded schema builds")
}

pub(super) fn sites() -> Vec<SiteTarget> {
    vec![SiteTarget::whole_payload(), SiteTarget::field_leaf(0)]
}

pub(super) fn write() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

/// Stage one entry and commit it. The session is scoped so its mutable borrow of the
/// store ends here, freeing the store for affine recovery classification.
pub(super) fn unscoped_store(engine: FaultEngine) -> DurableStore<FaultEngine> {
    DurableStore::from_engine(engine, project(&schema(), sites()))
}
