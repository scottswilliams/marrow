//! A presence guard must execute every key load and its producer before its consumer.

use super::{
    ROOT_KEY_ID, admitted, code_of, durable_schema, durable_schema_with_keys, finish_two_key, ok,
    spans,
};
use marrow_image::{
    DraftTxn, ExportId, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, Scalar,
};

fn finish_presence_export(
    mut draft: DraftTxn<'_>,
    params: Vec<ImageType>,
    local_count: u16,
    code: Vec<Instr>,
) -> Vec<u8> {
    let source = ok(draft.intern_string("src/main.mw"));
    let name = ok(draft.intern_string("put"));
    let func = draft
        .add_function(FunctionDef {
            name,
            source,
            params,
            ret: ImageType::Unit,
            local_count,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn guard_provenance_rejects_entry_at_exists() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,               // 0
            Instr::ConstLoad(flag),        // 1
            Instr::JumpIfFalse(5),         // 2
            Instr::LocalGet(1),            // 3
            Instr::Jump(6),                // 4
            Instr::LocalGet(0),            // 5
            Instr::DurExists(sites.entry), // 6
            Instr::JumpIfFalse(10),        // 7
            Instr::ConstLoad(text),        // 8
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 9
            Instr::TxnCommit,              // 10
            Instr::Return,                 // 11
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn guard_provenance_rejects_entry_at_exists_conditional() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,               // 0
            Instr::ConstLoad(flag),        // 1
            Instr::JumpIfFalse(5),         // 2
            Instr::ConstLoad(flag),        // 3
            Instr::Jump(7),                // 4
            Instr::LocalGet(0),            // 5
            Instr::DurExists(sites.entry), // 6
            Instr::JumpIfFalse(10),        // 7
            Instr::ConstLoad(text),        // 8
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 9
            Instr::TxnCommit,              // 10
            Instr::Return,                 // 11
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn guard_provenance_rejects_entry_at_read_entry() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,                  // 0
            Instr::ConstLoad(flag),           // 1
            Instr::JumpIfFalse(5),            // 2
            Instr::LocalGet(1),               // 3
            Instr::Jump(6),                   // 4
            Instr::LocalGet(0),               // 5
            Instr::DurReadEntry(sites.entry), // 6
            Instr::BranchPresent(11),         // 7
            Instr::Pop,                       // 8
            Instr::ConstLoad(text),           // 9
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 10
            Instr::TxnCommit,                 // 11
            Instr::Return,                    // 12
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn guard_provenance_rejects_entry_at_read_entry_conditional() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,                          // 0
            Instr::ConstLoad(flag),                   // 1
            Instr::JumpIfFalse(6),                    // 2
            Instr::LocalGet(1),                       // 3
            Instr::DurReadEntry(sites.entry.clone()), // 4
            Instr::Jump(8),                           // 5
            Instr::LocalGet(0),                       // 6
            Instr::DurReadEntry(sites.entry),         // 7
            Instr::BranchPresent(12),                 // 8
            Instr::Pop,                               // 9
            Instr::ConstLoad(text),                   // 10
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 11
            Instr::TxnCommit,                         // 12
            Instr::Return,                            // 13
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn guard_provenance_rejects_entry_at_create_record_load() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let zero = ok(draft.intern_int(0));
    let bytes = finish_presence_export(
        draft,
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        3,
        vec![
            Instr::TxnBegin,                    // 0
            Instr::ConstLoad(zero),             // 1
            Instr::ConstLoad(text),             // 2
            Instr::SomeWrap,                    // 3
            Instr::RecordNew(sites.record),     // 4
            Instr::LocalSet(2),                 // 5
            Instr::ConstLoad(flag),             // 6
            Instr::JumpIfFalse(10),             // 7
            Instr::LocalGet(1),                 // 8
            Instr::Jump(11),                    // 9
            Instr::LocalGet(0),                 // 10
            Instr::LocalGet(2),                 // 11
            Instr::DurCreateEntry(sites.entry), // 12
            Instr::ConstLoad(text),             // 13
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 14
            Instr::TxnCommit,                   // 15
            Instr::Return,                      // 16
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn guard_provenance_rejects_entry_at_create() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let zero = ok(draft.intern_int(0));
    let bytes = finish_presence_export(
        draft,
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        3,
        vec![
            Instr::TxnBegin,                    // 0
            Instr::ConstLoad(zero),             // 1
            Instr::ConstLoad(text),             // 2
            Instr::SomeWrap,                    // 3
            Instr::RecordNew(sites.record),     // 4
            Instr::LocalSet(2),                 // 5
            Instr::ConstLoad(flag),             // 6
            Instr::JumpIfFalse(11),             // 7
            Instr::LocalGet(1),                 // 8
            Instr::LocalGet(2),                 // 9
            Instr::Jump(13),                    // 10
            Instr::LocalGet(0),                 // 11
            Instr::LocalGet(2),                 // 12
            Instr::DurCreateEntry(sites.entry), // 13
            Instr::ConstLoad(text),             // 14
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 15
            Instr::TxnCommit,                   // 16
            Instr::Return,                      // 17
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn guard_provenance_rejects_entry_at_composite_exists_second_key() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema_with_keys(
        &mut draft,
        vec![
            KeyColumn {
                scalar: Scalar::Int,
                id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
            },
            KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes([0x1c; 16]),
            },
        ],
    );
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_presence_export(
        draft,
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        3,
        vec![
            Instr::TxnBegin,               // 0
            Instr::ConstLoad(flag),        // 1
            Instr::JumpIfFalse(5),         // 2
            Instr::LocalGet(2),            // 3
            Instr::Jump(6),                // 4
            Instr::LocalGet(0),            // 5
            Instr::LocalGet(1),            // 6
            Instr::DurExists(sites.entry), // 7
            Instr::JumpIfFalse(11),        // 8
            Instr::ConstLoad(text),        // 9
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0, 1],
            }, // 10
            Instr::TxnCommit,              // 11
            Instr::Return,                 // 12
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn guard_provenance_rejects_entry_at_composite_read_second_key() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema_with_keys(
        &mut draft,
        vec![
            KeyColumn {
                scalar: Scalar::Int,
                id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
            },
            KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes([0x1c; 16]),
            },
        ],
    );
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_presence_export(
        draft,
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        3,
        vec![
            Instr::TxnBegin,                  // 0
            Instr::ConstLoad(flag),           // 1
            Instr::JumpIfFalse(5),            // 2
            Instr::LocalGet(2),               // 3
            Instr::Jump(6),                   // 4
            Instr::LocalGet(0),               // 5
            Instr::LocalGet(1),               // 6
            Instr::DurReadEntry(sites.entry), // 7
            Instr::BranchPresent(12),         // 8
            Instr::Pop,                       // 9
            Instr::ConstLoad(text),           // 10
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0, 1],
            }, // 11
            Instr::TxnCommit,                 // 12
            Instr::Return,                    // 13
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn guard_provenance_allows_entry_at_exists_first_key() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,               // 0
            Instr::ConstLoad(flag),        // 1
            Instr::JumpIfFalse(5),         // 2
            Instr::ConstLoad(flag),        // 3
            Instr::Pop,                    // 4
            Instr::LocalGet(0),            // 5
            Instr::DurExists(sites.entry), // 6
            Instr::JumpIfFalse(10),        // 7
            Instr::ConstLoad(text),        // 8
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 9
            Instr::TxnCommit,              // 10
            Instr::Return,                 // 11
        ],
    );
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn guard_provenance_allows_entry_at_read_first_key() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,                  // 0
            Instr::ConstLoad(flag),           // 1
            Instr::JumpIfFalse(5),            // 2
            Instr::ConstLoad(flag),           // 3
            Instr::Pop,                       // 4
            Instr::LocalGet(0),               // 5
            Instr::DurReadEntry(sites.entry), // 6
            Instr::BranchPresent(11),         // 7
            Instr::Pop,                       // 8
            Instr::ConstLoad(text),           // 9
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 10
            Instr::TxnCommit,                 // 11
            Instr::Return,                    // 12
        ],
    );
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn guard_provenance_allows_entry_at_create_first_key() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let zero = ok(draft.intern_int(0));
    let bytes = finish_presence_export(
        draft,
        vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Text),
        ],
        3,
        vec![
            Instr::TxnBegin,                    // 0
            Instr::ConstLoad(zero),             // 1
            Instr::ConstLoad(text),             // 2
            Instr::SomeWrap,                    // 3
            Instr::RecordNew(sites.record),     // 4
            Instr::LocalSet(2),                 // 5
            Instr::ConstLoad(flag),             // 6
            Instr::JumpIfFalse(10),             // 7
            Instr::ConstLoad(flag),             // 8
            Instr::Pop,                         // 9
            Instr::LocalGet(0),                 // 10
            Instr::LocalGet(2),                 // 11
            Instr::DurCreateEntry(sites.entry), // 12
            Instr::ConstLoad(text),             // 13
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 14
            Instr::TxnCommit,                   // 15
            Instr::Return,                      // 16
        ],
    );
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn guard_provenance_allows_entry_at_composite_exists_first_key() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema_with_keys(
        &mut draft,
        vec![
            KeyColumn {
                scalar: Scalar::Int,
                id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
            },
            KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes([0x1c; 16]),
            },
        ],
    );
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_presence_export(
        draft,
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        3,
        vec![
            Instr::TxnBegin,               // 0
            Instr::ConstLoad(flag),        // 1
            Instr::JumpIfFalse(5),         // 2
            Instr::ConstLoad(flag),        // 3
            Instr::Pop,                    // 4
            Instr::LocalGet(0),            // 5
            Instr::LocalGet(1),            // 6
            Instr::DurExists(sites.entry), // 7
            Instr::JumpIfFalse(11),        // 8
            Instr::ConstLoad(text),        // 9
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0, 1],
            }, // 10
            Instr::TxnCommit,              // 11
            Instr::Return,                 // 12
        ],
    );
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn guard_provenance_allows_entry_at_composite_read_first_key() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema_with_keys(
        &mut draft,
        vec![
            KeyColumn {
                scalar: Scalar::Int,
                id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
            },
            KeyColumn {
                scalar: Scalar::Text,
                id: LedgerIdBytes::from_bytes([0x1c; 16]),
            },
        ],
    );
    let flag = ok(draft.intern_bool(true));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_presence_export(
        draft,
        vec![
            ImageType::scalar(Scalar::Int),
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        3,
        vec![
            Instr::TxnBegin,                  // 0
            Instr::ConstLoad(flag),           // 1
            Instr::JumpIfFalse(5),            // 2
            Instr::ConstLoad(flag),           // 3
            Instr::Pop,                       // 4
            Instr::LocalGet(0),               // 5
            Instr::LocalGet(1),               // 6
            Instr::DurReadEntry(sites.entry), // 7
            Instr::BranchPresent(12),         // 8
            Instr::Pop,                       // 9
            Instr::ConstLoad(text),           // 10
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0, 1],
            }, // 11
            Instr::TxnCommit,                 // 12
            Instr::Return,                    // 13
        ],
    );
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn guard_provenance_rejects_late_backward_entry_with_unchanged_frame() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let sites = durable_schema(&mut draft);
    let falseflag = ok(draft.intern_bool(false));
    let text = ok(draft.intern_text("x"));
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::TxnBegin,               // 0
            Instr::ConstLoad(falseflag),   // 1
            Instr::JumpIfFalse(10),        // 2
            Instr::LocalGet(0),            // 3
            Instr::DurExists(sites.entry), // 4
            Instr::JumpIfFalse(8),         // 5
            Instr::ConstLoad(text),        // 6
            Instr::DurSetField {
                site: sites.label,
                key_slots: vec![0],
            }, // 7
            Instr::TxnCommit,              // 8
            Instr::Return,                 // 9
            Instr::LocalGet(1),            // 10
            Instr::Jump(4),                // 11
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}
