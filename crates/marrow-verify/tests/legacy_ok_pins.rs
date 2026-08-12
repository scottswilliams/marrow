//! Structural Ok-pins: drafts the producer ENCODES today although the reference they
//! carry answers to no table row, leaving the independent verifier as the only owner
//! that refuses them.
//!
//! Each case pins both halves of that split — `encode()` returns `Ok` and
//! `verify(&bytes)` returns `Err` — so the coherence hoist has an executable baseline
//! for the producer-side conversion. Each clean twin is asserted to verify, proving the
//! rejection comes from the one defect and not from the fixture's shape.

use marrow_image::{
    CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape, EncodedImage, EnumTypeDef,
    ExportId, FieldDef, FuncId, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn,
    LedgerIdBytes, RecordTypeDef, RootOccurrenceDef, Scalar, SpanEntry, TypeId, VariantDef,
};
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
    draft
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
// `DurIterateBounded`/`DurIndexScan` `list_ty` family is not separately
// constructible as a defect: those instructions demand a live site operand whose
// typed `encodable()` state cannot be forged, and their collection ordinal is the
// same raw-write kind `ListNew` pins here.

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
/// flip must cite this pin: today a `VacantLoad` embedding a type that names no TYPES
/// row still encodes.
#[test]
fn an_out_of_range_vacant_load_type_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(
        Vec::new(),
        vec![Instr::VacantLoad(FORGED_TYPE), Instr::Return],
    )
    .encode()
    .expect("the producer accepts the unanswered vacant-load type today");
    assert!(verify(&image.bytes).is_err());
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
/// the durable clean shape the three pins below vary one reference from. `branch`
/// adds an otherwise-valid keyed branch member with the given entry record.
fn durable_draft(entry: TableRef, branch: Option<TableRef>, code: Vec<Instr>) -> ImageDraft {
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
    draft
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
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    draft.intern_int(0);
    let main = draft
        .add_function(FunctionDef {
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
