//! Strict group replacement consumes a proof of its containing entry and exact keys.

use super::{
    APPLICATION_ID, PLACEMENT_ID, PRODUCT_ID, ROOT_KEY_ID, VALUE_FIELD_ID, admitted, admitted_plan,
    code_of, durable_schema, field_member, finish_two_key, ok, product_members, rehash,
    scalar_shapes, sections, site, spans,
};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId, FieldDef, FuncId,
    FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, PlannedSiteRef,
    RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget, TypeId,
};
use marrow_verify::{VerifyPhase, verify};

struct GroupSites {
    entry: PlannedSiteRef,
    group: PlannedSiteRef,
    sibling: PlannedSiteRef,
    other_root: PlannedSiteRef,
    group_record: TypeId,
    entry_record: TypeId,
}

fn group_draft() -> (ImageDraft, GroupSites) {
    let mut owner = ImageDraft::new();
    let mut draft = admitted(&mut owner);
    let shapes = scalar_shapes(&mut draft);
    let pages = ok(draft.intern_string("pages"));
    let group_name = ok(draft.intern_string("Details"));
    let group_record = ok(draft.add_record_type(RecordTypeDef {
        name: group_name,
        fields: vec![FieldDef {
            name: pages,
            ty: ImageType::scalar(Scalar::Int),
            required: false,
        }],
    }));
    let title = ok(draft.intern_string("title"));
    let details = ok(draft.intern_string("details"));
    let extra = ok(draft.intern_string("extra"));
    let book = ok(draft.intern_string("Book"));
    let entry_record = ok(draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![
            FieldDef {
                name: title,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
            FieldDef {
                name: details,
                ty: ImageType::Record {
                    idx: group_record,
                    optional: false,
                },
                required: true,
            },
            FieldDef {
                name: extra,
                ty: ImageType::Record {
                    idx: group_record,
                    optional: false,
                },
                required: true,
            },
        ],
    }));
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            entry_record,
            vec![
                field_member(shapes, None, VALUE_FIELD_ID, true, Scalar::Text),
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Group {
                        id: LedgerIdBytes::from_bytes([0x20; 16]),
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Group {
                        id: LedgerIdBytes::from_bytes([0x22; 16]),
                    },
                },
                field_member(shapes, Some(1), [0x21; 16], false, Scalar::Int),
                field_member(shapes, Some(2), [0x23; 16], false, Scalar::Int),
            ],
        )
        .expect("the two sparse groups match their record slots");
    let mut roots = Vec::new();
    for (name, placement, key_ids) in [
        ("books", PLACEMENT_ID, [ROOT_KEY_ID, [0x1c; 16]]),
        ("other", [0x1b; 16], [[0x2c; 16], [0x2d; 16]]),
    ] {
        let name = ok(draft.intern_string(name));
        roots.push(
            draft
                .add_root_occurrence(
                    &admitted_plan(),
                    LedgerIdBytes::from_bytes(PRODUCT_ID),
                    RootOccurrenceDef {
                        name,
                        keys: key_ids
                            .into_iter()
                            .map(|id| KeyColumn {
                                scalar: Scalar::Int,
                                id: LedgerIdBytes::from_bytes(id),
                            })
                            .collect(),
                        placement: LedgerIdBytes::from_bytes(placement),
                        indexes: Vec::new().into(),
                    },
                )
                .expect("the root is admitted"),
        );
    }
    let members = product_members(&draft);
    let sites = GroupSites {
        entry: site(
            &mut draft,
            roots[0].occurrence(),
            roots[0].placement_path(),
            SemanticTarget::WholePayload,
        ),
        group: site(
            &mut draft,
            roots[0].occurrence(),
            members[1].path(),
            SemanticTarget::GroupEntry,
        ),
        sibling: site(
            &mut draft,
            roots[0].occurrence(),
            members[2].path(),
            SemanticTarget::GroupEntry,
        ),
        other_root: site(
            &mut draft,
            roots[1].occurrence(),
            members[1].path(),
            SemanticTarget::GroupEntry,
        ),
        group_record,
        entry_record,
    };
    draft.commit();
    (owner, sites)
}

