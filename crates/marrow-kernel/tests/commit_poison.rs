//! Indeterminate commit poisons the store; a reopen classifies, and nothing retries.
//!
//! A durable commit resolves to exactly one of confirmed, aborted, or indeterminate.
//! A confirmed commit reports `Committed`; a clean abort reports `CommitFault` and
//! leaves the store usable; an indeterminate commit — durability unknown — reports
//! `CommitFault`, latches the store's poison flag, and is never replayed. The only
//! recovery is a reopen that reads the intended witness token: present means the
//! commit's writes landed (complete-new), absent means they did not (complete-old).
//!
//! These drive the production kernel commit path (`TxnSession::commit`, the exact call
//! `marrow-vm` issues for `TxnCommit`) through a fault-injecting engine double whose
//! transaction reports a chosen [`CommitOutcome`] while independently either persisting
//! or discarding the staged bytes — so the classify path is exercised both ways.

use std::cell::Cell;
use std::rc::Rc;

use marrow_store::{
    ByteEngine, Cell as StoreCell, CommitOutcome, MemoryEngine, ReadView, StoreError, WriteTxn,
};

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    CommitResult, DemandCoverage, Durable, DurableStore, EntryValue, FieldSchema, InvocationGrant,
    KernelFault, Reopen, SiteSpec, SiteTarget, StoreSchema,
};
use marrow_kernel::equality::ValueDomain;

/// What the double's next `commit` does: the reported durability verdict is chosen
/// independently of whether the staged bytes actually land, so an indeterminate verdict
/// can accompany either a persisted or a discarded write.
#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Persist the staged bytes and report `Confirmed` (the honest in-memory path).
    Confirm,
    /// Persist the staged bytes but report `Indeterminate`: the write landed, yet the
    /// caller cannot know it — a reopen must classify complete-new.
    IndeterminatePersist,
    /// Discard the staged bytes and report `Indeterminate`: the write did not land, and
    /// a reopen must classify complete-old.
    IndeterminateDrop,
    /// Discard the staged bytes and report `Aborted`: a clean abort that leaves the
    /// store unchanged and unpoisoned.
    Abort,
}

/// A shared, test-controlled commit mode. A transaction snapshots it at `begin`, so a
/// test flips the mode between sessions to model, e.g., a recovered store committing
/// cleanly after an earlier abort.
#[derive(Clone)]
struct ModeHandle(Rc<Cell<Mode>>);

impl ModeHandle {
    fn new(mode: Mode) -> Self {
        Self(Rc::new(Cell::new(mode)))
    }
    fn set(&self, mode: Mode) {
        self.0.set(mode);
    }
    fn get(&self) -> Mode {
        self.0.get()
    }
}

/// A test-controlled mid-transaction write fault. A transaction snapshots the target at
/// `begin`; the double then returns a [`StoreError`] from the Nth write op (`put` or
/// `remove`, counted together, 1-based) that transaction issues — modelling an engine
/// write that fails partway through a commit or an apply plan, before the transaction's
/// own `commit`. `None` never faults.
#[derive(Clone)]
struct WriteFaultHandle(Rc<Cell<Option<u32>>>);

impl WriteFaultHandle {
    fn inert() -> Self {
        Self(Rc::new(Cell::new(None)))
    }
    fn set(&self, target: Option<u32>) {
        self.0.set(target);
    }
    fn get(&self) -> Option<u32> {
        self.0.get()
    }
}

/// A byte engine that delegates to an in-memory backend but resolves each commit per a
/// test-chosen [`Mode`] and may fail a chosen mid-transaction write per a
/// [`WriteFaultHandle`].
struct FaultEngine {
    inner: MemoryEngine,
    mode: ModeHandle,
    write_fault: WriteFaultHandle,
}

impl FaultEngine {
    /// A double that only ever misreports the commit verdict; no write faults.
    fn new(mode: ModeHandle) -> Self {
        Self::with_write_fault(mode, WriteFaultHandle::inert())
    }
    /// A double that both resolves commit per `mode` and fails the write `write_fault`
    /// selects, so a test can exercise an engine write that fails mid-transaction.
    fn with_write_fault(mode: ModeHandle, write_fault: WriteFaultHandle) -> Self {
        Self {
            inner: MemoryEngine::new(),
            mode,
            write_fault,
        }
    }
}

/// The double's transaction: the backend transaction plus the mode captured at `begin`
/// and the mid-transaction write fault (the 1-based write index to fail, and a running
/// count of the writes issued so far).
struct FaultTxn<'a> {
    inner: <MemoryEngine as ByteEngine>::Txn<'a>,
    mode: Mode,
    fail_on_write: Option<u32>,
    writes: u32,
}

