//! Required reads independently consume typed key tuples and live entry proofs.

use marrow_image::{ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar};

use super::admitted_helper::admitted;
use super::tracer_schema::*;

#[test]
fn required_reads_reject_sparse_and_non_field_targets() {
    for target in ["required", "sparse", "entry"] {
        let mut owner = ImageDraft::new();
        let mut draft = admitted(&mut owner);
        let sites = durable_schema(&mut draft);
        let site = match target {
            "required" => sites.value,
            "sparse" => sites.label,
            _ => sites.entry.clone(),
        };
        let bytes = finish_two_key(
            draft,
            vec![
                Instr::LocalGet(0),
                Instr::DurExists(sites.entry),
                Instr::JumpIfFalse(5),
                Instr::DurReadFieldPresent {
                    site,
                    key_slots: vec![0],
                },
                Instr::Pop,
                Instr::Return,
            ],
        );
        assert_eq!(
            code_of(&bytes),
            if target == "required" {
                "VERIFIED"
            } else {
                "image.function"
            },
            "{target}"
        );
    }
}

#[test]
fn required_reads_need_the_exact_initialized_typed_key_slots() {
    for (slots, verdict) in [
        (vec![0], "VERIFIED"),
        (vec![1], "image.flow"),
        (vec![2], "image.function"),
        (vec![3], "image.function"),
        (vec![4], "image.function"),
        (vec![0, 1], "image.function"),
    ] {
        let mut owner = ImageDraft::new();
        let mut draft = admitted(&mut owner);
        let sites = durable_schema(&mut draft);
        let code = vec![
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(5),
            Instr::DurReadFieldPresent {
                site: sites.value,
                key_slots: slots.clone(),
            },
            Instr::Pop,
            Instr::Return,
        ];
        let name = ok(draft.intern_string("read"));
        let source = ok(draft.intern_string("src/main.mw"));
        let function = draft
            .add_function(FunctionDef {
                name,
                source,
                params: vec![
                    ImageType::scalar(Scalar::Text),
                    ImageType::scalar(Scalar::Text),
                    ImageType::scalar(Scalar::Bool),
                ],
                ret: ImageType::Unit,
                local_count: 4,
                spans: spans(&code),
                code,
            })
            .expect("live sites");
        draft.add_export(ExportId::of_local("", "read"), function);
        assert_eq!(
            code_of(&draft.encode().expect("encode").bytes),
            verdict,
            "{slots:?}"
        );
    }
}

#[test]
fn rebinding_a_proved_key_invalidates_a_required_read() {
    let mut owner = ImageDraft::new();
    let mut draft = admitted(&mut owner);
    let sites = durable_schema(&mut draft);
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(7),
            Instr::LocalGet(1),
            Instr::LocalSet(0),
            Instr::DurReadFieldPresent {
                site: sites.value,
                key_slots: vec![0],
            },
            Instr::Pop,
            Instr::Return,
        ],
    );
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn required_branch_reads_need_the_same_family_and_whole_ordered_tuple() {
    for (guard_branch, read_branch, slots, verdict) in [
        (true, true, vec![0, 1], "VERIFIED"),
        (true, true, vec![0], "image.function"),
        (true, true, vec![1, 0], "image.flow"),
        (false, true, vec![0, 1], "image.flow"),
        (true, false, vec![0], "image.flow"),
        (false, false, vec![0], "VERIFIED"),
    ] {
        let mut schema = super::branch_presence_schema();
        let draft = admitted(&mut schema.owner);
        let mut code = vec![Instr::LocalGet(0)];
        if guard_branch {
            code.push(Instr::LocalGet(1));
        }
        code.push(Instr::DurExists(if guard_branch {
            schema.entry
        } else {
            schema.root.entry
        }));
        let end = (code.len() + 3) as u32;
        code.extend([
            Instr::JumpIfFalse(end),
            Instr::DurReadFieldPresent {
                site: if read_branch {
                    schema.field
                } else {
                    schema.root.value
                },
                key_slots: slots.clone(),
            },
            Instr::Pop,
            Instr::Return,
        ]);
        assert_eq!(
            code_of(&finish_two_key(draft, code)),
            verdict,
            "guard branch={guard_branch}, read branch={read_branch}, slots={slots:?}"
        );
    }
}

