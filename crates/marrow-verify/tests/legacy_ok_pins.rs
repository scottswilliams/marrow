//! Structural Ok-pins: drafts the producer ENCODES today although the reference they
//! carry answers to no table row, leaving the independent verifier as the only owner
//! that refuses them.
//!
//! Each case pins both halves of that split — `encode()` returns `Ok` and
//! `verify(&bytes)` returns `Err` — so the coherence hoist has an executable baseline
//! for the producer-side conversion. Each clean twin is asserted to verify, proving the
//! rejection comes from the one defect and not from the fixture's shape.

use marrow_image::{
    AdmittedRoot, CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape, EncodedImage,
    EnumTypeDef, ExportId, FieldDef, FuncId, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn,
    LedgerIdBytes, RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry, TypeId,
    VariantDef,
};
use marrow_image::{DurableIndexComponent, DurableIndexShape};
use marrow_verify::verify;

#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

/// A type reference naming a TYPES row no fixture declares.
const FORGED_TYPE: ImageType = ImageType::Record {
    idx: u16::MAX,
    optional: false,
};

/// A minimal exported `main`: one function, one constant, one export, no durable graph.
fn main_draft(params: Vec<ImageType>, code: Vec<Instr>) -> ImageDraft {
    main_draft_with_id(params, code).0
}

/// [`main_draft`] plus `main`'s own `FuncId`, for the relation pins that need to name
/// the exported function a second time.
fn main_draft_with_id(params: Vec<ImageType>, code: Vec<Instr>) -> (ImageDraft, FuncId) {
    let mut draft = ImageDraft::new();
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
    (draft, main)
}

/// An unexported companion function with the given return type and body, for the
/// test-entry relation pins.
fn add_plain_function(
    draft: &mut ImageDraft,
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
    vec![Instr::ConstLoad(0), Instr::Return]
}

fn clean_image() -> EncodedImage {
    main_draft(Vec::new(), short_code())
        .encode()
        .expect("the clean twin encodes")
}

