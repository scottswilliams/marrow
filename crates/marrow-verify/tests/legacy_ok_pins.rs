//! Producer-refusal pins for the hoisted structural references and relations: drafts
//! that ENCODED before the coherence hoist — leaving the independent verifier as the
//! only owner that refused them — and are now refused by the producer's coherence
//! walk with the exact typed payload each pin states. Every flip cites its
//! pre-restructure Ok-pin, whose baseline git history carries.
//!
//! The clean twins still encode and verify, proving each refusal comes from the one
//! defect and not from the fixture's shape; the verifier remains the independent
//! decoder of whatever the producer emits.

use marrow_image::{
    AdmittedRoot, CollTypeId, CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape,
    DraftTxn, EncodedImage, EnumId, EnumTypeDef, ExportId, FieldDef, FuncId, FunctionDef,
    ImageBuildError, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootId,
    RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry, TypeId, VariantDef,
};
use marrow_image::{DurableIndexComponent, DurableIndexShape};
use marrow_verify::verify;

#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

/// The armed transaction a fresh savepoint admits over `owner`.
fn admitted(owner: &mut ImageDraft) -> DraftTxn<'_> {
    owner
        .begin_transaction(owner.savepoint())
        .expect("a fresh savepoint admits")
}

/// A type reference naming a TYPES row no fixture declares.
const FORGED_TYPE: ImageType = ImageType::Record {
    idx: TypeId::from_index(u16::MAX),
    optional: false,
};

/// A minimal exported `main`: one function, one constant, one export, no durable graph.
fn main_draft(params: Vec<ImageType>, code: Vec<Instr>) -> ImageDraft {
    main_draft_with_id(params, code).0
}

/// [`main_draft`] plus `main`'s own `FuncId`, for the relation pins that need to name
/// the exported function a second time.
fn main_draft_with_id(params: Vec<ImageType>, code: Vec<Instr>) -> (ImageDraft, FuncId) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    draft.intern_int(0);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            local_count: params.len() as u16,
            params,
            ret: ImageType::scalar(Scalar::Int),
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.commit();
    (draft_owner, main)
}

/// An unexported companion function with the given return type and body, for the
/// test-entry relation pins.
fn add_plain_function(
    draft: &mut DraftTxn<'_>,
    name: &str,
    ret: ImageType,
    code: Vec<Instr>,
) -> FuncId {
    let src = draft.intern_string("src/tests.mw");
    let fname = draft.intern_string(name);
    draft
        .add_function(FunctionDef {
            name: fname,
            source: src,
            params: Vec::new(),
            ret,
            local_count: 0,
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
            code,
        })
        .expect("every site operand is live")
}

fn short_code() -> Vec<Instr> {
    vec![
        Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
        Instr::Return,
    ]
}

fn clean_image() -> EncodedImage {
    main_draft(Vec::new(), short_code())
        .encode()
        .expect("the clean twin encodes")
}

/// A function index no row of a one-function draft answers, minted by a draft that
/// holds two: a `FuncId` is a table position, not a capability bound to its draft.
fn forged_func_id() -> FuncId {
    let mut other_owner = ImageDraft::new();
    let mut other = admitted(&mut other_owner);
    let src = other.intern_string("s");
    let name = other.intern_string("f");
    other.intern_int(0);
    let def = FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: vec![SpanEntry {
            instr_index: 0,
            line: 1,
            column: 1,
        }],
        code: vec![
            Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
            Instr::Return,
        ],
    };
    other
        .add_function(def.clone())
        .expect("every site operand is live");
    other.add_function(def).expect("every site operand is live")
}