impl FaultTxn<'_> {
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

impl ReadView for FaultTxn<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.get(key)
    }
    fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<StoreCell>, StoreError> {
        self.inner.scan_after(prefix, cursor)
    }
}

impl WriteTxn for FaultTxn<'_> {
    fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
        self.maybe_fault("put")?;
        self.inner.put(key, value)
    }
    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.maybe_fault("remove")?;
        self.inner.remove(key)
    }
    fn commit(self) -> CommitOutcome {
        let FaultTxn { inner, mode, .. } = self;
        match mode {
            Mode::Confirm => inner.commit(),
            // Persist then misreport: the in-memory swap lands the bytes (including the
            // witness cell the kernel staged), but the verdict hides that.
            Mode::IndeterminatePersist => {
                let _ = inner.commit();
                CommitOutcome::Indeterminate
            }
            // Drop the working copy: nothing lands.
            Mode::IndeterminateDrop => {
                drop(inner);
                CommitOutcome::Indeterminate
            }
            Mode::Abort => {
                drop(inner);
                CommitOutcome::Aborted
            }
        }
    }
}

impl ByteEngine for FaultEngine {
    type View<'a> = <MemoryEngine as ByteEngine>::View<'a>;
    type Txn<'a> = FaultTxn<'a>;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        self.inner.read_view()
    }
    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        Ok(FaultTxn {
            inner: self.inner.begin()?,
            mode: self.mode.get(),
            fail_on_write: self.write_fault.get(),
            writes: 0,
        })
    }
    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        self.inner.require_write_access(op)
    }
    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.inner.audit_integrity()
    }
}

fn schema() -> StoreSchema {
    StoreSchema {
        root_name: "counters".into(),
        key: vec![ScalarKind::Str],
        fields: vec![FieldSchema::scalar("value", ScalarKind::Int, true)],
        branches: Vec::new(),
    }
}

fn sites() -> Vec<SiteSpec> {
    vec![
        SiteSpec {
            target: SiteTarget::WholePayload,
        },
        SiteSpec {
            target: SiteTarget::FieldLeaf(0),
        },
    ]
}

fn write() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

fn entry(v: i64) -> EntryValue {
    EntryValue {
        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(v)))],
    }
}

/// Stage one entry and commit it, returning the session's witness token and the commit
/// result. The session is scoped so its mutable borrow of the store ends here, freeing
/// the store for a later `classify` read.
fn commit_one(
    store: &mut DurableStore<FaultEngine>,
    key: &str,
    v: i64,
) -> ([u8; 16], CommitResult) {
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let site = txn.site(0);
    txn.create_entry(&site, &[KeyScalar::Str(key.into())], entry(v))
        .expect("create");
    let token = txn.token();
    let result = txn.commit();
    (token, result)
}

/// An indeterminate commit reports `CommitFault`, latches poison, and makes every later
/// commit fault — there is no retry, only a reopen. `CommitResult::CommitFault` is the
/// value `marrow-vm` maps to the `run.commit` fault.
#[test]
fn an_indeterminate_commit_poisons_and_every_later_commit_faults() {
    let mode = ModeHandle::new(Mode::IndeterminateDrop);
    let mut store = DurableStore::from_engine(FaultEngine::new(mode.clone()), schema(), sites());

    let (_token, result) = commit_one(&mut store, "a", 1);
    assert_eq!(result, CommitResult::CommitFault);

    // A "retry" would be a fresh, well-formed transaction committing cleanly. Even with
    // the double flipped to confirm, the latched poison refuses it: the only recovery is
    // a reopen, never a replay.
    mode.set(Mode::Confirm);
    let (_token2, retry) = commit_one(&mut store, "b", 2);
    assert_eq!(
        retry,
        CommitResult::CommitFault,
        "a poisoned store refuses every later commit rather than retrying"
    );
}

/// After an indeterminate commit whose write actually landed, a reopen reads the intended
/// witness and classifies complete-new.
#[test]
fn classify_reopens_complete_new_when_the_write_landed() {
    let mode = ModeHandle::new(Mode::IndeterminatePersist);
    let mut store = DurableStore::from_engine(FaultEngine::new(mode), schema(), sites());

    let (token, result) = commit_one(&mut store, "a", 1);
    assert_eq!(result, CommitResult::CommitFault);

    assert_eq!(
        store.classify(token).expect("classify reads the witness"),
        Reopen::CompleteNew,
        "the witness token landed, so the commit completed"
    );
}

