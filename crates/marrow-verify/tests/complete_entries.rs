//! Complete-entry pins on the verifier's presence lattice and field-set typing (design
//! B2): a proof over a family ends at any erase of that family, whatever key operand the
//! erase names, and at a call whose demand closure writes the family; a field set takes
//! a definite value for a required or a sparse field alike.

use marrow_image::{DraftTxn, FunctionDef, ImageDraft, ImageType, Instr, Scalar};

#[path = "../../marrow-image/tests/common/site_seam.rs"]
mod site_seam;

#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;

#[path = "common/admitted.rs"]
mod admitted_helper;
use admitted_helper::admitted;

#[path = "common/tracer_schema.rs"]
#[allow(
    dead_code,
    reason = "each verifier test binary uses the slice of the shared tracer fixture its pins need"
)]
mod tracer_schema;
use tracer_schema::*;

/// The verdict of `if exists(slot 0) { <between>; strict sparse set on slot 0 }`, where
/// `between` may add functions to the draft and returns the instructions inside the guard.
fn strict_set_after(between: impl FnOnce(&mut DraftTxn<'_>, &Sites) -> Vec<Instr>) -> String {
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
    code_of(&finish_two_key(draft, code))
}

/// A call to a helper that erases the guarded entry ends the fact (the lattice consults the
/// callee's demand closure at the `Call`). Today calls are transparent and the image verifies.
#[test]
fn a_strict_sparse_set_after_a_call_that_erases_the_family_rejects() {
    let verdict = strict_set_after(|draft, sites| {
        let src = ok(draft.intern_string("src/main.mw"));
        let name = ok(draft.intern_string("eraser"));
        let code = vec![
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::Return,
        ];
        let eraser = draft
            .add_function(FunctionDef {
                name,
                source: src,
                params: vec![ImageType::scalar(Scalar::Text)],
                ret: ImageType::Unit,
                local_count: 1,
                spans: spans(&code),
                code,
            })
            .expect("every site operand is live");
        vec![Instr::LocalGet(0), Instr::Call(eraser.index())]
    });
    assert_eq!(verdict, "image.flow");
}

/// An erase of the family keyed by a constant, no slot at all, ends the fact. Today the
/// exact-key kill finds no slot to match and the image verifies.
#[test]
fn a_strict_sparse_set_after_an_inline_keyed_erase_of_the_family_rejects() {
    let verdict = strict_set_after(|draft, sites| {
        let other_key = ok(draft.intern_text("k"));
        vec![
            Instr::ConstLoad(other_key),
            Instr::DurEraseEntry(sites.entry.clone()),
        ]
    });
    assert_eq!(verdict, "image.flow");
}

/// An erase of the family through a different key slot ends the fact (the slots may hold
/// one key). Today only the fact on slot 1 is removed and the image verifies.
#[test]
fn a_strict_sparse_set_after_an_erase_through_another_slot_of_the_family_rejects() {
    let verdict = strict_set_after(|_, sites| {
        vec![
            Instr::LocalGet(1),
            Instr::DurEraseEntry(sites.entry.clone()),
        ]
    });
    assert_eq!(verdict, "image.flow");
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
    assert_eq!(code_of(&bytes), "VERIFIED");
}

/// An optional operand never reaches a field set: `absent`-capable values clear a field
/// only through `DurEraseField`, so a wrapped operand is a per-function type rejection.
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
    assert_eq!(code_of(&bytes), "image.function");
}
