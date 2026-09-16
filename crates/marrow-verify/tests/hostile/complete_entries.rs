//! Complete-entry presence and field-set typing: a proof ends at any entry erase
//! in its family, whatever key operand the erase names, and at a call that can
//! erase an entry in that family. A field set takes a definite value for a
//! required or a sparse field alike.

use marrow_image::{
    DraftTxn, ExportId, FieldDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes,
    RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget,
};
use marrow_verify::VerifyPhase;

use super::tracer_schema::Verdict::{Refused, Verified};
use super::tracer_schema::*;
use marrow_test_support::admitted;
use marrow_test_support::{admitted_plan, site};

/// The verdict of `if exists(slot 0) { <between>; strict sparse set on slot 0 }`, where
/// `between` may add functions to the draft and returns the instructions inside the guard.
fn strict_set_after(between: impl FnOnce(&mut DraftTxn<'_>, &Sites) -> Vec<Instr>) -> Verdict {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let text = ok(draft.intern_text("x"));
    let middle = between(&mut draft, &sites);
    let commit_at = (4 + middle.len() + 2) as u32; // the TxnCommit after the set
    let mut code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurExists(sites.entry.clone()),
        Instr::JumpIfFalse(commit_at),
    ];
    code.extend(middle);
    code.extend([
        Instr::ConstLoad(text),
        Instr::DurSetField {
            site: sites.label.clone(),
            key_slots: vec![0],
        },
        Instr::TxnCommit,
        Instr::Return,
    ]);
    verdict_of(&finish_two_key(draft, code))
}

/// A call to a helper that can erase an entry in the guarded family ends the fact.
#[test]
fn a_strict_sparse_set_after_a_call_that_erases_the_family_rejects() {
    let verdict = strict_set_after(|draft, sites| {
        let code = vec![
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::Return,
        ];
        let eraser = add_fn(
            draft,
            "eraser",
            vec![ImageType::scalar(Scalar::Text)],
            ImageType::Unit,
            1,
            code,
        );
        vec![Instr::LocalGet(0), Instr::Call(eraser.index())]
    });
    assert_eq!(verdict, Refused(VerifyPhase::Flow));
}

/// An entry erase keyed by a constant ends the fact for the family without a
/// matching key slot.
#[test]
fn a_strict_sparse_set_after_an_inline_keyed_erase_of_the_family_rejects() {
    let verdict = strict_set_after(|draft, sites| {
        let other_key = ok(draft.intern_text("k"));
        vec![
            Instr::ConstLoad(other_key),
            Instr::DurEraseEntry(sites.entry.clone()),
        ]
    });
    assert_eq!(verdict, Refused(VerifyPhase::Flow));
}

/// An entry erase through a different key slot ends the fact for the family;
/// the slots may hold the same key.
#[test]
fn a_strict_sparse_set_after_an_erase_through_another_slot_of_the_family_rejects() {
    let verdict = strict_set_after(|_, sites| {
        vec![
            Instr::LocalGet(1),
            Instr::DurEraseEntry(sites.entry.clone()),
        ]
    });
    assert_eq!(verdict, Refused(VerifyPhase::Flow));
}

/// A field set takes a definite value for a required field as for a sparse one: the
/// required `value:int` set under an `exists` guard verifies.
#[test]
fn a_required_field_set_through_a_proven_entry_verifies() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let seven = ok(draft.intern_int(7));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(6),
            Instr::ConstLoad(seven),
            Instr::DurSetField {
                site: sites.value,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(verdict_of(&bytes), Verified);
}

#[test]
fn a_read_only_required_field_read_needs_its_own_presence_check() {
    for guarded in [false, true] {
        let mut draft_owner = ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let sites = durable_schema(&mut draft);
        let mut code = Vec::new();
        if guarded {
            code.extend([
                Instr::LocalGet(0),
                Instr::DurExists(sites.entry),
                Instr::JumpIfFalse(6),
            ]);
        }
        code.extend([
            Instr::DurReadFieldPresent {
                site: sites.value,
                key_slots: vec![0],
            },
            Instr::IntNeg,
            Instr::Pop,
            Instr::Return,
        ]);
        let bytes = finish_two_key(draft, code);
        assert_eq!(
            verdict_of(&bytes),
            if guarded {
                Verified
            } else {
                Refused(VerifyPhase::Flow)
            }
        );
        if guarded {
            let image = marrow_verify::verify(&bytes).expect("the guarded read verified");
            assert!(
                image
                    .functions()
                    .iter()
                    .all(|function| !function.is_mutating())
            );
        }
    }
}

/// A field set consumes a definite value; `DurEraseField` clears a sparse field.
/// A wrapped set operand is rejected during function typing.
#[test]
fn a_field_set_with_an_optional_operand_rejects_at_function() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(7),
            Instr::ConstLoad(text),
            Instr::SomeWrap,
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Function));
}

#[test]
fn a_create_record_constructor_cannot_lend_its_field_slot_as_an_entry_key() {
    let mut owner = ImageDraft::new();
    let mut draft = admitted(&mut owner);
    let name = ok(draft.intern_string("Counter"));
    let value_name = ok(draft.intern_string("value"));
    let record = ok(draft.add_record_type(RecordTypeDef {
        name,
        fields: vec![FieldDef {
            name: value_name,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    }));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let shapes = scalar_shapes(&mut draft);
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![field_member(
                shapes,
                None,
                VALUE_FIELD_ID,
                true,
                Scalar::Text,
            )],
        )
        .expect("the complete string record is declared");
    let root_name = ok(draft.intern_string("counters"));
    let root = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Text,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the string-keyed root is admitted");
    let entry = site(
        &mut draft,
        root.occurrence(),
        root.placement_path(),
        SemanticTarget::WholePayload,
    );
    let members = product_members(&draft);
    let field = site(
        &mut draft,
        root.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    let text = ok(draft.intern_text("updated"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::RecordNew(record),
            Instr::DurCreateEntry(entry),
            Instr::ConstLoad(text),
            Instr::DurSetField {
                site: field,
                key_slots: vec![1],
            },
            Instr::TxnCommit,
            Instr::Return,
        ],
    );
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}

#[test]
fn a_composite_create_cannot_prove_a_key_load_bypassed_by_another_edge() {
    let mut owner = ImageDraft::new();
    let mut draft = admitted(&mut owner);
    let sites = durable_schema_with_keys(
        &mut draft,
        vec![
            KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
            },
            KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes([0x17; 16]),
            },
        ],
    );
    let zero = ok(draft.intern_int(0));
    let text = ok(draft.intern_text("x"));
    let flag = ok(draft.intern_bool(true));
    let code = vec![
        Instr::TxnBegin,
        Instr::ConstLoad(zero),
        Instr::ConstLoad(text),
        Instr::SomeWrap,
        Instr::RecordNew(sites.record),
        Instr::LocalSet(2),
        Instr::ConstLoad(flag),
        Instr::JumpIfFalse(10),
        Instr::ConstLoad(text),
        Instr::Jump(11),
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::LocalGet(2),
        Instr::DurCreateEntry(sites.entry),
        Instr::ConstLoad(text),
        Instr::DurSetField {
            site: sites.label,
            key_slots: vec![0, 1],
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let function = add_fn(
        &mut draft,
        "put",
        vec![ImageType::scalar(Scalar::Text); 2],
        ImageType::Unit,
        3,
        code,
    );
    draft.add_export(ExportId::of_local("", "put"), function);
    let bytes = draft.encode().expect("encode").bytes;
    assert_eq!(verdict_of(&bytes), Refused(VerifyPhase::Flow));
}
