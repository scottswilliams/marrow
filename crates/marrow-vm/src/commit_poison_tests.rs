//! The production VM commit path over a fault-injecting engine: `TxnSession::commit` — the
//! exact call the VM issues for `TxnCommit` — resolves to confirmed, aborted, or
//! indeterminate, and the VM preserves each as its typed outcome, distinguishing a
//! pre-commit staging fault from a known-old commit failure and never classifying an
//! indeterminate result itself. Private to the VM so the executor is driven against an
//! injected engine without any public bare-host execution route.

mod key_domains;
mod required_reads;

use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId, FieldDef, FunctionDef,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootOccurrenceDef,
    Scalar, SemanticTarget, SpanEntry, TypeId,
};
use marrow_kernel::durable::{
    DemandCoverage, DurableCommitState, DurableStore, InvocationGrant, SessionError,
};
use marrow_store::{ByteEngine, StoreError};
use marrow_verify::{VerifiedImage, verify};

use crate::fault::{DurableExecutionFault, IncompleteDisposition};
use crate::run::run_durable;
use crate::value::Value;

use crate::admitted_plan::admitted_plan;

#[path = "../../marrow-kernel/tests/common/fault_engine.rs"]
mod fault_engine;
use fault_engine::{FaultEngine, Mode, ModeHandle, WriteFaultHandle, unscoped_store, write};

const APPLICATION_ID: [u8; 16] = [0x91; 16];
const ROOT_PLACEMENT_ID: [u8; 16] = [0x92; 16];
const ROOT_PRODUCT_ID: [u8; 16] = [0x93; 16];
const ROOT_KEY_ID: [u8; 16] = [0x94; 16];
const VALUE_FIELD_ID: [u8; 16] = [0x95; 16];

// The real engine outlives each store borrow, allowing byte damage between valid
// seeding and execution without exposing the kernel's physical-key constructors.
struct BorrowedEngine<'a, E>(&'a mut E);

impl<E: ByteEngine> ByteEngine for BorrowedEngine<'_, E> {
    type View<'a>
        = E::View<'a>
    where
        Self: 'a;
    type Txn<'a>
        = E::Txn<'a>
    where
        Self: 'a;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        self.0.read_view()
    }
    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        self.0.begin()
    }
    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        self.0.require_write_access(op)
    }
    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.0.audit_integrity()
    }
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

fn required_int_record(draft: &mut DraftTxn<'_>, name: &str) -> TypeId {
    let name = draft.intern_string(name).expect("a within-domain mint");
    let field_name = draft.intern_string("value").expect("a within-domain mint");
    draft
        .add_record_type(RecordTypeDef {
            name,
            fields: vec![FieldDef {
                name: field_name,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        })
        .expect("a within-domain mint")
}

/// The encoded fixture image; every consumer seals it through the verifier.
fn vm_commit_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let record = required_int_record(&mut draft, "Counter");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let root_name = draft
        .intern_string("counters")
        .expect("a within-domain mint");
    let product = LedgerIdBytes::from_bytes(ROOT_PRODUCT_ID);
    let value = draft
        .value_scalar(Scalar::Int)
        .expect("the test arena mints");
    draft
        .declare_product(
            &admitted_plan(),
            product,
            record,
            vec![DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Field {
                    id: LedgerIdBytes::from_bytes(VALUE_FIELD_ID),
                    required: true,
                    value,
                },
            }],
        )
        .expect("a well-formed declaration");
    let counters = draft
        .add_root_occurrence(
            &admitted_plan(),
            product,
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Text,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let entry_handle = draft
        .bind_occurrence_site(
            counters.occurrence(),
            counters.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("the root placement is a canonical path of this occurrence");
    let entry_site = draft
        .request_site(&entry_handle)
        .expect("the binding is live");
    let key = draft.intern_text("vm").expect("a within-domain mint");
    let value = draft.intern_int(7).expect("a within-domain mint");
    let code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(key),
        Instr::ConstLoad(value),
        Instr::RecordNew(record),
        Instr::DurCreateEntry(entry_site),
        Instr::TxnCommit,
        Instr::Return,
    ];
    let name = draft.intern_string("write").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            spans: vm_spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "write"), function);
    draft.encode().expect("encode VM fixture").bytes
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

#[test]
fn vm_preserves_confirmed_aborted_and_pending_commit_outcomes() {
    let image = verify(&vm_commit_image()).expect("verify VM fixture");
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
    confirmed
        .read_session(InvocationGrant::full_store(), demand)
        .expect("confirmed handle remains usable");

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
    aborted
        .read_session(InvocationGrant::full_store(), demand)
        .expect("aborted handle remains usable");

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
                assert!(matches!(
                    pending.read_session(InvocationGrant::full_store(), demand),
                    Err(SessionError::Poisoned),
                ));
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
fn vm_preserves_staging_and_witness_failures_without_poisoning() {
    // A create plan writes the entry marker then its value leaf. Failing the value write
    // happens before TxnCommit and remains an ordinary typed runtime fault.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let image = verify(&vm_commit_image()).expect("verify VM fixture");
        write_fault.set(Some(2));
        let fault = run_vm_write(&mut store, &image).expect_err("stage write must fault");
        let DurableExecutionFault::Runtime(fault) = fault else {
            panic!("a pre-commit stage failure became invocation-incomplete");
        };
        assert_eq!(fault.code(), "store.io");
        store
            .read_session(InvocationGrant::full_store(), write())
            .expect("a staging fault does not poison the handle");

        write_fault.set(None);
        assert!(matches!(run_vm_write(&mut store, &image), Ok(None)));
    }

    // The third create write is the witness cell. Its failure aborts before the engine
    // commit, so the VM reports incomplete/known-old without minting a recovery fact.
    {
        let mode = ModeHandle::new(Mode::Confirm);
        let write_fault = WriteFaultHandle::inert();
        let mut store = unscoped_store(FaultEngine::with_write_fault(mode, write_fault.clone()));
        let image = verify(&vm_commit_image()).expect("verify VM fixture");
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
        store
            .read_session(InvocationGrant::full_store(), write())
            .expect("a witness-put failure does not poison the handle");

        write_fault.set(None);
        assert!(matches!(run_vm_write(&mut store, &image), Ok(None)));
    }
}
