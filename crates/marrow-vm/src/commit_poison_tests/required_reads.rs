//! A verified strict field read over real engine bytes: a missing or malformed
//! required leaf faults at that instruction, and its earlier staged write aborts.

use super::{
    APPLICATION_ID, BorrowedEngine, ROOT_KEY_ID, ROOT_PLACEMENT_ID, ROOT_PRODUCT_ID,
    VALUE_FIELD_ID, admitted_plan, required_int_record, vm_spans,
};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, ExportId, FunctionDef, ImageDraft, ImageType,
    Instr, KeyColumn, LedgerIdBytes, RootOccurrenceDef, Scalar, SemanticTarget,
};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, encode_domain};
use marrow_kernel::durable::{
    CommitResult, DemandCoverage, Durable, DurableStore, EntryValue, InvocationGrant,
    NativeOpenAccess, Presence,
};
use marrow_kernel::equality::ValueDomain;
use marrow_lifecycle::{PreparedImage, prepare};
use marrow_store::{
    ByteEngine, Cell, CommitOutcome, MemoryEngine, NativeEngineOwner, ReadView, WriteTxn,
};
use marrow_verify::{SealedInstr, verify};

use crate::fault::DurableExecutionFault;
use crate::run::run_durable;
use crate::value::Value;

const SENTINEL: i64 = 876_543_219;
const OLD: i64 = 3;
const NEW: i64 = 7;

#[derive(Clone, Copy, Debug)]
enum Target {
    Root,
    Branch,
}

impl Target {
    fn keys(self) -> Vec<KeyScalar> {
        let mut keys = vec![KeyScalar::Str("target".into())];
        if matches!(self, Self::Branch) {
            keys.push(KeyScalar::Int(11));
        }
        keys
    }
}

#[derive(Clone, Copy, Debug)]
enum Damage {
    Healthy,
    Missing,
    Malformed,
}

struct Fixture {
    prepared: PreparedImage,
    earlier_site: u16,
    target_site: u16,
    strict_pc: u32,
}