/// After an indeterminate commit whose write was dropped, a reopen finds no witness and
/// classifies complete-old.
#[test]
fn classify_reopens_complete_old_when_the_write_dropped() {
    let mode = ModeHandle::new(Mode::IndeterminateDrop);
    let mut store = DurableStore::from_engine(FaultEngine::new(mode), schema(), sites());

    let (token, result) = commit_one(&mut store, "a", 1);
    assert_eq!(result, CommitResult::CommitFault);

    assert_eq!(
        store.classify(token).expect("classify reads the witness"),
        Reopen::CompleteOld,
        "no witness landed, so the commit did not complete"
    );
}

/// A clean abort faults the commit but leaves the store unpoisoned: a subsequent
/// well-formed transaction commits. This pins the `Aborted` arm distinct from
/// `Indeterminate` — the E00-review-validated mapping.
#[test]
fn a_clean_abort_faults_without_poisoning() {
    let mode = ModeHandle::new(Mode::Abort);
    let mut store = DurableStore::from_engine(FaultEngine::new(mode.clone()), schema(), sites());

    let (_token, aborted) = commit_one(&mut store, "a", 1);
    assert_eq!(aborted, CommitResult::CommitFault);

    // Not poisoned: a later commit succeeds where a poisoned store would have faulted.
    mode.set(Mode::Confirm);
    let (_token2, next) = commit_one(&mut store, "b", 2);
    assert_eq!(
        next,
        CommitResult::Committed,
        "a clean abort leaves the store usable"
    );

    // And the aborted write never landed, while the later one did.
    let mut read = store
        .read_session(InvocationGrant::full_store(), write())
        .expect("read session");
    let site = read.site(1);
    assert_eq!(
        read.read_field(&site, &[KeyScalar::Str("a".into())]),
        Ok(None),
        "the aborted write is not present"
    );
    assert_eq!(
        read.read_field(&site, &[KeyScalar::Str("b".into())]),
        Ok(Some(ValueDomain::Scalar(RuntimeScalar::Int(2)))),
        "the post-abort commit is present"
    );
}

/// Read the `value` field of entry `key` on a settled store, scoping the read session so
/// its borrow ends before the caller drives the store again.
fn read_value(store: &mut DurableStore<FaultEngine>, key: &str) -> Option<ValueDomain> {
    let mut read = store
        .read_session(InvocationGrant::full_store(), write())
        .expect("read session");
    let site = read.site(1);
    read.read_field(&site, &[KeyScalar::Str(key.into())])
        .expect("field read")
}

/// The witness put shares the staged data's transaction (`store.rs` ~424-432): if it
/// fails, the commit poisons and reports `CommitFault`, and the transaction rolls back so
/// prior committed state survives. Fail the third write of the commit — after the created
/// entry's marker and value leaf, the witness put is next — and confirm the poison latch,
/// the fault, and the untouched prior entry.
#[test]
fn a_witness_put_failure_poisons_and_leaves_prior_state() {
    let mode = ModeHandle::new(Mode::Confirm);
    let write_fault = WriteFaultHandle::inert();
    let mut store = DurableStore::from_engine(
        FaultEngine::with_write_fault(mode, write_fault.clone()),
        schema(),
        sites(),
    );

    let (_token, seeded) = commit_one(&mut store, "a", 1);
    assert_eq!(seeded, CommitResult::Committed);

    // In the next commit: write 1 = marker put, write 2 = value-leaf put, write 3 = the
    // witness put. Fail the witness put.
    write_fault.set(Some(3));
    let (_token2, faulted) = commit_one(&mut store, "b", 2);
    assert_eq!(faulted, CommitResult::CommitFault);

    // Poisoned: a later well-formed commit faults rather than retrying.
    write_fault.set(None);
    let (_token3, later) = commit_one(&mut store, "c", 3);
    assert_eq!(
        later,
        CommitResult::CommitFault,
        "a witness-put failure poisons the store"
    );

    // The prior commit survives; neither the faulted nor the refused write landed.
    assert_eq!(
        read_value(&mut store, "a"),
        Some(ValueDomain::Scalar(RuntimeScalar::Int(1)))
    );
    assert_eq!(read_value(&mut store, "b"), None);
    assert_eq!(read_value(&mut store, "c"), None);
}