fn image(build: impl FnOnce(&mut DraftTxn<'_>, &GroupSites) -> Vec<Instr>) -> Vec<u8> {
    let (mut owner, sites) = group_draft();
    let mut draft = admitted(&mut owner);
    let code = build(&mut draft, &sites);
    let source = ok(draft.intern_string("src/main.mw"));
    let name = ok(draft.intern_string("put"));
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: vec![
                ImageType::scalar(Scalar::Int),
                ImageType::scalar(Scalar::Int),
                ImageType::scalar(Scalar::Int),
                ImageType::scalar(Scalar::Bool),
            ],
            ret: ImageType::Unit,
            local_count: 5,
            spans: spans(&code),
            code,
        })
        .expect("all sites are live");
    draft.add_export(ExportId::of_local("", "put"), function);
    draft.encode().expect("encode").bytes
}

fn group_value(draft: &mut DraftTxn<'_>, sites: &GroupSites) -> Vec<Instr> {
    let pages = ok(draft.intern_int(7));
    vec![
        Instr::ConstLoad(pages),
        Instr::SomeWrap,
        Instr::RecordNew(sites.group_record),
    ]
}

fn replacement(site: &PlannedSiteRef, keys: &[u16]) -> Instr {
    Instr::DurReplaceGroup {
        site: site.clone(),
        key_slots: keys.to_vec(),
    }
}

fn finish(mut code: Vec<Instr>, exits: &[usize]) -> Vec<Instr> {
    let commit = code.len() as u32;
    for exit in exits {
        match &mut code[*exit] {
            Instr::Jump(target) | Instr::JumpIfFalse(target) | Instr::BranchPresent(target) => {
                *target = commit;
            }
            other => panic!("expected an exit branch, got {other:?}"),
        }
    }
    code.extend([Instr::TxnCommit, Instr::Return]);
    code
}

fn optional_guard(sites: &GroupSites) -> Vec<Instr> {
    vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurReadGroup(sites.group.clone()),
        Instr::BranchPresent(0),
    ]
}

fn helper(draft: &mut DraftTxn<'_>, name: &str, code: Vec<Instr>) -> FuncId {
    let name = ok(draft.intern_string(name));
    let source = ok(draft.intern_string("src/main.mw"));
    draft
        .add_function(FunctionDef {
            name,
            source,
            params: vec![ImageType::scalar(Scalar::Int); 2],
            ret: ImageType::Unit,
            local_count: 2,
            spans: spans(&code),
            code,
        })
        .expect("the helper's sites are live")
}

