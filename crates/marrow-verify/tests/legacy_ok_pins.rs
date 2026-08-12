//! Structural Ok-pins: drafts the producer ENCODES today although the reference they
//! carry answers to no table row, leaving the independent verifier as the only owner
//! that refuses them.
//!
//! Each case pins both halves of that split — `encode()` returns `Ok` and
//! `verify(&bytes)` returns `Err` — so the coherence hoist has an executable baseline
//! for the producer-side conversion. Each clean twin is asserted to verify, proving the
//! rejection comes from the one defect and not from the fixture's shape.

use marrow_image::{
    CollectionTypeDef, EncodedImage, EnumTypeDef, ExportId, FieldDef, FuncId, FunctionDef,
    ImageDraft, ImageType, Instr, RecordTypeDef, Scalar, SpanEntry, VariantDef,
};
use marrow_verify::verify;

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
