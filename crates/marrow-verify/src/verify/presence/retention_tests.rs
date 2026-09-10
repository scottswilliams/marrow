use super::{PresenceFact, admitted_plan::admitted_plan, site_seam::site};
use crate::{SealedInstr, VerifiedImage};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, ExportId, FieldDef, FunctionDef, ImageDraft,
    ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootOccurrenceDef, Scalar,
    SemanticTarget, SpanEntry,
};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::panic::{catch_unwind, resume_unwind};

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    completed: usize,
    retained_sets: usize,
    retained_facts: usize,
    slots: usize,
    capacity: usize,
}

thread_local! {
    static COUNTS: Cell<Option<Counts>> = const { Cell::new(None) };
}

pub(super) fn record_success(entry: &[Option<BTreeSet<PresenceFact>>], capacity: usize) {
    COUNTS.with(|cell| {
        let Some(mut counts) = cell.get() else {
            return;
        };
        counts.completed += 1;
        counts.slots += entry.len();
        counts.capacity += capacity;
        for set in entry.iter().flatten() {
            counts.retained_sets += 1;
            counts.retained_facts += set.len();
        }
        cell.set(Some(counts));
    });
}

fn observe(bytes: &[u8]) -> (VerifiedImage, Counts) {
    COUNTS.with(|cell| {
        assert!(cell.get().is_none(), "observations do not nest");
        cell.set(Some(Counts::default()));
    });
    let result = catch_unwind(|| crate::verify(bytes));
    let counts = COUNTS.with(|cell| cell.take().expect("observation is active"));
    match result {
        Ok(result) => (result.expect("guarded integer reads verify"), counts),
        Err(panic) => resume_unwind(panic),
    }
}

fn image(keys: u16, padding: usize) -> Vec<u8> {
    let mut owner = ImageDraft::new();
    let savepoint = owner.savepoint();
    let mut draft = owner.begin_transaction(savepoint).expect("fresh savepoint");
    draft.set_application_identity(LedgerIdBytes::from_bytes([1; 16]));
    let product = LedgerIdBytes::from_bytes([2; 16]);
    let name = draft.intern_string("Counter").expect("record name");
    let field_name = draft.intern_string("value").expect("field name");
    let int = ImageType::scalar(Scalar::Int);
    let record = draft
        .add_record_type(RecordTypeDef {
            name,
            fields: vec![FieldDef {
                name: field_name,
                ty: int,
                required: true,
            }],
        })
        .expect("required integer field");
    let value = draft.value_scalar(Scalar::Int).expect("integer shape");
    draft
        .declare_product(
            &admitted_plan(),
            product,
            record,
            vec![DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Field {
                    id: LedgerIdBytes::from_bytes([3; 16]),
                    required: true,
                    value,
                },
            }],
        )
        .expect("one-field product");
    let members = draft.product_members(product).expect("declared field");
    let name = draft.intern_string("counters").expect("root name");
    let root = draft
        .add_root_occurrence(
            &admitted_plan(),
            product,
            RootOccurrenceDef {
                name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes([5; 16]),
                }],
                placement: LedgerIdBytes::from_bytes([4; 16]),
                indexes: Vec::new().into(),
            },
        )
        .expect("one integer-keyed root");
    let entry = site(
        &mut draft,
        root.occurrence(),
        root.placement_path(),
        SemanticTarget::WholePayload,
    );
    let field = site(
        &mut draft,
        root.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    let zero = draft.intern_int(0).expect("zero constant");
    let accumulator = keys;
    let mut code = vec![Instr::ConstLoad(zero), Instr::LocalSet(accumulator)];
    let mut guards = Vec::new();
    for slot in 0..keys {
        code.extend([Instr::LocalGet(slot), Instr::DurExists(entry.clone())]);
        guards.push(code.len());
        code.push(Instr::JumpIfFalse(0));
    }
    for _ in 0..padding {
        code.extend([
            Instr::LocalGet(accumulator),
            Instr::ConstLoad(zero),
            Instr::IntAdd,
            Instr::LocalSet(accumulator),
        ]);
    }
    for slot in 0..keys {
        code.extend([
            Instr::LocalGet(accumulator),
            Instr::DurReadFieldPresent {
                site: field.clone(),
                key_slots: vec![slot],
            },
            Instr::IntAdd,
            Instr::LocalSet(accumulator),
        ]);
    }
    code.extend([Instr::LocalGet(accumulator), Instr::Return]);
    let absent = code.len() as u32;
    code.extend([Instr::ConstLoad(zero), Instr::Return]);
    for guard in guards {
        code[guard] = Instr::JumpIfFalse(absent);
    }
    let name = draft.intern_string("inspect").expect("function name");
    let source = draft.intern_string("retention.mw").expect("source name");
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: vec![int; usize::from(keys)],
            ret: int,
            local_count: keys + 1,
            spans: (0..code.len())
                .map(|index| SpanEntry {
                    instr_index: index as u32,
                    line: index as u32 + 1,
                    column: 1,
                })
                .collect(),
            code,
        })
        .expect("guard and accumulator operands are live");
    draft.add_export(ExportId::of_local("", "inspect"), function);
    draft.encode().expect("small guarded-read image").bytes
}

#[test]
fn ordinary_padding_does_not_retain_more_presence_sets_or_facts() {
    // Complete all six verifications before the resource assertion so an initial
    // regression failure still reports both key counts and every padding size.
    let observations = [1u16, 8].map(|keys| {
        [0, 16, 64].map(|padding| {
            let bytes = image(keys, padding);
            let (verified, counts) = observe(&bytes);
            assert_eq!(verified.functions().len(), 1);
            let function = &verified.functions()[0];
            let code = function.instrs();
            assert_eq!(function.params().len(), usize::from(keys));
            assert_eq!(function.local_count(), keys + 1);
            assert_eq!(function.max_stack(), 2);
            assert_eq!(code.len(), 7 * usize::from(keys) + 6 + 4 * padding);
            let read_slots: Vec<_> = code
                .iter()
                .filter_map(|instruction| {
                    if let SealedInstr::DurReadFieldPresent { key_slots, .. } = instruction {
                        Some(key_slots.as_slice())
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(read_slots.len(), usize::from(keys));
            for (slot, actual) in (0..keys).zip(read_slots) {
                assert_eq!(actual, &[slot]);
            }
            assert_eq!(counts.completed, 1);
            assert_eq!(counts.slots, code.len());
            assert!(counts.capacity >= counts.slots);
            eprintln!("presence retention keys={keys} padding={padding}: {counts:?}");
            counts
        })
    });
    for (keys, [base, pad16, pad64]) in [1, 8].into_iter().zip(observations) {
        for padded in [pad16, pad64] {
            assert_eq!(
                (padded.retained_sets, padded.retained_facts),
                (base.retained_sets, base.retained_facts),
                "ordinary padding retains additional presence state with {keys} keys",
            );
        }
    }
    for counts in observations.into_iter().flatten() {
        assert_eq!(
            (counts.retained_sets, counts.retained_facts),
            (2, 0),
            "only the empty initial entry and shared absent destination retain presence sets",
        );
    }
}