/// A function index no row of a one-function draft answers, minted by a draft that
/// holds two: a `FuncId` is a table position, not a capability bound to its draft.
fn forged_func_id() -> FuncId {
    let mut other = ImageDraft::new();
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
        code: vec![Instr::ConstLoad(0), Instr::Return],
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

/// The coherence hoist will convert this Ok to `InvalidReference("call target")`; the
/// flip must cite this pin: today a `Call` naming no function row still encodes, and
/// only the verifier refuses the bytes.
#[test]
fn an_out_of_range_call_target_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(Vec::new(), vec![Instr::Call(u16::MAX), Instr::Return])
        .encode()
        .expect("the producer accepts the unanswered call target today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("export target")`; the
/// flip must cite this pin: today an export naming no function row still encodes, and
/// only the verifier refuses the bytes.
#[test]
fn an_out_of_range_export_target_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    draft.add_export(ExportId::of_local("", "ghost"), forged_func_id());
    let image = draft
        .encode()
        .expect("the producer accepts the unanswered export target today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("test target")`; the
/// flip must cite this pin: today a test entry naming no function row still encodes,
/// and only the verifier refuses the bytes.
#[test]
fn an_out_of_range_test_entry_target_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    let entry_name = draft.intern_string("t");
    draft.add_test_entry(entry_name, forged_func_id());
    let image = draft
        .encode()
        .expect("the producer accepts the unanswered test target today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today a parameter type naming no TYPES row still encodes
/// (`ImageType::Record` is publicly constructible over any raw index), and only the
/// verifier refuses the bytes.
#[test]
fn an_out_of_range_param_type_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(vec![FORGED_TYPE], short_code())
        .encode()
        .expect("the producer accepts the unanswered type index today");
    assert!(verify(&image.bytes).is_err());
}

// ---- The remaining §B.3 reference families: each raw table ordinal the encoder
// writes unchecked today, pinned standalone as Ok-then-verifier-rejects. The
// `DurIterateBounded`/`DurIndexScan` `list_ty` operand belongs here too — a live
// site operand carries the instruction while its `list_ty` is a public raw ordinal
// that dangles — and both opcode paths are pinned below the durable fixture.

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today a `RecordNew` naming no TYPES row still encodes.
#[test]
fn an_out_of_range_record_new_ordinal_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(Vec::new(), vec![Instr::RecordNew(u16::MAX), Instr::Return])
        .encode()
        .expect("the producer accepts the unanswered record ordinal today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("collection type")`;
/// the flip must cite this pin: today a `ListNew` naming no COLLTYPES row still
/// encodes.
#[test]
fn an_out_of_range_list_new_ordinal_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(Vec::new(), vec![Instr::ListNew(u16::MAX), Instr::Return])
        .encode()
        .expect("the producer accepts the unanswered collection ordinal today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("enum type")`; the
/// flip must cite this pin: today an `EnumConstruct` naming no ENUMS row still encodes.
#[test]
fn an_out_of_range_enum_construct_ordinal_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(
        Vec::new(),
        vec![
            Instr::EnumConstruct {
                enum_idx: u16::MAX,
                variant: 0,
            },
            Instr::Return,
        ],
    )
    .encode()
    .expect("the producer accepts the unanswered enum ordinal today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today a `VacantLoad` embedding an optional record type
/// that names no TYPES row still encodes, and the verifier's record-ordinal check is
/// the exact refusal. (The operand must be optional — a bare forged type is rejected
/// for its optionality before the ordinal is ever consulted.)
#[test]
fn an_out_of_range_vacant_load_type_encodes_today_and_only_the_verifier_rejects() {
    let body = |idx: u16| {
        vec![
            Instr::VacantLoad(ImageType::Record {
                idx,
                optional: true,
            }),
            Instr::Pop,
            Instr::ConstLoad(0),
            Instr::Return,
        ]
    };
    // The corrected twin — the same fixture with a TYPES row answering index 0 —
    // verifies, so the rejection below is the forged ordinal's alone.
    let corrected = with_decoy_record(main_draft(Vec::new(), body(0)))
        .encode()
        .expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    let image = with_decoy_record(main_draft(Vec::new(), body(u16::MAX)))
        .encode()
        .expect("the producer accepts the unanswered vacant-load type today");
    let rejection = verify(&image.bytes).expect_err("the vacant-load ordinal is refused");
    assert_eq!(rejection.detail(), "vacant-load record index out of range");
}

/// The coherence hoist will convert this Ok to `InvalidReference("root table")`; the
/// flip must cite this pin: today a `MakeIdentity` naming no ROOTS row still encodes.
#[test]
fn an_out_of_range_make_identity_root_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(
        Vec::new(),
        vec![
            Instr::MakeIdentity {
                root: u16::MAX,
                cols: 0,
            },
            Instr::Return,
        ],
    )
    .encode()
    .expect("the producer accepts the unanswered root ordinal today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today a TYPES field whose `ImageType` names no TYPES row
/// still encodes.
#[test]
fn an_out_of_range_field_type_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
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
    let image = draft
        .encode()
        .expect("the producer accepts the unanswered field type today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today an ENUMS variant payload leaf naming no TYPES row
/// still encodes.
#[test]
fn an_out_of_range_enum_payload_type_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
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
    let image = draft
        .encode()
        .expect("the producer accepts the unanswered payload type today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today a COLLTYPES element naming no TYPES row still
/// encodes.
#[test]
fn an_out_of_range_collection_elem_type_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    draft.add_collection_type(CollectionTypeDef::List { elem: FORGED_TYPE });
    let image = draft
        .encode()
        .expect("the producer accepts the unanswered element type today");
    assert!(verify(&image.bytes).is_err());
}

// ---- The two omitted DURABLE type-table ordinals and the `MakeIdentity` cols
// relation (design draft 7 §B.3). `TypeId` is a raw newtype with a public
// `from_index`, so both record ordinals are forged directly.

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
    let (draft, _root) = durable_parts(entry, branch, false);
    finish_main(draft, code, ImageType::scalar(Scalar::Int))
}

/// The durable graph of [`durable_draft`], before `main` is added, returning the
/// admitted root so a caller can bind operation sites. `indexed` gives the root one
/// nonunique managed index projecting the field then the identity key.
fn durable_parts(
    entry: TableRef,
    branch: Option<TableRef>,
    indexed: bool,
) -> (ImageDraft, AdmittedRoot) {
    let mut draft = ImageDraft::new();
    let value = draft.value_shapes_mut().scalar(Scalar::Int);
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
    (draft, root)
}

/// Add and export `main` over an already-built durable graph.
fn finish_main(mut draft: ImageDraft, code: Vec<Instr>, ret: ImageType) -> ImageDraft {
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
    draft
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

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today a root entry record naming no TYPES row is written
/// to the DURABLE body unchecked, and only the verifier refuses the bytes.
#[test]
fn an_out_of_range_root_entry_record_encodes_today_and_only_the_verifier_rejects() {
    let image = durable_draft(TableRef::Forged, None, short_code())
        .encode()
        .expect("the producer accepts the unanswered entry record today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today a branch entry record naming no TYPES row is written
/// to the DURABLE body unchecked, and only the verifier refuses the bytes.
#[test]
fn an_out_of_range_branch_record_encodes_today_and_only_the_verifier_rejects() {
    let image = durable_draft(TableRef::Valid, Some(TableRef::Forged), short_code())
        .encode()
        .expect("the producer accepts the unanswered branch record today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("root table")`; the
/// flip must cite this pin: today a `MakeIdentity` naming a valid root but a `cols`
/// count unequal to that root's key arity still encodes, and only the verifier
/// refuses the bytes.
#[test]
fn a_make_identity_cols_arity_mismatch_encodes_today_and_only_the_verifier_rejects() {
    let image = durable_draft(
        TableRef::Valid,
        None,
        vec![
            Instr::ConstLoad(0),
            Instr::ConstLoad(0),
            Instr::MakeIdentity { root: 0, cols: 2 },
            Instr::Return,
        ],
    )
    .encode()
    .expect("the producer accepts the arity mismatch today");
    assert!(verify(&image.bytes).is_err());
}

// ---- The `list_ty` family, both opcode paths: a live provenance-validated site
// operand carries the instruction, while its `list_ty` — a public raw COLLTYPES
// ordinal — dangles past the (empty) collection table.

/// The coherence hoist will convert this Ok to `InvalidReference("collection type")`;
/// the flip must cite this pin: today a bounded traversal whose `list_ty` names no
/// COLLTYPES row still encodes, and only the verifier refuses the bytes.
#[test]
fn a_dangling_iterate_list_type_encodes_today_and_only_the_verifier_rejects() {
    let (mut draft, root) = durable_parts(TableRef::Valid, None, false);
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
            list_ty: u16::MAX,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    let image = finish_main(draft, code, ImageType::Unit)
        .encode()
        .expect("the producer accepts the dangling list type today");
    let rejection = verify(&image.bytes).expect_err("the dangling list type is refused");
    assert_eq!(
        rejection.detail(),
        "bounded traversal list type does not name a list of the traversed key",
    );
}

/// The coherence hoist will convert this Ok to `InvalidReference("collection type")`;
/// the flip must cite this pin: today an index scan whose `list_ty` names no COLLTYPES
/// row still encodes, and only the verifier refuses the bytes.
#[test]
fn a_dangling_index_scan_list_type_encodes_today_and_only_the_verifier_rejects() {
    let scan_image = |list_ty: u16| {
        let (mut draft, root) = durable_parts(TableRef::Valid, None, true);
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
            Instr::ConstLoad(0),
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
        finish_main(draft, code, ImageType::Unit)
            .encode()
            .expect("the producer accepts either list type today")
    };
    // The corrected twin — the same fixture naming the real `List[int]` row —
    // verifies, so the rejection below is the dangling ordinal's alone.
    let outcome = verify(&scan_image(0).bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    let rejection =
        verify(&scan_image(u16::MAX).bytes).expect_err("the dangling list type is refused");
    assert_eq!(
        rejection.detail(),
        "index scan list type does not name a list of the identity key",
    );
}

// ---- Domain-decoy `ImageType` pins (review 7 item 5): index 0 into an EMPTY target
// domain while a WRONG domain is populated at index 0, asserting the exact
// target-domain rejection — a check consulting the wrong table would accept these.

/// A fieldless record populating TYPES row 0, as decoy for the non-record domains.
fn with_decoy_record(mut draft: ImageDraft) -> ImageDraft {
    let name = draft.intern_string("Decoy");
    draft.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    draft
}

/// A payloadless enum populating ENUMS row 0, as decoy for the record domain.
fn with_decoy_enum(mut draft: ImageDraft) -> ImageDraft {
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
    draft
}

/// The Record domain resolves against TYPES: with TYPES empty and ENUMS populated at
/// index 0, a wrong-table check would accept this draft; the exact TYPES-domain
/// rejection pins the correct domain.
#[test]
fn a_record_type_decoy_draws_the_types_domain_rejection() {
    let draft = with_decoy_enum(main_draft(
        vec![ImageType::Record {
            idx: 0,
            optional: false,
        }],
        short_code(),
    ));
    let image = draft.encode().expect("the decoy encodes");
    let rejection = verify(&image.bytes).expect_err("the empty TYPES domain refuses index 0");
    assert_eq!(rejection.detail(), "record param type index out of range");
}

/// The Enum domain resolves against ENUMS: with ENUMS empty and TYPES populated at
/// index 0, a wrong-table check would accept this draft.
#[test]
fn an_enum_type_decoy_draws_the_enums_domain_rejection() {
    let draft = with_decoy_record(main_draft(
        vec![ImageType::Enum {
            idx: 0,
            optional: false,
        }],
        short_code(),
    ));
    let image = draft.encode().expect("the decoy encodes");
    let rejection = verify(&image.bytes).expect_err("the empty ENUMS domain refuses index 0");
    assert_eq!(rejection.detail(), "enum param type index out of range");
}

/// The Collection domain resolves against COLLTYPES: with COLLTYPES empty and TYPES
/// populated at index 0, a wrong-table check would accept this draft.
#[test]
fn a_collection_type_decoy_draws_the_colltypes_domain_rejection() {
    let draft = with_decoy_record(main_draft(
        vec![ImageType::Collection {
            idx: 0,
            optional: false,
        }],
        short_code(),
    ));
    let image = draft.encode().expect("the decoy encodes");
    let rejection = verify(&image.bytes).expect_err("the empty COLLTYPES domain refuses index 0");
    assert_eq!(
        rejection.detail(),
        "collection param type index out of range"
    );
}

/// The Identity domain resolves against ROOTS: with ROOTS empty (a storeless image)
/// and TYPES populated at index 0, a wrong-table check would accept this draft.
#[test]
fn an_identity_type_decoy_draws_the_roots_domain_rejection() {
    let draft = with_decoy_record(main_draft(
        vec![ImageType::Identity {
            root: 0,
            optional: false,
        }],
        short_code(),
    ));
    let image = draft.encode().expect("the decoy encodes");
    let rejection = verify(&image.bytes).expect_err("the empty ROOTS domain refuses index 0");
    assert_eq!(
        rejection.detail(),
        "identity param type root index out of range",
    );
}

/// The subordinate `EnumConstruct.variant` ordinal, designated into the pinned set: it
/// is checked against the resolved enum, not a table of its own, so its boundary
/// coverage derives from the adjacent `enum_idx` cells (same instruction, same tape
/// position — the `enum_idx` crossings in `legacy_bridge.rs` carry both operands).
#[test]
fn an_out_of_range_enum_construct_variant_draws_the_exact_variant_rejection() {
    let body = |variant: u16| {
        vec![
            Instr::EnumConstruct {
                enum_idx: 0,
                variant,
            },
            Instr::Pop,
            Instr::ConstLoad(0),
            Instr::Return,
        ]
    };
    // The corrected twin — variant 0, the decoy enum's one payloadless member —
    // verifies, so the rejection below is the unanswered variant's alone.
    let corrected = with_decoy_enum(main_draft(Vec::new(), body(0)))
        .encode()
        .expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    // The decoy enum is the one ENUMS row; variant 5 names no member of it.
    let image = with_decoy_enum(main_draft(Vec::new(), body(5)))
        .encode()
        .expect("the producer accepts the variant today");
    let rejection = verify(&image.bytes).expect_err("the unanswered variant is refused");
    assert_eq!(rejection.detail(), "enum variant index out of range");
}

// ---- The remaining collection-ordinal opcode (design draft 8 §B.3): `MapNew` shares
// `ListNew`'s operand kind and tape position; `TextSplit`/`TextLines` derive from
// these two by the cutpoint law in `legacy_bridge.rs`.

/// The coherence hoist will convert this Ok to `InvalidReference("collection type")`;
/// the flip must cite this pin: today a `MapNew` naming no COLLTYPES row still encodes.
#[test]
fn an_out_of_range_map_new_ordinal_encodes_today_and_only_the_verifier_rejects() {
    let body = |idx: u16| {
        vec![
            Instr::MapNew(idx),
            Instr::Pop,
            Instr::ConstLoad(0),
            Instr::Return,
        ]
    };
    let with_map_row = |code: Vec<Instr>| {
        let mut draft = main_draft(Vec::new(), code);
        draft.add_collection_type(CollectionTypeDef::Map {
            key: ImageType::scalar(Scalar::Int),
            value: ImageType::scalar(Scalar::Int),
        });
        draft.encode().expect("either collection ordinal encodes")
    };
    // The corrected twin — the same fixture naming the real Map row — verifies, so
    // the rejection below is the dangling ordinal's alone.
    let outcome = verify(&with_map_row(body(0)).bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    verify(&with_map_row(body(u16::MAX)).bytes)
        .expect_err("the unanswered collection ordinal is refused");
}

// ---- The non-range export/test relations (design drafts 8 §B.3 and review 9): rows
// the public draft APIs accept unchecked today, refused only by the verifier — the
// target relations, the id relation, both test-signature decision sites, and calls
// into test entries (the draft-8 call-closure exclusion was false; the verifier scans
// direct tape call targets in its seal phase). Their policy crossings sit in
// `legacy_bridge.rs` (a literal Strings × duplicate-export cell; the rest derive by
// the cutpoint law — EXPORTS and TEST-ENTRY rows assemble after every cutpoint).

/// The coherence hoist will convert this Ok to `InvalidReference("export table")`; the
/// flip must cite this pin: two exports naming one function violate the
/// one-export-per-function relation.
#[test]
fn a_duplicate_export_target_encodes_today_and_only_the_verifier_rejects() {
    let (mut draft, main) = main_draft_with_id(Vec::new(), short_code());
    draft.add_export(ExportId::of_local("", "again"), main);
    let image = draft
        .encode()
        .expect("the producer accepts the duplicate export target today");
    let rejection = verify(&image.bytes).expect_err("the duplicate export is refused");
    assert_eq!(rejection.detail(), "duplicate export function index");
}

/// The coherence hoist will convert this Ok to `InvalidReference("test table")`; the
/// flip must cite this pin: two test entries naming one function violate the
/// unique-test-function relation.
#[test]
fn a_duplicate_test_target_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    let test_fn = add_plain_function(&mut draft, "t", ImageType::Unit, vec![Instr::Return]);
    let first = draft.intern_string("ta");
    let second = draft.intern_string("tb");
    draft.add_test_entry(first, test_fn);
    draft.add_test_entry(second, test_fn);
    let image = draft
        .encode()
        .expect("the producer accepts the duplicate test target today");
    let rejection = verify(&image.bytes).expect_err("the duplicate test target is refused");
    assert_eq!(rejection.detail(), "duplicate test-entry function index");
}

/// The coherence hoist will convert this Ok to `InvalidReference("test table")`; the
/// flip must cite this pin: two test entries wearing one name violate the unique-name
/// relation the sorted table encodes.
#[test]
fn a_duplicate_test_name_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    let first = add_plain_function(&mut draft, "t1", ImageType::Unit, vec![Instr::Return]);
    let second = add_plain_function(&mut draft, "t2", ImageType::Unit, vec![Instr::Return]);
    let name = draft.intern_string("t");
    draft.add_test_entry(name, first);
    draft.add_test_entry(name, second);
    let image = draft
        .encode()
        .expect("the producer accepts the duplicate test name today");
    let rejection = verify(&image.bytes).expect_err("the duplicate test name is refused");
    assert_eq!(
        rejection.detail(),
        "test entries must be sorted and unique by name",
    );
}

/// The coherence hoist will convert this Ok to `InvalidReference("test table")`; the
/// flip must cite this pin: a test entry naming an exported function violates the
/// export/test disjointness relation.
#[test]
fn an_export_test_overlap_encodes_today_and_only_the_verifier_rejects() {
    // The overlapping function is unit-returning and structurally test-valid, so the
    // overlap is the ONE defect: removing the export alone makes the image verify.
    let build = |exported: bool| {
        let mut draft = main_draft(Vec::new(), short_code());
        let test_fn = add_plain_function(&mut draft, "t", ImageType::Unit, vec![Instr::Return]);
        if exported {
            draft.add_export(ExportId::of_local("", "t"), test_fn);
        }
        let name = draft.intern_string("tn");
        draft.add_test_entry(name, test_fn);
        draft.encode().expect("the overlap fixture encodes")
    };
    let outcome = verify(&build(false).bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    let rejection = verify(&build(true).bytes).expect_err("the export/test overlap is refused");
    assert_eq!(rejection.detail(), "a test entry is also an export");
}

/// The coherence hoist will convert this Ok to `InvalidReference("export table")`; the
/// flip must cite this pin: two export rows wearing one `ExportId` violate the
/// unique-id relation the sorted table encodes.
#[test]
fn a_duplicate_export_id_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    // A second structurally valid function, exported under `main`'s exact id.
    let second = add_plain_function(
        &mut draft,
        "g",
        ImageType::scalar(Scalar::Int),
        vec![Instr::ConstLoad(0), Instr::Return],
    );
    draft.add_export(ExportId::of_local("", "main"), second);
    let image = draft
        .encode()
        .expect("the producer accepts the duplicate export id today");
    let rejection = verify(&image.bytes).expect_err("the duplicate export id is refused");
    assert_eq!(
        rejection.detail(),
        "exports must be sorted and unique by id"
    );
}

/// The coherence hoist will convert this Ok to `InvalidReference("test table")`; the
/// flip must cite this pin: a test entry over a parameter-taking function violates the
/// FIRST decision site of the test signature law (nonzero params — decided before the
/// return-shape site the non-unit pin above covers).
#[test]
fn a_test_entry_with_params_encodes_today_and_only_the_verifier_rejects() {
    let build = |params: Vec<ImageType>| {
        let mut draft = main_draft(Vec::new(), short_code());
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
        draft.encode().expect("the signature fixture encodes")
    };
    // The corrected twin — the same entry over a zero-parameter unit function —
    // verifies, so the rejection below is the parameter's alone.
    let outcome = verify(&build(Vec::new()).bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    let rejection = verify(&build(vec![ImageType::scalar(Scalar::Int)]).bytes)
        .expect_err("the parameter-taking test entry is refused");
    assert_eq!(rejection.detail(), "a test entry takes no parameters");
}

/// The coherence hoist will convert this Ok to `InvalidReference("test table")`; the
/// flip must cite this pin: an `Assert` in a non-test exported function violates the
/// membership law — the encoder writes the opcode unchecked, and the verifier's
/// TEST-ENTRY seal scan is the only owner that refuses it.
#[test]
fn an_assert_outside_a_test_entry_encodes_today_and_only_the_verifier_rejects() {
    let assert_body = |draft: &mut ImageDraft| {
        let truth = draft.intern_bool(true);
        vec![
            Instr::ConstLoad(truth.index()),
            Instr::Assert,
            Instr::Return,
        ]
    };
    // The corrected twin — the SAME asserting body inside a registered test entry —
    // verifies, so the rejection below is the membership relation's alone.
    let mut corrected = main_draft(Vec::new(), short_code());
    let code = assert_body(&mut corrected);
    let test_fn = add_plain_function(&mut corrected, "t", ImageType::Unit, code);
    let name = corrected.intern_string("tn");
    corrected.add_test_entry(name, test_fn);
    let corrected = corrected.encode().expect("the corrected twin encodes");
    let outcome = verify(&corrected.bytes);
    assert!(outcome.is_ok(), "{outcome:?}");

    let mut draft = main_draft(Vec::new(), short_code());
    let code = assert_body(&mut draft);
    let asserting = add_plain_function(&mut draft, "t", ImageType::Unit, code);
    draft.add_export(ExportId::of_local("", "t"), asserting);
    let image = draft
        .encode()
        .expect("the producer accepts the assert today");
    let rejection = verify(&image.bytes).expect_err("the stray assert is refused");
    assert_eq!(
        rejection.detail(),
        "an assert instruction sits outside a test entry",
    );
}

/// The coherence hoist will convert this Ok to `InvalidReference("test table")`; the
/// flip must cite this pin: a test body that performs a direct durable operation AND
/// drives a transaction-owning export violates the driver-mix law — both halves are
/// ordinary unchecked tape writes the encoder accepts.
#[test]
fn a_test_driver_mix_encodes_today_and_only_the_verifier_rejects() {
    let build = |drives_owner: bool| {
        let (mut draft, root) = durable_parts(TableRef::Valid, None, false);
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
                Instr::ConstLoad(0),
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
        let mut code = vec![Instr::ConstLoad(0), Instr::DurExists(site), Instr::Pop];
        // ...beside the drive of the transaction owner.
        if drives_owner {
            code.push(Instr::Call(0));
        }
        code.push(Instr::Return);
        let test_fn = add_plain_function(&mut draft, "t", ImageType::Unit, code);
        let name = draft.intern_string("tn");
        draft.add_test_entry(name, test_fn);
        finish_main(draft, short_code(), ImageType::scalar(Scalar::Int))
            .encode()
            .expect("the driver-mix fixture encodes")
    };
    // The corrected twin — the same direct-durable test without the owner call —
    // verifies, so the rejection below is the mix's alone.
    let outcome = verify(&build(false).bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    let rejection = verify(&build(true).bytes).expect_err("the driver mix is refused");
    assert_eq!(
        rejection.detail(),
        "a test body performs a direct durable operation and also drives a \
         transaction-owning export",
    );
}

/// The coherence hoist will convert this Ok to `InvalidReference("test table")`; the
/// flip must cite this pin: a `Call` whose direct tape target is a test entry violates
/// the entry-point relation (the verifier scans direct call targets in its seal phase
/// — no call closure is needed, correcting draft 8's exclusion).
#[test]
fn a_call_into_a_test_entry_encodes_today_and_only_the_verifier_rejects() {
    // `main` is ordinal 0, so the companion the call names is ordinal 1.
    let build = |tested: bool| {
        let mut draft = main_draft(
            Vec::new(),
            vec![Instr::Call(1), Instr::ConstLoad(0), Instr::Return],
        );
        let callee = add_plain_function(&mut draft, "t", ImageType::Unit, vec![Instr::Return]);
        assert_eq!(callee.index(), 1, "the call names the companion");
        if tested {
            let name = draft.intern_string("tn");
            draft.add_test_entry(name, callee);
        }
        draft.encode().expect("the call fixture encodes")
    };
    // The corrected twin — the same call with the callee not registered as a test —
    // verifies, so the rejection below is the entry-point relation's alone.
    let outcome = verify(&build(false).bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
    let rejection = verify(&build(true).bytes).expect_err("the call into a test entry is refused");
    assert_eq!(rejection.detail(), "a test entry may not be called");
}

/// The coherence hoist will convert this Ok to `InvalidReference("test table")`; the
/// flip must cite this pin: a test entry over an int-returning function violates the
/// test signature law (zero params, unit return).
#[test]
fn a_bad_test_signature_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    // A structurally valid int function, wrong only as a TEST target.
    let test_fn = add_plain_function(
        &mut draft,
        "t",
        ImageType::scalar(Scalar::Int),
        vec![Instr::ConstLoad(0), Instr::Return],
    );
    let name = draft.intern_string("tn");
    draft.add_test_entry(name, test_fn);
    let image = draft
        .encode()
        .expect("the producer accepts the signature today");
    let rejection = verify(&image.bytes).expect_err("the test signature is refused");
    assert_eq!(rejection.detail(), "a test entry must return unit");
}
