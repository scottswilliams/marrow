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
    Reopen, SiteSpec, SiteTarget, StoreSchema,
};

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

/// A byte engine that delegates to an in-memory backend but resolves each commit per a
/// test-chosen [`Mode`].
struct FaultEngine {
    inner: MemoryEngine,
    mode: ModeHandle,
}

impl FaultEngine {
    fn new(mode: ModeHandle) -> Self {
        Self {
            inner: MemoryEngine::new(),
            mode,
        }
    }
}

/// The double's transaction: the backend transaction plus the mode captured at `begin`.
struct FaultTxn<'a> {
    inner: <MemoryEngine as ByteEngine>::Txn<'a>,
    mode: Mode,
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
        self.inner.put(key, value)
    }
    fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
        self.inner.remove(key)
    }
    fn commit(self) -> CommitOutcome {
        let FaultTxn { inner, mode } = self;
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
        key: ScalarKind::Str,
        fields: vec![FieldSchema {
            name: "value".into(),
            kind: ScalarKind::Int,
            required: true,
        }],
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
        fields: vec![Some(RuntimeScalar::Int(v))],
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
    txn.create_entry(&site, KeyScalar::Str(key.into()), entry(v))
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
        read.read_field(&site, KeyScalar::Str("a".into())),
        Ok(None),
        "the aborted write is not present"
    );
    assert_eq!(
        read.read_field(&site, KeyScalar::Str("b".into())),
        Ok(Some(RuntimeScalar::Int(2))),
        "the post-abort commit is present"
    );
}