#[test]
fn a_group_replacement_without_a_presence_fact_rejects() {
    let bytes = image(|draft, sites| {
        let mut code = vec![Instr::TxnBegin];
        code.extend(group_value(draft, sites));
        code.push(replacement(&sites.group, &[0, 1]));
        finish(code, &[])
    });
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn an_entry_guard_allows_a_group_replacement() {
    let bytes = image(|draft, sites| {
        let mut code = vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::DurExists(sites.entry.clone()),
            Instr::JumpIfFalse(0),
        ];
        code.extend(group_value(draft, sites));
        code.push(replacement(&sites.group, &[0, 1]));
        finish(code, &[4])
    });
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn an_optional_group_guard_proves_its_entry_for_either_group() {
    for sibling in [false, true] {
        let bytes = image(|_, sites| {
            let mut code = optional_guard(sites);
            let target = if sibling {
                &sites.sibling
            } else {
                &sites.group
            };
            code.push(replacement(target, &[0, 1]));
            finish(code, &[4])
        });
        assert_eq!(code_of(&bytes), "VERIFIED");
    }
}

#[test]
fn an_optional_group_guard_does_not_prove_another_key_or_root() {
    for other_root in [false, true] {
        let bytes = image(|_, sites| {
            let mut code = optional_guard(sites);
            code.push(if other_root {
                replacement(&sites.other_root, &[0, 1])
            } else {
                replacement(&sites.group, &[2, 1])
            });
            finish(code, &[4])
        });
        assert_eq!(code_of(&bytes), "image.flow");
    }
}

#[test]
fn a_group_replacement_requires_complete_initialized_typed_key_slots() {
    for keys in [vec![0], vec![0, 1, 2], vec![0, 3], vec![0, 4], vec![0, 5]] {
        let bytes = image(|_, sites| {
            let mut code = optional_guard(sites);
            code.push(replacement(&sites.group, &keys));
            finish(code, &[4])
        });
        assert_eq!(code_of(&bytes), "image.function");
    }
}

#[test]
fn a_group_replacement_requires_a_bare_record() {
    let bytes = image(|_, sites| {
        let mut code = optional_guard(sites);
        code.push(Instr::SomeWrap);
        code.push(replacement(&sites.group, &[0, 1]));
        finish(code, &[4])
    });
    assert_eq!(code_of(&bytes), "image.function");
}

#[test]
fn an_optional_group_guard_is_killed_by_key_rebinding() {
    let bytes = image(|_, sites| {
        let mut code = optional_guard(sites);
        code.extend([Instr::LocalGet(2), Instr::LocalSet(0)]);
        code.push(replacement(&sites.group, &[0, 1]));
        finish(code, &[4])
    });
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn an_optional_group_guard_is_killed_by_same_family_erase_at_another_key() {
    let bytes = image(|_, sites| {
        let mut code = optional_guard(sites);
        code.extend([
            Instr::LocalGet(2),
            Instr::LocalGet(1),
            Instr::DurEraseEntry(sites.entry.clone()),
        ]);
        code.push(replacement(&sites.group, &[0, 1]));
        finish(code, &[4])
    });
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn an_optional_group_guard_is_killed_by_direct_and_transitive_erase_calls() {
    for depth in [1, 2] {
        let bytes = image(|draft, sites| {
            let mut callee = helper(
                draft,
                "erase",
                vec![
                    Instr::LocalGet(0),
                    Instr::LocalGet(1),
                    Instr::DurEraseEntry(sites.entry.clone()),
                    Instr::Return,
                ],
            );
            if depth == 2 {
                callee = helper(
                    draft,
                    "relay",
                    vec![
                        Instr::LocalGet(0),
                        Instr::LocalGet(1),
                        Instr::Call(callee.index()),
                        Instr::Return,
                    ],
                );
            }
            let mut code = optional_guard(sites);
            code.extend([
                Instr::LocalGet(0),
                Instr::LocalGet(1),
                Instr::Call(callee.index()),
            ]);
            code.push(replacement(&sites.group, &[0, 1]));
            finish(code, &[4])
        });
        assert_eq!(code_of(&bytes), "image.flow");
    }
}

#[test]
fn an_optional_group_guard_survives_replacement_and_sparse_group_erasure() {
    let bytes = image(|draft, sites| {
        let title = ok(draft.intern_text("replacement"));
        let mut replace = vec![
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::ConstLoad(title),
        ];
        replace.extend(group_value(draft, sites));
        replace.extend(group_value(draft, sites));
        replace.extend([
            Instr::RecordNew(sites.entry_record),
            Instr::DurReplaceEntry(sites.entry.clone()),
            Instr::Return,
        ]);
        let replace = helper(draft, "replace", replace);
        let erase = helper(
            draft,
            "eraseGroup",
            vec![
                Instr::LocalGet(0),
                Instr::LocalGet(1),
                Instr::DurEraseGroup(sites.group.clone()),
                Instr::Return,
            ],
        );
        let mut code = optional_guard(sites);
        code.extend([
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::Call(replace.index()),
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::DurEraseGroup(sites.group.clone()),
            Instr::LocalGet(0),
            Instr::LocalGet(1),
            Instr::Call(erase.index()),
        ]);
        code.push(replacement(&sites.group, &[0, 1]));
        finish(code, &[4])
    });
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn the_absent_group_read_edge_does_not_prove_presence() {
    let bytes = image(|draft, sites| {
        let mut code = optional_guard(sites);
        code.extend([Instr::Pop, Instr::Jump(0)]);
        let absent = code.len() as u32;
        code[4] = Instr::BranchPresent(absent);
        code.extend(group_value(draft, sites));
        code.push(replacement(&sites.group, &[0, 1]));
        finish(code, &[6])
    });
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn an_arbitrary_optional_group_record_does_not_prove_presence() {
    let bytes = image(|draft, sites| {
        let mut code = vec![Instr::TxnBegin];
        code.extend(group_value(draft, sites));
        code.push(Instr::SomeWrap);
        let branch = code.len();
        code.push(Instr::BranchPresent(0));
        code.push(replacement(&sites.group, &[0, 1]));
        finish(code, &[branch])
    });
    assert_eq!(code_of(&bytes), "image.flow");
}

#[test]
fn a_composite_group_guard_requires_its_complete_producer_window() {
    for entry in 0..=3 {
        let bytes = image(|draft, sites| {
            let mut code = vec![Instr::TxnBegin, Instr::LocalGet(3), Instr::JumpIfFalse(0)];
            match entry {
                0 => {}
                1 => code.push(Instr::LocalGet(2)),
                2 => code.extend([Instr::LocalGet(2), Instr::LocalGet(1)]),
                3 => {
                    code.extend(group_value(draft, sites));
                    code.push(Instr::SomeWrap);
                }
                _ => unreachable!(),
            }
            let jump = code.len();
            code.push(Instr::Jump(0));
            let first_key = code.len();
            code[2] = Instr::JumpIfFalse(first_key as u32);
            code[jump] = Instr::Jump((first_key + entry) as u32);
            code.extend([
                Instr::LocalGet(0),
                Instr::LocalGet(1),
                Instr::DurReadGroup(sites.group.clone()),
                Instr::BranchPresent(0),
                replacement(&sites.group, &[0, 1]),
            ]);
            finish(code, &[first_key + 3])
        });
        assert_eq!(
            code_of(&bytes),
            if entry == 0 { "VERIFIED" } else { "image.flow" },
        );
    }
}

#[test]
fn the_active_family_exists_opcode_still_verifies() {
    assert_eq!(marrow_image::OP_DUR_FAMILY_EXISTS, 0x39);
    let mut owner = ImageDraft::new();
    let mut draft = admitted(&mut owner);
    let sites = durable_schema(&mut draft);
    let bytes = finish_two_key(
        draft,
        vec![
            Instr::DurFamilyExists(sites.entry),
            Instr::Pop,
            Instr::Return,
        ],
    );
    assert_eq!(code_of(&bytes), "VERIFIED");
}

#[test]
fn retired_entry_mutation_bytes_are_unknown_opcodes() {
    let mut owner = ImageDraft::new();
    let mut draft = admitted(&mut owner);
    let source = ok(draft.intern_string("src/main.mw"));
    let name = ok(draft.intern_string("empty"));
    let code = vec![Instr::Return];
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            spans: spans(&code),
            code,
        })
        .expect("a storeless function is admitted");
    draft.add_export(ExportId::of_local("", "empty"), function);
    let original = draft.encode().expect("encode").bytes;
    assert_eq!(code_of(&original), "VERIFIED");
    let (_, offset, len) = sections(&original)
        .into_iter()
        .find(|(id, _, _)| *id == 5)
        .expect("functions section");
    // One zero-parameter Unit function: count, name, source, parameter count,
    // return type, local count, code length, then its one Return opcode.
    let code_len_at = offset + 2 + 2 + 2 + 1 + 1 + 2;
    assert_eq!(&original[code_len_at..code_len_at + 4], &[0, 0, 0, 1]);
    let opcode_at = code_len_at + 4;
    assert_eq!(opcode_at + 1, offset + len);
    assert_eq!(original[opcode_at], marrow_image::OP_RETURN);
    for retired in [0x2C, 0x33, 0x34, 0x3A] {
        let mut bytes = original.clone();
        bytes[opcode_at] = retired;
        rehash(&mut bytes);
        let rejection =
            verify(&bytes).expect_err("the retired byte is refused before operand decoding");
        assert_eq!(rejection.phase(), VerifyPhase::Function);
        assert_eq!(rejection.code(), "image.function");
        assert_eq!(rejection.detail(), "unknown or not-yet-supported opcode");
    }
}