#[test]
fn required_read_only_guards_reject_bypassed_key_producers() {
    for bypass in [false, true] {
        let mut owner = ImageDraft::new();
        let mut draft = admitted(&mut owner);
        let sites = durable_schema(&mut draft);
        let flag = ok(draft.intern_bool(true));
        let code = vec![
            Instr::ConstLoad(flag),
            Instr::JumpIfFalse(4),
            if bypass {
                Instr::LocalGet(1)
            } else {
                Instr::ConstLoad(flag)
            },
            if bypass { Instr::Jump(5) } else { Instr::Pop },
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(9),
            Instr::DurReadFieldPresent {
                site: sites.value,
                key_slots: vec![0],
            },
            Instr::Pop,
            Instr::Return,
        ];
        assert_eq!(
            code_of(&finish_two_key(draft, code)),
            if bypass { "image.flow" } else { "VERIFIED" }
        );
    }
}

#[test]
fn a_required_read_cannot_follow_commit_even_with_a_live_proof() {
    for after_commit in [false, true] {
        let mut owner = ImageDraft::new();
        let mut draft = admitted(&mut owner);
        let sites = durable_schema(&mut draft);
        let mut code = vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(10),
            Instr::LocalGet(0),
            Instr::DurEraseField(sites.label),
        ];
        let read = Instr::DurReadFieldPresent {
            site: sites.value,
            key_slots: vec![0],
        };
        if after_commit {
            code.extend([Instr::TxnCommit, read, Instr::Pop]);
        } else {
            code.extend([read, Instr::Pop, Instr::TxnCommit]);
        }
        code.extend([Instr::Return, Instr::TxnCommit, Instr::Return]);
        assert_eq!(
            code_of(&finish_two_key(draft, code)),
            if after_commit {
                "image.flow"
            } else {
                "VERIFIED"
            }
        );
    }
}

#[test]
fn a_transitive_family_erase_invalidates_a_required_read() {
    for erasing in [false, true] {
        let mut owner = ImageDraft::new();
        let mut draft = admitted(&mut owner);
        let sites = durable_schema(&mut draft);
        let source = ok(draft.intern_string("src/main.mw"));
        let mut callee = None;
        for name in ["erase", "relay"] {
            let mut code = if let Some(callee) = callee {
                vec![Instr::LocalGet(0), Instr::Call(callee)]
            } else if erasing {
                vec![
                    Instr::LocalGet(0),
                    Instr::DurEraseEntry(sites.entry.clone()),
                ]
            } else {
                Vec::new()
            };
            code.push(Instr::Return);
            let name = ok(draft.intern_string(name));
            let function = draft
                .add_function(FunctionDef {
                    name,
                    source,
                    params: vec![ImageType::scalar(Scalar::Text)],
                    ret: ImageType::Unit,
                    local_count: 1,
                    spans: spans(&code),
                    code,
                })
                .expect("live sites");
            callee = Some(function.index());
        }
        // A sparse erase makes both controls mutating without ending the proof.
        let code = vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(sites.entry),
            Instr::JumpIfFalse(10),
            Instr::LocalGet(0),
            Instr::DurEraseField(sites.label),
            Instr::LocalGet(1),
            Instr::Call(callee.expect("relay")),
            Instr::DurReadFieldPresent {
                site: sites.value,
                key_slots: vec![0],
            },
            Instr::Pop,
            Instr::TxnCommit,
            Instr::Return,
        ];
        assert_eq!(
            code_of(&finish_two_key(draft, code)),
            if erasing { "image.flow" } else { "VERIFIED" }
        );
    }
}
