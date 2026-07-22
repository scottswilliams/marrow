//! Indeterminate commit recovery is affine, exact, and never retried.
//!
//! A durable commit resolves to exactly one of confirmed, aborted, or indeterminate.
//! A confirmed commit reports `Committed`; a clean abort reports `Aborted` and leaves
//! the store usable. An indeterminate commit latches the poison flag and returns one
//! opaque non-cloneable recovery fact owning the exact before and proposed-after
//! witness states. Consuming that fact classifies known-new, known-old, or unknown.
//!
//! These drive the production kernel commit path (`TxnSession::commit`, the exact call
//! `marrow-vm` issues for `TxnCommit`) through a fault-injecting engine double whose
//! transaction reports a chosen [`CommitOutcome`] while independently either persisting
//! or discarding the staged bytes — so the classify path is exercised both ways.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ops::Bound;
use std::rc::Rc;

use marrow_image::{
    DurableMemberDef, DurableValueShape, ExportId, FieldDef, FunctionDef, ImageDraft, ImageType,
    Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity, Scalar, SemanticPath,
    SemanticStep, SemanticStepKind, SiteDef, SpanEntry,
};
use marrow_store::{ByteEngine, Cell as StoreCell, CommitOutcome, ReadView, StoreError, WriteTxn};
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::{DurableExecutionFault, IncompleteDisposition, Value, run_durable};

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    CommitResult, DemandCoverage, Durable, DurableCommitState, DurableStore, EntryValue,
    FieldSchema, InvocationGrant, KernelFault, NativeStore, SessionError, SiteSpec, SiteTarget,
    StoreSchema,
};
use marrow_kernel::equality::ValueDomain;

const APPLICATION_ID: [u8; 16] = [0x91; 16];
const ROOT_PLACEMENT_ID: [u8; 16] = [0x92; 16];
const ROOT_PRODUCT_ID: [u8; 16] = [0x93; 16];
const ROOT_KEY_ID: [u8; 16] = [0x94; 16];
const VALUE_FIELD_ID: [u8; 16] = [0x95; 16];

fn vm_root_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
        ),
    ])
}