/// The clean twin the four pins below vary one reference from.
#[test]
fn the_clean_twin_verifies() {
    let outcome = verify(&clean_image().bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("call target")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_call_target_draws_the_call_target_refusal() {
    assert_eq!(
        main_draft(Vec::new(), vec![Instr::Call(u16::MAX), Instr::Return])
            .encode()
            .map(|_| ()),
        Err(ImageBuildError::InvalidReference("call target")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("export target")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_export_target_draws_the_export_target_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    draft.add_export(ExportId::of_local("", "ghost"), forged_func_id());
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("export target")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test target")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_test_entry_target_draws_the_test_target_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    let entry_name = draft.intern_string("t");
    draft.add_test_entry(entry_name, forged_func_id());
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("test target")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("type table")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_param_type_draws_the_type_table_refusal() {
    assert_eq!(
        main_draft(vec![FORGED_TYPE], short_code())
            .encode()
            .map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

// ---- The remaining §B.3 reference families: each raw table ordinal the encoder
// once wrote unchecked, pinned standalone as the producer refusal the coherence
// hoist installed. The `DurIterateBounded`/`DurIndexScan` `list_ty` operand belongs
// here too — a live site operand carries the instruction while its `list_ty` is a
// public raw ordinal that dangles — and both opcode paths are pinned below the
// durable fixture.

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("type table")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_record_new_ordinal_draws_the_type_table_refusal() {
    assert_eq!(
        main_draft(
            Vec::new(),
            vec![
                Instr::RecordNew(marrow_image::TypeId::from_index(u16::MAX)),
                Instr::Return
            ]
        )
        .encode()
        .map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("collection type")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_list_new_ordinal_draws_the_collection_type_refusal() {
    assert_eq!(
        main_draft(
            Vec::new(),
            vec![
                Instr::ListNew(marrow_image::CollTypeId::from_index(u16::MAX)),
                Instr::Return
            ]
        )
        .encode()
        .map(|_| ()),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("enum type")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_enum_construct_ordinal_draws_the_enum_type_refusal() {
    assert_eq!(
        main_draft(
            Vec::new(),
            vec![
                Instr::EnumConstruct {
                    enum_idx: EnumId::from_index(u16::MAX),
                    variant: 0,
                },
                Instr::Return,
            ],
        )
        .encode()
        .map(|_| ()),
        Err(ImageBuildError::InvalidReference("enum type")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("type table")` before any byte is measured or emitted.
/// The corrected twin — the same fixture with a TYPES row answering index 0 — still
/// encodes and verifies, so the refusal is the forged ordinal's alone. (The operand
/// stays optional: the coherence check is the domain range check; optionality remains
/// the verifier's law.)
#[test]
fn an_out_of_range_vacant_load_type_draws_the_type_table_refusal() {
    let body = |idx: u16| {
        vec![
            Instr::VacantLoad(ImageType::Record {
                idx: TypeId::from_index(idx),
                optional: true,
            }),
            Instr::Pop,
            Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
            Instr::Return,
        ]
    };
    let corrected = with_decoy_record(main_draft(Vec::new(), body(0)))
        .encode()
        .expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        with_decoy_record(main_draft(Vec::new(), body(u16::MAX)))
            .encode()
            .map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("root table")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_make_identity_root_draws_the_root_table_refusal() {
    assert_eq!(
        main_draft(
            Vec::new(),
            vec![
                Instr::MakeIdentity {
                    root: RootId::from_index(u16::MAX),
                    cols: 0,
                },
                Instr::Return,
            ],
        )
        .encode()
        .map(|_| ()),
        Err(ImageBuildError::InvalidReference("root table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("type table")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_field_type_draws_the_type_table_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    let name = draft.intern_string("R");
    let field_name = draft.intern_string("f");
    draft.add_record_type(RecordTypeDef {
        name,
        fields: vec![FieldDef {
            name: field_name,
            ty: FORGED_TYPE,
            required: true,
        }],
    });
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("type table")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_enum_payload_type_draws_the_type_table_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    let name = draft.intern_string("P");
    let variant_name = draft.intern_string("pv");
    draft.add_enum_type(EnumTypeDef {
        name,
        variants: vec![VariantDef {
            name: variant_name,
            category: false,
            payload: vec![FORGED_TYPE],
        }],
    });
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("type table")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_collection_elem_type_draws_the_type_table_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    draft.add_collection_type(CollectionTypeDef::List { elem: FORGED_TYPE });
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

// ---- The two omitted DURABLE type-table ordinals and the `MakeIdentity` cols
// relation (design draft 7 §B.3). `TypeId` is a raw newtype with a public
// `from_index`, so both record ordinals are forged directly; both now draw the
// producer's type-table refusal at their exact body positions.

/// How the durable fixture's two forgeable record references are shaped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TableRef {
    Valid,
    Forged,
}

impl TableRef {
    fn resolve(self, valid: TypeId) -> TypeId {
        match self {
            TableRef::Valid => valid,
            TableRef::Forged => TypeId::from_index(u16::MAX),
        }
    }
}

/// One keyed root over one Product with one `int` field, plus the exported `main`:
/// the durable clean shape the pins below vary one reference from. `branch` adds an
/// otherwise-valid keyed branch member with the given entry record.
fn durable_draft(entry: TableRef, branch: Option<TableRef>, code: Vec<Instr>) -> ImageDraft {
    let (owner, _root) = durable_parts(entry, branch, false);
    finish_main(owner, code, ImageType::scalar(Scalar::Int))
}

/// The durable graph of [`durable_draft`], before `main` is added, returning the
/// admitted root so a caller can bind operation sites. `indexed` gives the root one
/// nonunique managed index projecting the field then the identity key.
fn durable_parts(
    entry: TableRef,
    branch: Option<TableRef>,
    indexed: bool,
) -> (ImageDraft, AdmittedRoot) {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let value = draft.value_scalar(Scalar::Int);
    let type_name = draft.intern_string("R");
    // The verifier ties each field/group member to one record slot (a keyed branch is
    // a distinct durable node, not a slot), so the entry record declares exactly one
    // field for the declaration's one field member.
    let field_name = draft.intern_string("f0");
    let fields = vec![FieldDef {
        name: field_name,
        ty: ImageType::scalar(Scalar::Int),
        required: true,
    }];
    let record = draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields,
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    let mut members = vec![DeclarationMemberDef {
        parent: None,
        shape: DeclarationMemberShape::Field {
            id: LedgerIdBytes::from_bytes([0x0e; 16]),
            required: true,
            value,
        },
    }];
    if let Some(branch) = branch {
        // The branch declares no members, so its own entry record is a fieldless type.
        let branch_type_name = draft.intern_string("B");
        let branch_record = draft.add_record_type(RecordTypeDef {
            name: branch_type_name,
            fields: Vec::new(),
        });
        let branch_name = draft.intern_string("b");
        members.push(DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Branch {
                placement: LedgerIdBytes::from_bytes([0x21; 16]),
                name: branch_name,
                record: branch.resolve(branch_record),
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes([0x22; 16]),
                }],
            },
        });
    }
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes([0x0d; 16]),
            entry.resolve(record),
            members,
        )
        .expect("a well-formed declaration");
    let root_name = draft.intern_string("r");
    let indexes = if indexed {
        vec![DurableIndexShape {
            id: LedgerIdBytes::from_bytes([0x23; 16]),
            unique: false,
            components: vec![
                DurableIndexComponent::Field(LedgerIdBytes::from_bytes([0x0e; 16])),
                DurableIndexComponent::Key(LedgerIdBytes::from_bytes([0x0c; 16])),
            ],
        }]
    } else {
        Vec::new()
    };
    let root = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes([0x0d; 16]),
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes([0x0c; 16]),
                }],
                placement: LedgerIdBytes::from_bytes([0x0b; 16]),
                indexes: indexes.into(),
            },
        )
        .expect("the Product is declared");
    draft.commit();
    (draft_owner, root)
}

/// Add and export `main` over an already-built durable graph.
fn finish_main(mut owner: ImageDraft, code: Vec<Instr>, ret: ImageType) -> ImageDraft {
    let mut draft = admitted(&mut owner);
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    draft.intern_int(0);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret,
            local_count: 0,
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.commit();
    owner
}

/// The durable clean twin (with and without a valid branch) verifies, so each
/// rejection below comes from the one forged reference.
#[test]
fn the_durable_clean_twin_verifies() {
    let plain = durable_draft(TableRef::Valid, None, short_code())
        .encode()
        .expect("the durable clean twin encodes");
    let outcome = verify(&plain.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    let branched = durable_draft(TableRef::Valid, Some(TableRef::Valid), short_code())
        .encode()
        .expect("the branched clean twin encodes");
    let outcome = verify(&branched.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("type table")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_root_entry_record_draws_the_type_table_refusal() {
    assert_eq!(
        durable_draft(TableRef::Forged, None, short_code())
            .encode()
            .map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("type table")` before any byte is measured or emitted.
#[test]
fn an_out_of_range_branch_record_draws_the_type_table_refusal() {
    assert_eq!(
        durable_draft(TableRef::Valid, Some(TableRef::Forged), short_code())
            .encode()
            .map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("root table")` before any byte is measured or emitted.
#[test]
fn a_make_identity_cols_arity_mismatch_draws_the_root_table_refusal() {
    assert_eq!(
        durable_draft(
            TableRef::Valid,
            None,
            vec![
                Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
                Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
                Instr::MakeIdentity {
                    root: RootId::from_index(0),
                    cols: 2
                },
                Instr::Return,
            ],
        )
        .encode()
        .map(|_| ()),
        Err(ImageBuildError::InvalidReference("root table")),
    );
}

// ---- The `list_ty` family, both opcode paths: a live provenance-validated site
// operand carries the instruction, while its `list_ty` — a public raw COLLTYPES
// ordinal — dangles past the (empty) collection table.

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("collection type")` before any byte is measured or emitted.
#[test]
fn a_dangling_iterate_list_type_draws_the_collection_type_refusal() {
    let (mut draft_owner, root) = durable_parts(TableRef::Valid, None, false);
    let mut draft = admitted(&mut draft_owner);
    let handle = draft
        .bind_occurrence_site(
            root.occurrence(),
            root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("a keyed placement");
    let site = draft.request_site(&handle).expect("a live demand");
    let code = vec![
        Instr::DurIterateBounded {
            site,
            limit: 2,
            from: false,
            list_ty: CollTypeId::from_index(u16::MAX),
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    assert_eq!(
        finish_main(
            {
                draft.commit();
                draft_owner
            },
            code,
            ImageType::Unit
        )
        .encode()
        .map(|_| ()),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("collection type")` before any byte is measured or emitted.
/// The corrected twin — the same fixture naming the real `List[int]` row — still
/// encodes and verifies, so the refusal is the dangling ordinal's alone; the deeper
/// list-of-the-identity-key law remains the verifier's.
#[test]
fn a_dangling_index_scan_list_type_draws_the_collection_type_refusal() {
    let scan_draft = |list_ty: CollTypeId| {
        let (mut draft_owner, root) = durable_parts(TableRef::Valid, None, true);
        let mut draft = admitted(&mut draft_owner);
        // COLLTYPES row 0: the `List[int]` a corrected scan freezes its keys into.
        draft.add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        });
        let scan_path = root.index_paths()[0].clone();
        let handle = draft
            .bind_occurrence_site(root.occurrence(), &scan_path, SemanticTarget::IndexScan)
            .expect("a managed index");
        let site = draft.request_site(&handle).expect("a live demand");
        // The scan pops its held field-component prefix (one `int`) before the list
        // check, and a corrected scan pushes the frozen list then the truncation flag —
        // both popped before the unit return.
        let code = vec![
            Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
            Instr::DurIndexScan {
                site,
                limit: 2,
                from: false,
                list_ty,
            },
            Instr::Pop,
            Instr::Pop,
            Instr::Return,
        ];
        finish_main(
            {
                draft.commit();
                draft_owner
            },
            code,
            ImageType::Unit,
        )
    };
    let corrected = scan_draft(CollTypeId::from_index(0))
        .encode()
        .expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        scan_draft(CollTypeId::from_index(u16::MAX))
            .encode()
            .map(|_| ()),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

// ---- Domain-decoy `ImageType` pins (review 7 item 5): index 0 into an EMPTY target
// domain while a WRONG domain is populated at index 0, asserting the exact
// target-domain refusal — a check consulting the wrong table would accept these.

/// A fieldless record populating TYPES row 0, as decoy for the non-record domains.
fn with_decoy_record(mut owner: ImageDraft) -> ImageDraft {
    let mut draft = admitted(&mut owner);
    let name = draft.intern_string("Decoy");
    draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    draft.commit();
    owner
}

/// A payloadless enum populating ENUMS row 0, as decoy for the record domain.
fn with_decoy_enum(mut owner: ImageDraft) -> ImageDraft {
    let mut draft = admitted(&mut owner);
    let name = draft.intern_string("DecoyEnum");
    let variant = draft.intern_string("dv");
    draft.add_enum_type(EnumTypeDef {
        name,
        variants: vec![VariantDef {
            name: variant,
            category: false,
            payload: Vec::new(),
        }],
    });
    draft.commit();
    owner
}

/// Flipped by the coherence hoist, citing the pre-restructure decoy pin this test
/// carried: with TYPES empty and ENUMS populated at index 0, a producer check
/// consulting the wrong table would accept this draft; the exact
/// `InvalidReference("type table")` refusal pins the correct domain.
#[test]
fn a_record_type_decoy_draws_the_types_domain_refusal() {
    let draft = with_decoy_enum(main_draft(
        vec![ImageType::Record {
            idx: TypeId::from_index(0),
            optional: false,
        }],
        short_code(),
    ));
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure decoy pin this test
/// carried: with ENUMS empty and TYPES populated at index 0, a producer check
/// consulting the wrong table would accept this draft; the exact
/// `InvalidReference("enum type")` refusal pins the correct domain.
#[test]
fn an_enum_type_decoy_draws_the_enums_domain_refusal() {
    let draft = with_decoy_record(main_draft(
        vec![ImageType::Enum {
            idx: EnumId::from_index(0),
            optional: false,
        }],
        short_code(),
    ));
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("enum type")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure decoy pin this test
/// carried: with COLLTYPES empty and TYPES populated at index 0, a producer check
/// consulting the wrong table would accept this draft; the exact
/// `InvalidReference("collection type")` refusal pins the correct domain.
#[test]
fn a_collection_type_decoy_draws_the_colltypes_domain_refusal() {
    let draft = with_decoy_record(main_draft(
        vec![ImageType::Collection {
            idx: CollTypeId::from_index(0),
            optional: false,
        }],
        short_code(),
    ));
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure decoy pin this test
/// carried: with ROOTS empty and TYPES populated at index 0, a producer check
/// consulting the wrong table would accept this draft; the exact
/// `InvalidReference("root table")` refusal pins the correct domain.
#[test]
fn an_identity_type_decoy_draws_the_roots_domain_refusal() {
    let draft = with_decoy_record(main_draft(
        vec![ImageType::Identity {
            root: RootId::from_index(0),
            optional: false,
        }],
        short_code(),
    ));
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("root table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("enum type")` before any byte is measured or emitted.
/// The subordinate `EnumConstruct.variant` ordinal is checked against the resolved
/// enum, not a table of its own. The corrected twin — variant 0, the decoy enum's one
/// payloadless member — still encodes and verifies.
#[test]
fn an_out_of_range_enum_construct_variant_draws_the_enum_type_refusal() {
    let body = |variant: u16| {
        vec![
            Instr::EnumConstruct {
                enum_idx: EnumId::from_index(0),
                variant,
            },
            Instr::Pop,
            Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
            Instr::Return,
        ]
    };
    let corrected = with_decoy_enum(main_draft(Vec::new(), body(0)))
        .encode()
        .expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    // The decoy enum is the one ENUMS row; variant 5 names no member of it.
    assert_eq!(
        with_decoy_enum(main_draft(Vec::new(), body(5)))
            .encode()
            .map(|_| ()),
        Err(ImageBuildError::InvalidReference("enum type")),
    );
}

// ---- The remaining collection-ordinal opcode (design draft 8 §B.3): `MapNew` shares
// `ListNew`'s operand kind, tape position, and hoisted check arm;
// `TextSplit`/`TextLines` derive from these two by the derivation law in
// `measure_verdicts.rs`.

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("collection type")` before any byte is measured or emitted.
/// The corrected twin — the same fixture naming the real Map row — still encodes and
/// verifies, so the refusal is the dangling ordinal's alone.
#[test]
fn an_out_of_range_map_new_ordinal_draws_the_collection_type_refusal() {
    let with_map_row = |code: Vec<Instr>| {
        let mut draft_owner = main_draft(Vec::new(), code);
        let mut draft = admitted(&mut draft_owner);
        draft.add_collection_type(CollectionTypeDef::Map {
            key: ImageType::scalar(Scalar::Int),
            value: ImageType::scalar(Scalar::Int),
        });
        draft.commit();
        draft_owner
    };
    let body = |idx: u16| {
        vec![
            Instr::MapNew(CollTypeId::from_index(idx)),
            Instr::Pop,
            Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
            Instr::Return,
        ]
    };
    let corrected = with_map_row(body(0))
        .encode()
        .expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        with_map_row(body(u16::MAX)).encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

// ---- The non-range export/test relations (design drafts 8 §B.3 and review 9): rows
// the public draft APIs accept unchecked, now refused by the coherence walk at the
// EXPORTS and TEST-ENTRY positions — the target relations, the id relation, both
// test-signature decision sites, and calls into test entries (the draft-8
// call-closure exclusion was false; the verifier scans direct tape call targets in
// its seal phase, and the producer mirrors exactly that direct scan). Their policy
// crossings sit in `measure_verdicts.rs`; every crossing now resolves to the coherence
// side, per the derivation law recorded there.

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("export table")` before any byte is measured or emitted.
#[test]
fn a_duplicate_export_target_draws_the_export_table_refusal() {
    let (mut draft_owner, main) = main_draft_with_id(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    draft.add_export(ExportId::of_local("", "again"), main);
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("export table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test table")` before any byte is measured or emitted.
#[test]
fn a_duplicate_test_target_draws_the_test_table_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    let test_fn = add_plain_function(&mut draft, "t", ImageType::Unit, vec![Instr::Return]);
    let first = draft.intern_string("ta");
    let second = draft.intern_string("tb");
    draft.add_test_entry(first, test_fn);
    draft.add_test_entry(second, test_fn);
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("test table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test table")` before any byte is measured or emitted.
#[test]
fn a_duplicate_test_name_draws_the_test_table_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    let first = add_plain_function(&mut draft, "t1", ImageType::Unit, vec![Instr::Return]);
    let second = add_plain_function(&mut draft, "t2", ImageType::Unit, vec![Instr::Return]);
    let name = draft.intern_string("t");
    draft.add_test_entry(name, first);
    draft.add_test_entry(name, second);
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("test table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test table")` before any byte is measured or emitted.
/// The corrected twin — the same entry without the export — still encodes and
/// verifies, so the refusal is the overlap relation's alone.
#[test]
fn an_export_test_overlap_draws_the_test_table_refusal() {
    let build = |exported: bool| {
        let mut draft_owner = main_draft(Vec::new(), short_code());
        let mut draft = admitted(&mut draft_owner);
        let test_fn = add_plain_function(&mut draft, "t", ImageType::Unit, vec![Instr::Return]);
        if exported {
            draft.add_export(ExportId::of_local("", "t"), test_fn);
        }
        let name = draft.intern_string("tn");
        draft.add_test_entry(name, test_fn);
        draft.commit();
        draft_owner
    };
    let corrected = build(false).encode().expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        build(true).encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("test table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("export table")` before any byte is measured or emitted.
#[test]
fn a_duplicate_export_id_draws_the_export_table_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    // A second structurally valid function, exported under `main`'s exact id.
    let second = add_plain_function(
        &mut draft,
        "g",
        ImageType::scalar(Scalar::Int),
        vec![
            Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
            Instr::Return,
        ],
    );
    draft.add_export(ExportId::of_local("", "main"), second);
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("export table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test table")` before any byte is measured or emitted.
/// The parameter site is the FIRST decision site of the test signature law, decided
/// before the return-shape site. The corrected twin — the same entry over a
/// zero-parameter unit function — still encodes and verifies.
#[test]
fn a_test_entry_with_params_draws_the_test_table_refusal() {
    let build = |params: Vec<ImageType>| {
        let mut draft_owner = main_draft(Vec::new(), short_code());
        let mut draft = admitted(&mut draft_owner);
        let src = draft.intern_string("src/tests.mw");
        let fname = draft.intern_string("t");
        let local_count = params.len() as u16;
        let test_fn = draft
            .add_function(FunctionDef {
                name: fname,
                source: src,
                params,
                ret: ImageType::Unit,
                local_count,
                spans: vec![SpanEntry {
                    instr_index: 0,
                    line: 1,
                    column: 1,
                }],
                code: vec![Instr::Return],
            })
            .expect("every site operand is live");
        let name = draft.intern_string("tn");
        draft.add_test_entry(name, test_fn);
        draft.commit();
        draft_owner
    };
    let corrected = build(Vec::new())
        .encode()
        .expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        build(vec![ImageType::scalar(Scalar::Int)])
            .encode()
            .map(|_| ()),
        Err(ImageBuildError::InvalidReference("test table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test table")` before any byte is measured or emitted.
/// The corrected twin — the SAME asserting body inside a registered test entry —
/// still encodes and verifies, so the refusal is the membership relation's alone.
#[test]
fn an_assert_outside_a_test_entry_draws_the_test_table_refusal() {
    let assert_body = |draft: &mut DraftTxn<'_>| {
        let truth = draft.intern_bool(true);
        vec![Instr::ConstLoad(truth), Instr::Assert, Instr::Return]
    };
    let mut corrected_owner = main_draft(Vec::new(), short_code());
    let mut corrected = admitted(&mut corrected_owner);
    let code = assert_body(&mut corrected);
    let test_fn = add_plain_function(&mut corrected, "t", ImageType::Unit, code);
    let name = corrected.intern_string("tn");
    corrected.add_test_entry(name, test_fn);
    let corrected = corrected.encode().expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");

    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    let code = assert_body(&mut draft);
    let asserting = add_plain_function(&mut draft, "t", ImageType::Unit, code);
    draft.add_export(ExportId::of_local("", "t"), asserting);
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("test table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test table")` before any byte is measured or emitted.
/// The corrected twin — the same direct-durable test without the owner call — still
/// encodes and verifies, so the refusal is the mix's alone.
#[test]
fn a_test_driver_mix_draws_the_test_table_refusal() {
    let build = |drives_owner: bool| {
        let (mut draft_owner, root) = durable_parts(TableRef::Valid, None, false);
        let mut draft = admitted(&mut draft_owner);
        let handle = draft
            .bind_occurrence_site(
                root.occurrence(),
                root.placement_path(),
                SemanticTarget::WholePayload,
            )
            .expect("a keyed placement");
        let owner_site = draft.request_site(&handle).expect("a live demand");
        draft.intern_int(0);
        // Ordinal 0: the transaction-owning export the mixed test drives (its
        // transaction must itself perform a durable operation to be a valid owner).
        let owner = add_plain_function(
            &mut draft,
            "put",
            ImageType::Unit,
            vec![
                Instr::TxnBegin,
                Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
                Instr::DurExists(owner_site),
                Instr::Pop,
                Instr::TxnCommit,
                Instr::Return,
            ],
        );
        assert_eq!(owner.index(), 0, "the call names the owner");
        draft.add_export(ExportId::of_local("", "put"), owner);
        let site = draft.request_site(&handle).expect("a live demand");
        // The direct durable op (an existence probe over the root key)...
        let mut code = vec![
            Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
            Instr::DurExists(site),
            Instr::Pop,
        ];
        // ...beside the drive of the transaction owner.
        if drives_owner {
            code.push(Instr::Call(0));
        }
        code.push(Instr::Return);
        let test_fn = add_plain_function(&mut draft, "t", ImageType::Unit, code);
        let name = draft.intern_string("tn");
        draft.add_test_entry(name, test_fn);
        finish_main(
            {
                draft.commit();
                draft_owner
            },
            short_code(),
            ImageType::scalar(Scalar::Int),
        )
    };
    let corrected = build(false).encode().expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        build(true).encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("test table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test table")` before any byte is measured or emitted.
/// The corrected twin — the same call with the callee not registered as a test —
/// still encodes and verifies, so the refusal is the entry-point relation's alone.
#[test]
fn a_call_into_a_test_entry_draws_the_test_table_refusal() {
    // `main` is ordinal 0, so the companion the call names is ordinal 1.
    let build = |tested: bool| {
        let mut draft_owner = main_draft(
            Vec::new(),
            vec![
                Instr::Call(1),
                Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
                Instr::Return,
            ],
        );
        let mut draft = admitted(&mut draft_owner);
        let callee = add_plain_function(&mut draft, "t", ImageType::Unit, vec![Instr::Return]);
        assert_eq!(callee.index(), 1, "the call names the companion");
        if tested {
            let name = draft.intern_string("tn");
            draft.add_test_entry(name, callee);
        }
        draft.commit();
        draft_owner
    };
    let corrected = build(false).encode().expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(
        build(true).encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("test table")),
    );
}

/// Flipped by the coherence hoist, citing the pre-restructure Ok-pin this test
/// carried: the producer now refuses this draft with
/// `InvalidReference("test table")` before any byte is measured or emitted.
/// The return-shape site: the SECOND decision site of the test signature law.
#[test]
fn a_bad_test_signature_draws_the_test_table_refusal() {
    let mut draft_owner = main_draft(Vec::new(), short_code());
    let mut draft = admitted(&mut draft_owner);
    // A structurally valid int function, wrong only as a TEST target.
    let test_fn = add_plain_function(
        &mut draft,
        "t",
        ImageType::scalar(Scalar::Int),
        vec![
            Instr::ConstLoad(marrow_image::ConstId::from_index(0)),
            Instr::Return,
        ],
    );
    let name = draft.intern_string("tn");
    draft.add_test_entry(name, test_fn);
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::InvalidReference("test table")),
    );
}