fn fixture(target: Target) -> Fixture {
    let mut owner = ImageDraft::new();
    let savepoint = owner.savepoint();
    let mut draft = owner.begin_transaction(savepoint).expect("fresh draft");
    let record = required_int_record(&mut draft, "Counter");
    let child_record = required_int_record(&mut draft, "Counter.children");
    let root_name = draft.intern_string("counters").expect("root name");
    let child_name = draft.intern_string("children").expect("branch name");
    let int = draft.value_scalar(Scalar::Int).expect("int shape");
    let product = LedgerIdBytes::from_bytes(ROOT_PRODUCT_ID);
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            product,
            record,
            vec![
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(VALUE_FIELD_ID),
                        required: true,
                        value: int,
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Branch {
                        placement: LedgerIdBytes::from_bytes([0x96; 16]),
                        name: child_name,
                        record: child_record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Int,
                            id: LedgerIdBytes::from_bytes([0x97; 16]),
                        }],
                    },
                },
                DeclarationMemberDef {
                    parent: Some(1),
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes([0x98; 16]),
                        required: true,
                        value: int,
                    },
                },
            ],
        )
        .expect("complete required root and branch declaration");
    let root = draft
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
        .expect("declared root");
    let members = draft.product_members(product).expect("published members");
    let (entry_path, field_path) = match target {
        Target::Root => (root.placement_path().clone(), members[0].path().clone()),
        Target::Branch => (
            members[1].path().clone(),
            draft.members_of(members[1].path()).expect("branch members")[0]
                .path()
                .clone(),
        ),
    };
    let mut sites = Vec::new();
    for (path, target) in [
        (root.placement_path(), SemanticTarget::WholePayload),
        (&entry_path, SemanticTarget::WholePayload),
        (&field_path, SemanticTarget::FieldLeaf),
    ] {
        let handle = draft
            .bind_occurrence_site(root.occurrence(), path, target)
            .expect("canonical published path");
        sites.push(draft.request_site(&handle).expect("live binding"));
    }
    let earlier_key = draft.intern_text("earlier").expect("earlier key");
    let target_key = draft.intern_text("target").expect("target key");
    let new_value = draft.intern_int(NEW).expect("replacement value");
    let absent = draft.intern_int(-1).expect("absent return value");
    let mut code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(earlier_key),
        Instr::ConstLoad(new_value),
        Instr::RecordNew(record),
        Instr::DurReplaceEntry(sites[0].clone()),
        Instr::ConstLoad(target_key),
        Instr::LocalSet(0),
    ];
    let slots = match target {
        Target::Root => vec![0],
        Target::Branch => {
            let child_key = draft.intern_int(11).expect("child key");
            code.extend([Instr::ConstLoad(child_key), Instr::LocalSet(1)]);
            vec![0, 1]
        }
    };
    code.extend(slots.iter().copied().map(Instr::LocalGet));
    code.push(Instr::DurExists(sites[1].clone()));
    let guard = code.len();
    code.push(Instr::JumpIfFalse(0));
    code.push(Instr::DurReadFieldPresent {
        site: sites[2].clone(),
        key_slots: slots.clone(),
    });
    let present = code.len();
    code.push(Instr::Jump(0));
    code[guard] = Instr::JumpIfFalse(u32::try_from(code.len()).expect("small tape"));
    code.push(Instr::ConstLoad(absent));
    code[present] = Instr::Jump(u32::try_from(code.len()).expect("small tape"));
    code.extend([Instr::TxnCommit, Instr::Return]);
    let name = draft.intern_string("read").expect("export name");
    let source = draft.intern_string("src/main.mw").expect("source name");
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: u16::try_from(slots.len()).expect("two locals at most"),
            spans: vm_spans(&code),
            code,
        })
        .expect("live sites");
    draft.add_export(ExportId::of_local("", "read"), function);
    let bytes = draft.encode().expect("encode fixture").bytes;
    let prepared = prepare(verify(&bytes).expect("independently verify fixture"));
    let image = prepared.image();
    let export = image
        .export_by_id(ExportId::of_local("", "read"))
        .expect("export");
    let tape = image.function(export.function()).instrs();
    let earlier_site = tape
        .iter()
        .find_map(|instr| match instr {
            SealedInstr::DurReplaceEntry(site) => Some(*site),
            _ => None,
        })
        .expect("earlier replacement");
    let target_site = tape
        .iter()
        .find_map(|instr| match instr {
            SealedInstr::DurExists(site) => Some(*site),
            _ => None,
        })
        .expect("target marker guard");
    let strict_pcs: Vec<_> = tape
        .iter()
        .enumerate()
        .filter_map(|(pc, instr)| {
            matches!(instr, SealedInstr::DurReadFieldPresent { .. }).then_some(pc)
        })
        .collect();
    assert_eq!(strict_pcs.len(), 1);
    Fixture {
        prepared,
        earlier_site,
        target_site,
        strict_pc: u32::try_from(strict_pcs[0]).expect("small sealed tape"),
    }
}

fn cells(engine: &impl ByteEngine) -> Vec<Cell> {
    let view = engine.read_view().expect("real read view");
    let cells = view.scan_after(&[], &[]).expect("fixture cells");
    let last = &cells.last().expect("seeded cells").0;
    assert!(
        view.scan_after(&[], last)
            .expect("end of fixture")
            .is_empty()
    );
    cells
}

fn entry(value: i64) -> EntryValue {
    EntryValue {
        fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(value)))],
        groups: Vec::new(),
    }
}