fn vm_spans(code: &[Instr]) -> Vec<SpanEntry> {
    code.iter()
        .enumerate()
        .map(|(index, _)| SpanEntry {
            instr_index: index as u32,
            line: 20 + index as u32,
            column: 4,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum VmWrite {
    Create,
    SetRequired,
}

fn vm_commit_image(write: VmWrite) -> VerifiedImage {
    let mut draft = ImageDraft::new();
    let record_name = draft.intern_string("Counter");
    let field_name = draft.intern_string("value");
    let record = draft.add_record_type(RecordTypeDef {
        name: record_name,
        fields: vec![FieldDef {
            name: field_name,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let root_name = draft.intern_string("counters");
    draft.add_root(RootDef {
        name: root_name,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes(ROOT_PRODUCT_ID),
            indexes: Vec::new(),
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes(VALUE_FIELD_ID),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });
    let entry_site = draft
        .add_site(SiteDef::whole_payload(vm_root_path()))
        .index();
    let mut field_path = vm_root_path().steps().to_vec();
    field_path.push(SemanticStep::new(
        SemanticStepKind::Field,
        LedgerIdBytes::from_bytes(VALUE_FIELD_ID),
    ));
    let field_site = draft
        .add_site(SiteDef::field_leaf(SemanticPath::from_steps(field_path)))
        .index();
    let key = draft.intern_text("vm");
    let value = draft.intern_int(7);
    let mut code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(key.index()),
        Instr::ConstLoad(value.index()),
    ];
    match write {
        VmWrite::Create => {
            code.push(Instr::RecordNew(record.index()));
            code.push(Instr::DurCreateEntry(entry_site));
        }
        VmWrite::SetRequired => code.push(Instr::DurSetRequired(field_site)),
    }
    code.extend([Instr::TxnCommit, Instr::Return]);
    let name = draft.intern_string("write");
    let source = draft.intern_string("src/main.mw");
    let function = draft.add_function(FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: vm_spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "write"), function);
    verify(&draft.encode().expect("encode VM fixture").bytes).expect("verify VM fixture")
}

fn run_vm_write(
    store: &mut DurableStore<FaultEngine>,
    image: &VerifiedImage,
) -> Result<Option<Value>, DurableExecutionFault> {
    let export = image
        .export_by_id(ExportId::of_local("", "write"))
        .expect("write export");
    let demand = DemandCoverage {
        read: export.demand().reads(),
        write: export.demand().writes(),
    };
    let mut session = store
        .txn_session(InvocationGrant::full_store(), demand)
        .expect("VM transaction session");
    run_durable(image, export.function(), Vec::new(), &mut session)
}

/// What the double's next `commit` does: the reported durability verdict is chosen
/// independently of whether the staged bytes actually land, so an indeterminate verdict
/// can accompany either a persisted or a discarded write.
#[derive(Clone, Copy, Debug)]
enum Mode {
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
#[derive(Clone)]
struct FaultEngine {
    inner: Rc<RefCell<BTreeMap<Vec<u8>, Vec<u8>>>>,
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
            inner: Rc::new(RefCell::new(BTreeMap::new())),
            mode,
            write_fault,
        }
    }
}

/// An owned coherent snapshot. The test engine shares durable bytes across separately
/// constructed handles so recovery KATs can actually drop the poisoned handle and reopen;
/// cloning the map here keeps each read view stable for its lifetime.
struct FaultView {
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
struct FaultTxn {
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

fn schema() -> StoreSchema {
    StoreSchema {
        root_name: "counters".into(),
        key: vec![ScalarKind::Str],
        fields: vec![FieldSchema::scalar("value", ScalarKind::Int, true)],
        branches: Vec::new(),
        groups: Vec::new(),
        indexes: Vec::new(),
    }
}

fn sites() -> Vec<SiteSpec> {
    vec![
        SiteSpec {
            root: 0,
            target: SiteTarget::WholePayload,
        },
        SiteSpec {
            root: 0,
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
        groups: Vec::new(),
        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(v)))],
    }
}

/// Stage one entry and commit it. The session is scoped so its mutable borrow of the
/// store ends here, freeing the store for affine recovery classification.
fn unscoped_store(engine: FaultEngine) -> DurableStore<FaultEngine> {
    DurableStore::from_engine(engine, schema(), sites())
}

#[test]
fn scoped_native_reopen_leaves_a_missing_engine_path_absent() {
    let dir = std::env::temp_dir().join(format!(
        "marrow-kernel-existing-reopen-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let path = dir.join("store.redb");
    assert!(
        NativeStore::open_native_with_recovery_scope(&path, vec![schema()], sites(), [0x61; 16],)
            .is_err(),
        "a scoped lifecycle reopen must refuse a missing engine",
    );
    assert!(
        !path.exists(),
        "a scoped lifecycle reopen must never create the missing engine path",
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn commit_one(store: &mut DurableStore<FaultEngine>, key: &str, v: i64) -> CommitResult {
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let site = txn.site(0);
    txn.create_entry(&site, &[KeyScalar::Str(key.into())], entry(v))
        .expect("create");
    txn.commit()
}

/// An indeterminate commit returns one affine fact, latches poison, and the latch is
/// consulted at session open (E02 residue, F02a): every later session open — read or
/// write — refuses with [`SessionError::Poisoned`], so a poisoned handle can neither
/// replay a commit nor observe its own indeterminate state. Recovery consumes the fact and
/// classifies exact before/after state.
#[test]
fn an_indeterminate_commit_poisons_and_every_later_session_open_refuses() {
    let mode = ModeHandle::new(Mode::IndeterminateDrop);
    let mut store = unscoped_store(FaultEngine::new(mode.clone()));

    let recovery = match commit_one(&mut store, "a", 1) {
        CommitResult::Indeterminate(recovery) => recovery,
        other => panic!("expected an indeterminate result, got {other:?}"),
    };
    assert!(
        store.has_unresolved_recovery(),
        "the lifecycle-visible poison latch must be set before the affine fact can be lost",
    );

    // A "retry" would be a fresh, well-formed transaction. Even with the double flipped to
    // confirm, the latch consulted at open refuses the transaction session outright — no
    // replay, only a reopen — and a read session is refused for the same reason: the
    // store's state is indeterminate until reclassified.
    mode.set(Mode::Confirm);
    assert!(
        matches!(
            store.txn_session(InvocationGrant::full_store(), write()),
            Err(SessionError::Poisoned)
        ),
        "a poisoned handle refuses a later transaction open rather than replaying",
    );
    assert!(
        matches!(
            store.read_session(InvocationGrant::full_store(), write()),
            Err(SessionError::Poisoned)
        ),
        "a poisoned handle refuses a later read open until a reopen reclassifies",
    );
    assert_eq!(
        store.resolve_recovery(recovery),
        DurableCommitState::Unknown,
        "the poisoned engine itself is not a fresh durable observation",
    );
    assert!(matches!(
        store.read_session(InvocationGrant::full_store(), write()),
        Err(SessionError::Poisoned),
    ));
}

/// A transaction's commit boundary is one-shot even though the VM-facing trait takes
/// `&mut self`: once the engine has returned an indeterminate verdict, the same session
/// cannot manufacture a later known-old result. The first call retains sole ownership of
/// the affine recovery fact and the store remains poisoned; the repeated call reports only
/// that this session has already crossed its terminal boundary.
#[test]
fn repeating_an_indeterminate_commit_never_reclassifies_it_as_aborted() {
    let mode = ModeHandle::new(Mode::IndeterminateDrop);
    let mut store = unscoped_store(FaultEngine::new(mode));
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write())
        .expect("txn session");
    let site = txn.site(0);
    txn.create_entry(&site, &[KeyScalar::Str("a".into())], entry(1))
        .expect("create");

    let recovery = match txn.commit() {
        CommitResult::Indeterminate(recovery) => recovery,
        other => panic!("expected an indeterminate result, got {other:?}"),
    };
    assert!(matches!(txn.commit(), CommitResult::SessionFinished));
    drop(txn);

    assert!(store.has_unresolved_recovery());
    assert_eq!(
        store.resolve_recovery(recovery),
        DurableCommitState::Unknown,
    );
}

/// A custom engine entering through the ordinary public constructor has no persistent
/// lifecycle provenance. Its affine fact remains truthful — the originating handle is
/// poisoned and the fact is consumed — but neither the persisted nor dropped half may claim
/// `KnownNew` or `KnownOld` against a later public handle.
#[test]
fn unscoped_indeterminate_facts_never_claim_persistent_classification() {
    for mode in [Mode::IndeterminatePersist, Mode::IndeterminateDrop] {
        let backing = FaultEngine::new(ModeHandle::new(mode));
        let mut store = unscoped_store(backing.clone());
        let recovery = match commit_one(&mut store, "a", 1) {
            CommitResult::Indeterminate(recovery) => recovery,
            other => panic!("expected an indeterminate result, got {other:?}"),
        };
        assert!(store.has_unresolved_recovery());
        drop(store);

        let mut reopened = unscoped_store(backing);
        assert_eq!(
            reopened.resolve_recovery(recovery),
            DurableCommitState::Unknown,
            "an unscoped custom-engine fact must not claim persistent provenance",
        );
        assert!(matches!(
            reopened.read_session(InvocationGrant::full_store(), write()),
            Err(SessionError::Poisoned),
        ));
    }
}

#[test]
fn vm_preserves_confirmed_aborted_and_pending_commit_outcomes() {
    let image = vm_commit_image(VmWrite::Create);
    let export = image
        .export_by_id(ExportId::of_local("", "write"))
        .expect("write export");
    let demand = DemandCoverage {
        read: export.demand().reads(),
        write: export.demand().writes(),
    };

    let mut confirmed = unscoped_store(FaultEngine::new(ModeHandle::new(Mode::Confirm)));
    let mut confirmed_session = confirmed
        .txn_session(InvocationGrant::full_store(), demand)
        .expect("confirmed transaction session");
    assert!(matches!(
        run_durable(
            &image,
            export.function(),
            Vec::new(),
            &mut confirmed_session,
        ),
        Ok(None)
    ));
    drop(confirmed_session);
    assert!(!confirmed.has_unresolved_recovery());

    let mut aborted = unscoped_store(FaultEngine::new(ModeHandle::new(Mode::Abort)));
    let mut aborted_session = aborted
        .txn_session(InvocationGrant::full_store(), demand)
        .expect("aborted transaction session");
    let aborted_fault = run_durable(&image, export.function(), Vec::new(), &mut aborted_session)
        .expect_err("an aborted commit cannot complete the invocation");
    drop(aborted_session);
    let DurableExecutionFault::Incomplete(aborted_incomplete) = aborted_fault else {
        panic!("an aborted commit was flattened to an ordinary runtime fault");
    };
    match aborted_incomplete.into_disposition() {
        IncompleteDisposition::Classified { fault, durable } => {
            assert_eq!(fault.code(), "run.commit");
            assert_eq!(durable, DurableCommitState::KnownOld);
        }
        IncompleteDisposition::Pending { .. } => {
            panic!("an aborted engine commit must not mint a recovery fact");
        }
    }
    assert!(!aborted.has_unresolved_recovery());

    for mode in [Mode::IndeterminatePersist, Mode::IndeterminateDrop] {
        let mut pending = unscoped_store(FaultEngine::new(ModeHandle::new(mode)));
        let mut pending_session = pending
            .txn_session(InvocationGrant::full_store(), demand)
            .expect("indeterminate transaction session");
        let pending_fault =
            run_durable(&image, export.function(), Vec::new(), &mut pending_session)
                .expect_err("an indeterminate commit cannot complete the invocation");
        drop(pending_session);
        let DurableExecutionFault::Incomplete(pending_incomplete) = pending_fault else {
            panic!("an indeterminate commit was flattened to an ordinary runtime fault");
        };
        match pending_incomplete.into_disposition() {
            IncompleteDisposition::Pending { fault, recovery } => {
                assert_eq!(fault.code(), "run.commit");
                assert!(pending.has_unresolved_recovery());
                drop(recovery);
            }
            IncompleteDisposition::Classified { .. } => {
                panic!("an indeterminate engine result was classified inside the VM");
            }
        }
    }
}

/// The production VM path preserves the distinction between an ordinary operation-stage
/// failure and known-old failures while preparing a commit. These use the same private
/// fault engine through `DurableStore::from_engine` as the commit-outcome matrix above;
/// neither failure poisons the handle, and a later independent invocation can commit.
#[test]
fn vm_preserves_staging_reconcile_and_witness_failures_without_poisoning() {
    // A create plan writes the entry marker then its value leaf. Failing the value write
    // happens before TxnCommit and remains an ordinary typed runtime fault.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let image = vm_commit_image(VmWrite::Create);
        write_fault.set(Some(2));
        let fault = run_vm_write(&mut store, &image).expect_err("stage write must fault");
        let DurableExecutionFault::Runtime(fault) = fault else {
            panic!("a pre-commit stage failure became invocation-incomplete");
        };
        assert_eq!(fault.code(), "store.io");
        assert!(!store.has_unresolved_recovery());

        write_fault.set(None);
        assert!(matches!(run_vm_write(&mut store, &image), Ok(None)));
    }

    // The third create write is the witness cell. Its failure aborts before the engine
    // commit, so the VM reports incomplete/known-old without minting a recovery fact.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let image = vm_commit_image(VmWrite::Create);
        write_fault.set(Some(3));
        let fault = run_vm_write(&mut store, &image).expect_err("witness write must fault");
        let DurableExecutionFault::Incomplete(incomplete) = fault else {
            panic!("a witness-put abort was flattened to an ordinary runtime fault");
        };
        match incomplete.into_disposition() {
            IncompleteDisposition::Classified { fault, durable } => {
                assert_eq!(fault.code(), "run.commit");
                assert_eq!(durable, DurableCommitState::KnownOld);
            }
            IncompleteDisposition::Pending { .. } => {
                panic!("a pre-engine witness-put failure minted a recovery fact");
            }
        }
        assert!(!store.has_unresolved_recovery());

        write_fault.set(None);
        assert!(matches!(run_vm_write(&mut store, &image), Ok(None)));
    }

    // A required-field write produces a markerless staged entry. Reconcile's second write
    // supplies the absent marker; failing it is likewise a pre-engine known-old outcome.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let image = vm_commit_image(VmWrite::SetRequired);
        write_fault.set(Some(2));
        let fault = run_vm_write(&mut store, &image).expect_err("reconcile write must fault");
        let DurableExecutionFault::Incomplete(incomplete) = fault else {
            panic!("a reconcile abort was flattened to an ordinary runtime fault");
        };
        match incomplete.into_disposition() {
            IncompleteDisposition::Classified { fault, durable } => {
                assert_eq!(fault.code(), "run.commit");
                assert_eq!(durable, DurableCommitState::KnownOld);
            }
            IncompleteDisposition::Pending { .. } => {
                panic!("a pre-engine reconcile failure minted a recovery fact");
            }
        }
        assert!(!store.has_unresolved_recovery());

        write_fault.set(None);
        assert!(matches!(run_vm_write(&mut store, &image), Ok(None)));
    }
}

/// A clean abort faults the commit but leaves the store unpoisoned: a subsequent
/// well-formed transaction commits. This pins the `Aborted` arm distinct from
/// `Indeterminate` — the E00-review-validated mapping.
#[test]
fn a_clean_abort_faults_without_poisoning() {
    let mode = ModeHandle::new(Mode::Abort);
    let mut store = unscoped_store(FaultEngine::new(mode.clone()));

    let aborted = commit_one(&mut store, "a", 1);
    assert!(matches!(aborted, CommitResult::Aborted));

    // Not poisoned: a later commit succeeds where a poisoned store would have faulted.
    mode.set(Mode::Confirm);
    let next = commit_one(&mut store, "b", 2);
    assert!(
        matches!(next, CommitResult::Committed),
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

/// The witness put shares the staged data's transaction: if it fails before the engine
/// commit, the result is known-old `Aborted` and the transaction rolls back. Fail the third
/// write — after the created entry's marker and value leaf — then prove the handle stays
/// usable and the staged entry did not land.
#[test]
fn a_witness_put_failure_is_known_old_and_leaves_the_handle_usable() {
    let mode = ModeHandle::new(Mode::Confirm);
    let write_fault = WriteFaultHandle::inert();
    let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));

    let seeded = commit_one(&mut store, "a", 1);
    assert!(matches!(seeded, CommitResult::Committed));
    // Prior state is present on the healthy handle, before the poisoning fault.
    assert_eq!(
        read_value(&mut store, "a"),
        Some(ValueDomain::Scalar(RuntimeScalar::Int(1)))
    );

    // In the next commit: write 1 = marker put, write 2 = value-leaf put, write 3 = the
    // witness put. Fail the witness put.
    write_fault.set(Some(3));
    let faulted = commit_one(&mut store, "b", 2);
    assert!(matches!(faulted, CommitResult::Aborted));

    // The witness put failed before the engine commit, so dropping the transaction proves
    // known-old and the handle remains usable.
    write_fault.set(None);
    assert!(matches!(
        commit_one(&mut store, "c", 3),
        CommitResult::Committed
    ));
    assert_eq!(read_value(&mut store, "b"), None);
}

/// Reconcile writes an absent marker for a markerless entry whose required fields are all
/// staged. If that marker put fails, the result is known-old `Aborted` and rolls back. Stage a required field
/// through `set_required` (which stages a leaf but no marker) and fail reconcile's marker
/// put — the second write of the commit, after the leaf — then confirm a later session can
/// commit and the partially staged entry is absent.
#[test]
fn a_reconcile_marker_put_failure_is_known_old_and_leaves_the_handle_usable() {
    let mode = ModeHandle::new(Mode::Confirm);
    let write_fault = WriteFaultHandle::inert();
    let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));

    let seeded = commit_one(&mut store, "a", 1);
    assert!(matches!(seeded, CommitResult::Committed));

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
    assert!(matches!(faulted, CommitResult::Aborted));

    // The reconcile write failed before commit, so dropping the transaction proves known-old
    // and a later session may proceed.
    write_fault.set(None);
    assert!(matches!(
        commit_one(&mut store, "c", 3),
        CommitResult::Committed
    ));
    assert_eq!(read_value(&mut store, "b"), None);
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
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let seeded = commit_one(&mut store, "a", 1);
        assert!(matches!(seeded, CommitResult::Committed));

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
        let next = commit_one(&mut store, "c", 3);
        assert!(
            matches!(next, CommitResult::Committed),
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
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let seeded = commit_one(&mut store, "a", 1);
        assert!(matches!(seeded, CommitResult::Committed));

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
