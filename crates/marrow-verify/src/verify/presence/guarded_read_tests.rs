use super::{admitted_plan::admitted_plan, site_seam::site};
use crate::{SealedConst, SealedInstr, SealedSite, SealedSiteTarget, VerifyPhase};
use marrow_codes::Code;
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, ExportId, FieldDef, FunctionDef, ImageDraft,
    ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootOccurrenceDef, Scalar,
    SemanticTarget, SpanEntry,
};

fn image(keys: u16, padding: usize, interleaved_read: Option<[u16; 2]>) -> Vec<u8> {
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
    let mut root_keys = vec![KeyColumn {
        scalar: Scalar::Int,
        id: LedgerIdBytes::from_bytes([5; 16]),
    }];
    if interleaved_read.is_some() {
        assert_eq!((keys, padding), (3, 0));
        root_keys.push(KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([6; 16]),
        });
    }
    let root = draft
        .add_root_occurrence(
            &admitted_plan(),
            product,
            RootOccurrenceDef {
                name,
                keys: root_keys,
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
    let (code, local_count) = if let Some(read_slots) = interleaved_read {
        let yes = draft.intern_bool(true).expect("diamond condition");
        let mut code = vec![Instr::ConstLoad(yes)];
        let mut guards = Vec::new();
        for [first, second] in [[0, 0], [0, 1], [1, 0], [1, 1]] {
            code.extend([
                Instr::LocalGet(first),
                Instr::LocalGet(second),
                Instr::DurExists(entry.clone()),
            ]);
            guards.push(code.len());
            code.push(Instr::JumpIfFalse(0));
            // Both diamond edges retain the Boolean below the guard operands.
            let join = u32::try_from(code.len() + 3).expect("small diamond target");
            code.extend([
                Instr::ConstLoad(yes),
                Instr::JumpIfFalse(join),
                Instr::BoolNot,
            ]);
        }
        code.extend([
            Instr::Pop,
            Instr::DurReadFieldPresent {
                site: field.clone(),
                key_slots: read_slots.to_vec(),
            },
            Instr::Return,
        ]);
        let absent = u32::try_from(code.len()).expect("small absent target");
        code.extend([Instr::Pop, Instr::ConstLoad(zero), Instr::Return]);
        for guard in guards {
            code[guard] = Instr::JumpIfFalse(absent);
        }
        assert_eq!(code.len(), 35);
        assert_eq!(code.iter().map(Instr::encoded_len).sum::<usize>(), 111);
        (code, keys)
    } else {
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
        (code, keys + 1)
    };
    let name = draft.intern_string("inspect").expect("function name");
    let source = draft.intern_string("retention.mw").expect("source name");
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: vec![int; usize::from(keys)],
            ret: int,
            local_count,
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

/// Padding between guarded reads changes only the instruction count: every key
/// still seals exactly one presence read against its own slot.
#[test]
fn padding_between_guarded_reads_seals_one_read_per_key() {
    for keys in [1u16, 8] {
        for padding in [0, 16, 64] {
            let bytes = image(keys, padding, None);
            let verified = crate::verify(&bytes).expect("the guarded-read image verifies");
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
        }
    }
}

/// A read whose key pair a guard established verifies; the same read over a pair
/// no guard established is refused in the flow phase.
#[test]
fn a_guarded_key_pair_verifies_and_an_unguarded_one_is_refused() {
    let bytes = image(3, 0, Some([0, 0]));
    let verified = crate::verify(&bytes).expect("the guarded pair verifies");
    assert_eq!(verified.functions().len(), 1);
    assert_eq!(verified.exports().len(), 1);
    let export = &verified.exports()[0];
    assert_eq!(export.id(), ExportId::of_local("", "inspect"));
    assert_eq!(export.function().index(), 0);
    assert!(!export.is_mutating());
    assert_eq!(export.reachable_sites(), &[0, 1]);
    let function = &verified.functions()[0];
    assert_eq!(function.name(), "inspect");
    assert_eq!(function.source(), "retention.mw");
    assert_eq!(function.params(), &[ImageType::scalar(Scalar::Int); 3]);
    assert_eq!(function.ret(), ImageType::scalar(Scalar::Int));
    assert_eq!(function.local_count(), 3);
    assert_eq!(function.max_stack(), 3);
    assert!(!function.is_mutating());
    assert_eq!(verified.roots().len(), 1);
    assert_eq!(verified.roots()[0].keys(), &[Scalar::Int; 2]);
    let fields = verified.record_type(verified.roots()[0].record()).fields();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name.as_ref(), "value");
    assert_eq!(fields[0].ty, ImageType::scalar(Scalar::Int));
    assert!(fields[0].required);
    assert_eq!(
        verified.sites(),
        &[
            SealedSite::Flat {
                root: 0,
                target: SealedSiteTarget::WholePayload,
            },
            SealedSite::Flat {
                root: 0,
                target: SealedSiteTarget::FieldLeaf(0),
            },
        ],
    );
    assert_eq!(
        verified.consts(),
        &[SealedConst::Int(0), SealedConst::Bool(true)],
    );
    let code = function.instrs();
    assert_eq!(code.len(), 35);
    assert_eq!(code[0], SealedInstr::ConstLoad(1));
    for (index, [first, second]) in [[0, 0], [0, 1], [1, 0], [1, 1]].into_iter().enumerate() {
        let start = 1 + 7 * index;
        assert_eq!(code[start], SealedInstr::LocalGet(first));
        assert_eq!(code[start + 1], SealedInstr::LocalGet(second));
        assert_eq!(code[start + 2], SealedInstr::DurExists(0));
        assert_eq!(code[start + 3], SealedInstr::JumpIfFalse(32));
        assert_eq!(code[start + 4], SealedInstr::ConstLoad(1));
        assert_eq!(code[start + 5], SealedInstr::JumpIfFalse(start + 7));
        assert_eq!(code[start + 6], SealedInstr::BoolNot);
    }
    assert_eq!(
        &code[29..],
        &[
            SealedInstr::Pop,
            SealedInstr::DurReadFieldPresent {
                site: 1,
                key_slots: vec![0, 0],
            },
            SealedInstr::Return,
            SealedInstr::Pop,
            SealedInstr::ConstLoad(0),
            SealedInstr::Return,
        ],
    );
    for index in 0..code.len() {
        assert_eq!(
            function.span_at(index),
            Some((u32::try_from(index).expect("small span") + 1, 1)),
        );
    }

    // Only the final read operand changes; slot 2 is initialized and key-typed,
    // but no guard establishes its ordered pair with slot 0.
    let negative = image(3, 0, Some([0, 2]));
    let refusal = crate::verify(&negative).expect_err("the unguarded pair is refused");
    assert_eq!(refusal.phase(), VerifyPhase::Flow);
    assert_eq!(refusal.code(), Code::ImageFlow);
}
