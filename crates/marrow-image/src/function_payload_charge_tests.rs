use super::{
    DECISIVE_FUNCTION_PAYLOAD, DraftStateError, DraftTxn, FuncId, FunctionDef, ImageDraft,
    SpanEntry,
};
use crate::bounds::MAX_IMAGE_BYTES;
use crate::encode::SPAN_ROW_BYTES;
use crate::instr::Instr;
use crate::ty::ImageType;

fn body(txn: &mut DraftTxn<'_>, instructions: usize, spans: usize) -> FunctionDef {
    let name = txn.intern_string("body").expect("a within-domain mint");
    let source = txn.intern_string("src").expect("a within-domain mint");
    FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        code: vec![Instr::Return; instructions],
        spans: (0..spans)
            .map(|index| SpanEntry {
                instr_index: index as u32,
                line: 1,
                column: 1,
            })
            .collect(),
    }
}

/// The charge is the instruction count plus one span row per span, and the
/// predicate flips exactly one byte past the image ceiling.
#[test]
fn a_successful_append_charges_its_instructions_and_span_rows() {
    let mut draft = ImageDraft::new();
    assert_eq!(draft.function_payload_charge, 0);
    let mut txn = draft.begin_transaction();
    let def = body(&mut txn, 7, 3);
    txn.add_function(def).expect("no site operand");
    assert_eq!(txn.function_payload_charge, 7 + 3 * SPAN_ROW_BYTES);
    assert!(!txn.function_payload_exceeds_image_limit());

    let remaining = MAX_IMAGE_BYTES - (7 + 3 * SPAN_ROW_BYTES);
    let def = body(&mut txn, remaining, 0);
    txn.add_function(def).expect("no site operand");
    assert_eq!(txn.function_payload_charge, MAX_IMAGE_BYTES);
    assert!(!txn.function_payload_exceeds_image_limit());

    let def = body(&mut txn, 1, 0);
    txn.add_function(def).expect("no site operand");
    assert_eq!(txn.function_payload_charge, DECISIVE_FUNCTION_PAYLOAD);
    assert!(txn.function_payload_exceeds_image_limit());
}

/// The charge saturates one past the ceiling and stays there, however much more is
/// appended.
#[test]
fn the_charge_saturates_at_the_decisive_total() {
    let mut draft = ImageDraft::new();
    let mut txn = draft.begin_transaction();
    let def = body(&mut txn, 1, MAX_IMAGE_BYTES);
    txn.add_function(def).expect("no site operand");
    assert_eq!(txn.function_payload_charge, DECISIVE_FUNCTION_PAYLOAD);
    let def = body(&mut txn, MAX_IMAGE_BYTES, MAX_IMAGE_BYTES);
    txn.add_function(def).expect("no site operand");
    assert_eq!(txn.function_payload_charge, DECISIVE_FUNCTION_PAYLOAD);
    assert!(txn.function_payload_exceeds_image_limit());
}

/// A refused append changes nothing: the function-slot carrier refusal leaves the
/// charge at the accepted total.
#[test]
fn a_refused_append_leaves_the_charge_unchanged() {
    let mut draft = ImageDraft::new();
    let mut txn = draft.begin_transaction();
    for _ in 0..=u16::MAX {
        let def = body(&mut txn, 1, 0);
        txn.add_function(def).expect("a within-carrier ordinal");
    }
    let accepted = txn.function_payload_charge;
    assert_eq!(accepted, usize::from(u16::MAX) + 1);
    let def = body(&mut txn, 1, 0);
    assert!(matches!(
        txn.add_function(def),
        Err(DraftStateError::CarrierDomain)
    ));
    assert_eq!(txn.function_payload_charge, accepted);
    assert!(!txn.function_payload_exceeds_image_limit());
}

/// A transaction that saturated the charge rolls back to the exact pre-admission
/// total; a committed one keeps it.
#[test]
fn rollback_restores_a_saturated_charge() {
    let mut draft = ImageDraft::new();
    let mut txn = draft.begin_transaction();
    let def = body(&mut txn, 5, 5);
    txn.add_function(def).expect("no site operand");
    let reserved = txn.reserve_function().expect("a prefix reservation");
    txn.commit();
    let before = draft.function_payload_charge;

    let mut txn = draft.begin_transaction();
    let def = body(&mut txn, 1, MAX_IMAGE_BYTES);
    txn.fill_function(reserved, def).expect("the prefix fills");
    assert!(txn.function_payload_exceeds_image_limit());
    txn.rollback();
    assert_eq!(draft.function_payload_charge, before);
    assert!(draft.function_code(reserved).is_none());
    assert!(!draft.function_payload_exceeds_image_limit());

    let mut txn = draft.begin_transaction();
    let def = body(&mut txn, 1, MAX_IMAGE_BYTES);
    txn.fill_function(reserved, def)
        .expect("the prefix refills");
    txn.commit();
    assert_eq!(draft.function_payload_charge, DECISIVE_FUNCTION_PAYLOAD);
    assert!(draft.function_payload_exceeds_image_limit());
}

#[test]
fn reservation_owns_the_full_carrier_and_invalid_fills_spend_nothing() {
    let mut owner = ImageDraft::new();
    let mut txn = owner.begin_transaction();
    for ordinal in 0..=u16::MAX {
        assert_eq!(
            txn.reserve_function()
                .expect("a within-carrier reservation")
                .index(),
            ordinal
        );
    }
    for _ in 0..2 {
        assert_eq!(txn.reserve_function(), Err(DraftStateError::CarrierDomain));
        assert_eq!(txn.function_count(), usize::from(u16::MAX) + 1);
        assert_eq!(txn.function_payload_charge, 0);
    }
    txn.commit();
    let mut txn = owner.begin_transaction();
    let def = body(&mut txn, 7, 0);
    let last = FuncId(u16::MAX);
    txn.fill_function(last, def.clone())
        .expect("valid fixture construction");
    assert_eq!(txn.function_payload_charge, 7);
    assert_eq!(
        txn.fill_function(last, def),
        Err(DraftStateError::IncoherentToken)
    );
    assert_eq!(txn.function_payload_charge, 7);
    assert_eq!(
        txn.function_code(last)
            .expect("the highest slot is filled")
            .len(),
        7
    );
    txn.rollback();
    assert_eq!(owner.function_payload_charge, 0);
    assert!(owner.function_code(last).is_none());

    let mut owner = ImageDraft::new();
    let mut txn = owner.begin_transaction();
    let def = body(&mut txn, 7, 0);
    assert_eq!(
        txn.fill_function(FuncId(0), def),
        Err(DraftStateError::IncoherentToken)
    );
    assert_eq!(txn.function_count(), 0);
    assert_eq!(txn.function_payload_charge, 0);
    assert_eq!(
        txn.reserve_function()
            .expect("a within-carrier reservation")
            .index(),
        0
    );
}