/// Reconcile writes an absent marker for a markerless entry whose required fields are all
/// staged (`store.rs` ~483-486). If that marker put fails, reconcile returns a fault, so
/// the commit poisons, reports `CommitFault`, and rolls back. Stage a required field
/// through `set_required` (which stages a leaf but no marker) and fail reconcile's marker
/// put — the second write of the commit, after the leaf.
#[test]
fn a_reconcile_marker_put_failure_poisons_and_leaves_prior_state() {
    let mode = ModeHandle::new(Mode::Confirm);
    let write_fault = WriteFaultHandle::inert();
    let mut store = DurableStore::from_engine(
        FaultEngine::with_write_fault(mode, write_fault.clone()),
        schema(),
        sites(),
    );

    let (_token, seeded) = commit_one(&mut store, "a", 1);
    assert_eq!(seeded, CommitResult::Committed);

    // Write 1 = the value-leaf put; write 2 = reconcile's marker put for the markerless
    // entry. Fail the marker put.
    write_fault.set(Some(2));
    let faulted = {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write())
            .expect("txn session");
        let value = txn.site(1);
        txn.set_required(
            &value,
            &[KeyScalar::Str("b".into())],
            ValueDomain::Scalar(RuntimeScalar::Int(2)),
        )
        .expect("stage required field");
        txn.commit()
    };
    assert_eq!(faulted, CommitResult::CommitFault);

    // Poisoned by the reconcile fault: a later well-formed commit still faults.
    write_fault.set(None);
    let (_token3, later) = commit_one(&mut store, "c", 3);
    assert_eq!(
        later,
        CommitResult::CommitFault,
        "a reconcile marker-put failure poisons the store"
    );

    assert_eq!(
        read_value(&mut store, "a"),
        Some(ValueDomain::Scalar(RuntimeScalar::Int(1)))
    );
    assert_eq!(read_value(&mut store, "b"), None);
    assert_eq!(read_value(&mut store, "c"), None);
}

/// An `apply` put or remove that fails mid-plan (`store.rs` ~646-660) faults the durable
/// op with `KernelFault::Engine` and does not commit, so the still-live transaction aborts
/// on drop and the prior committed state is intact. Exercises both the `Put` arm (a
/// partly-applied create) and the `Remove` arm (a partly-applied erase); in each the
/// second write of the plan fails, leaving one cell already staged in the working copy
/// that the abort must discard.
#[test]
fn an_apply_write_fault_faults_and_the_store_stays_abortable() {
    // Put arm: create writes marker (write 1) then value leaf (write 2); fail the leaf.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = DurableStore::from_engine(
            FaultEngine::with_write_fault(mode, write_fault.clone()),
            schema(),
            sites(),
        );
        let (_token, seeded) = commit_one(&mut store, "a", 1);
        assert_eq!(seeded, CommitResult::Committed);

        write_fault.set(Some(2));
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn session");
            let site = txn.site(0);
            let err = txn
                .create_entry(&site, &[KeyScalar::Str("b".into())], entry(2))
                .expect_err("the apply put fault surfaces");
            assert!(
                matches!(err, KernelFault::Engine(_)),
                "an engine write fault surfaces as KernelFault::Engine"
            );
            // Drop without commit: the transaction aborts, discarding the staged marker.
        }
        // The store took no permanent write: no poison (a later commit succeeds), the
        // prior entry is intact, and the partly-created entry never landed.
        write_fault.set(None);
        let (_token2, next) = commit_one(&mut store, "c", 3);
        assert_eq!(
            next,
            CommitResult::Committed,
            "an aborted apply leaves the store usable"
        );
        assert_eq!(
            read_value(&mut store, "a"),
            Some(ValueDomain::Scalar(RuntimeScalar::Int(1)))
        );
        assert_eq!(read_value(&mut store, "b"), None);
        assert_eq!(
            read_value(&mut store, "c"),
            Some(ValueDomain::Scalar(RuntimeScalar::Int(3)))
        );
    }

    // Remove arm: erase removes marker (write 1) then value leaf (write 2); fail the leaf
    // removal, so the marker is already removed in the working copy the abort discards.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = DurableStore::from_engine(
            FaultEngine::with_write_fault(mode, write_fault.clone()),
            schema(),
            sites(),
        );
        let (_token, seeded) = commit_one(&mut store, "a", 1);
        assert_eq!(seeded, CommitResult::Committed);

        write_fault.set(Some(2));
        {
            let mut txn = store
                .txn_session(InvocationGrant::full_store(), write())
                .expect("txn session");
            let site = txn.site(0);
            let err = txn
                .erase_entry(&site, &[KeyScalar::Str("a".into())])
                .expect_err("the apply remove fault surfaces");
            assert!(
                matches!(err, KernelFault::Engine(_)),
                "an engine remove fault surfaces as KernelFault::Engine"
            );
            // Drop without commit: the abort restores the partly-removed entry.
        }
        assert_eq!(
            read_value(&mut store, "a"),
            Some(ValueDomain::Scalar(RuntimeScalar::Int(1))),
            "the aborted erase left the prior entry intact"
        );
    }
}