fn check_case(engine: &mut impl ByteEngine, target: Target, damage: Damage) {
    let fixture = fixture(target);
    let projection = fixture
        .prepared
        .projection()
        .expect("executable projection");
    let image = fixture.prepared.image();
    let export = image
        .export_by_id(ExportId::of_local("", "read"))
        .expect("export");
    let demand = DemandCoverage {
        read: export.demand().reads(),
        write: export.demand().writes(),
    };
    let earlier_keys = [KeyScalar::Str("earlier".into())];
    let target_keys = target.keys();
    {
        let mut store = DurableStore::from_engine(BorrowedEngine(&mut *engine), projection.clone());
        let mut session = store
            .txn_session(InvocationGrant::full_store(), demand)
            .expect("seed");
        session
            .create_entry(
                &session.site(fixture.earlier_site),
                &earlier_keys,
                entry(OLD),
            )
            .expect("earlier entry");
        session
            .create_entry(
                &session.site(fixture.target_site),
                &target_keys,
                entry(SENTINEL),
            )
            .expect("target entry");
        assert!(matches!(session.commit(), CommitResult::Committed));
    }
    let sentinel = encode_domain(&ValueDomain::Scalar(RuntimeScalar::Int(SENTINEL)))
        .expect("sentinel encoding");
    let before = cells(engine);
    let matching: Vec<_> = before
        .iter()
        .filter(|(_, value)| *value == sentinel)
        .collect();
    assert_eq!(matching.len(), 1, "one sentinel identifies the actual leaf");
    let leaf = &matching[0].0;
    if !matches!(damage, Damage::Healthy) {
        let mut txn = engine.begin().expect("damage transaction");
        match damage {
            Damage::Missing => txn.remove(leaf).expect("remove only target leaf"),
            Damage::Malformed => txn
                .put(leaf, b"not-an-int".to_vec())
                .expect("malform only target leaf"),
            Damage::Healthy => unreachable!("healthy case performs no damage"),
        }
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let damaged = cells(engine);
    {
        let mut store = DurableStore::from_engine(BorrowedEngine(&mut *engine), projection.clone());
        {
            let mut session = store
                .txn_session(InvocationGrant::full_store(), demand)
                .expect("VM session");
            let result = run_durable(image, export.function(), Vec::new(), &mut session);
            match damage {
                Damage::Healthy => assert_eq!(
                    result.expect("healthy strict read"),
                    Some(Value::Int(SENTINEL))
                ),
                Damage::Missing | Damage::Malformed => {
                    let DurableExecutionFault::Runtime(fault) =
                        result.expect_err("required read faults")
                    else {
                        panic!("an operation fault must not become invocation-incomplete")
                    };
                    assert_eq!(fault.code(), marrow_codes::Code::RunCorruption.as_str());
                    assert_eq!((fault.line(), fault.column()), (20 + fixture.strict_pc, 4));
                    assert_eq!(
                        session.read_entry(&session.site(fixture.earlier_site), &earlier_keys),
                        Ok(Some(entry(NEW))),
                        "the earlier mutation was staged before the strict read fault",
                    );
                }
            }
        }
        let mut read = store
            .read_session(InvocationGrant::full_store(), demand)
            .expect("owner remains usable");
        let expected = if matches!(damage, Damage::Healthy) {
            NEW
        } else {
            OLD
        };
        assert_eq!(
            read.read_entry(&read.site(fixture.earlier_site), &earlier_keys),
            Ok(Some(entry(expected)))
        );
        assert_eq!(
            read.presence(&read.site(fixture.target_site), &target_keys),
            Ok(Presence::Present)
        );
    }
    if !matches!(damage, Damage::Healthy) {
        assert_eq!(
            cells(engine),
            damaged,
            "abort restores every post-corruption cell"
        );
    }
    eprintln!(
        "required read: engine={} target={target:?} damage={damage:?} strict_pc={} passed",
        std::any::type_name_of_val(engine),
        fixture.strict_pc,
    );
}

#[test]
fn required_field_faults_abort_earlier_writes_on_memory() {
    for target in [Target::Root, Target::Branch] {
        for damage in [Damage::Healthy, Damage::Missing, Damage::Malformed] {
            check_case(&mut MemoryEngine::new(), target, damage);
        }
    }
}

#[test]
fn required_field_faults_abort_earlier_writes_on_native() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "marrow-required-reads-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&root).expect("create a new scratch root without reusing prior data");
    for target in [Target::Root, Target::Branch] {
        for damage in [Damage::Healthy, Damage::Missing, Damage::Malformed] {
            let path = root.join(format!("{target:?}-{damage:?}"));
            std::fs::create_dir_all(&path).expect("native scratch");
            NativeEngineOwner::provision(&path).expect("provision actual native engine");
            let mut engine = NativeEngineOwner::acquire_existing(&path)
                .expect("owner lock")
                .bind_and_open_existing(NativeOpenAccess::ReadWrite, [0x99; 16], || {
                    Ok::<_, std::convert::Infallible>(())
                })
                .expect("open native engine");
            check_case(&mut engine, target, damage);
        }
    }
    std::fs::remove_dir_all(root).expect("remove closed native fixtures");
}
