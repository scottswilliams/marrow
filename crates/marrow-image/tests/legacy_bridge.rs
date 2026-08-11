//! The legacy encoder bridge: which producer-side result a draft
//! with several faults reports, and what the encoder allocates on the way to reporting
//! it.
//!
//! A durable field's value is spelled on the wire as its full expansion, so a draft
//! inside every declared bound can still describe a DURABLE body larger than any image
//! may be. That body is now refused by counting it rather than by building it — which
//! moves *when* the encoder learns the answer, and must not move *which* answer a draft
//! with an earlier or later fault receives.
//!
//! Each case below combines the CodeBytes refusal with one earlier policy fault and one
//! later ceiling fault and pins the exact result and the exact offending row. The
//! ordering they pin is the one the old encoder had: every fixed bound first, in
//! `check_bounds` order; then the durable graph's own coherence; then CodeBytes; then
//! the whole-image ceiling.
//!
//! That no contract hash is ever computed over bytes no image can carry is structural
//! rather than measured: the producer mints an identity at exactly one site, from a value
//! that exists only for a body the fence already admitted, and the identity owner refuses
//! a canonical payload past its own ceiling whoever asks (see the absence gate
//! `the_contract_identity_has_one_mint_per_side_and_a_bound_of_its_own`).

use marrow_image::bounds::{MAX_CODE_BYTES, MAX_KEY_COLUMNS, MAX_STRUCT_LEAVES};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, ExportId, FunctionDef, ImageBuildError,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootOccurrenceDef,
    Scalar, SpanEntry, ValueShapeNodeId,
};

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const KEY_ID: [u8; 16] = [0x0c; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
const FIELD_ID: [u8; 16] = [0x0e; 16];

/// How a fixture's durable field value is shaped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Value {
    /// One `int` leaf: a two-byte value on the wire.
    Scalar,
    /// Ten nesting levels of four leaves each: 11 value-graph nodes, and an expansion
    /// of `4^10` leaves — about 2 MiB of wire, four times what any image may be.
    OverCeiling,
    /// A dense struct one leaf past `MAX_STRUCT_LEAVES`: refused by `check_bounds`,
    /// which runs before every other result here.
    OverWideStruct,
}

impl Value {
    fn shape(self, draft: &mut ImageDraft) -> ValueShapeNodeId {
        let values = draft.value_shapes_mut();
        let int = values.scalar(Scalar::Int);
        match self {
            Value::Scalar => int,
            Value::OverCeiling => {
                let mut level = int;
                for _ in 0..10 {
                    level = values.struct_shape(vec![level; 4]);
                }
                level
            }
            Value::OverWideStruct => values.struct_shape(vec![int; MAX_STRUCT_LEAVES + 1]),
        }
    }
}

/// How a fixture's `main` is shaped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Code {
    /// Two instructions.
    Short,
    /// Past `MAX_CODE_BYTES`, so the function section refuses it.
    OverCodeBytes,
}

/// One fixture: a keyed root over a Product with one durable field, plus a `main`.
///
/// The three knobs are independent, so a case states exactly the faults it wants and
/// the encode result names which one the producer reports.
struct Fixture {
    value: Value,
    code: Code,
    keys: usize,
    application: bool,
}

impl Fixture {
    fn clean() -> Self {
        Self {
            value: Value::Scalar,
            code: Code::Short,
            keys: 1,
            application: true,
        }
    }

    fn value(mut self, value: Value) -> Self {
        self.value = value;
        self
    }

    fn code(mut self, code: Code) -> Self {
        self.code = code;
        self
    }

    /// A key tuple one column past `MAX_KEY_COLUMNS`: an occurrence-level fault
    /// `check_bounds` reports after the declaration graph and before CodeBytes.
    fn over_wide_key(mut self) -> Self {
        self.keys = MAX_KEY_COLUMNS + 1;
        self
    }

    /// Drop the application identity a non-empty durable graph is anchored by.
    fn without_application(mut self) -> Self {
        self.application = false;
        self
    }

    fn encode(self) -> Result<(), ImageBuildError> {
        let mut draft = ImageDraft::new();
        let value = self.value.shape(&mut draft);
        let type_name = draft.intern_string("R");
        let record = draft.add_record_type(RecordTypeDef {
            name: type_name,
            fields: Vec::new(),
        });
        if self.application {
            draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        }
        draft
            .declare_product(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                record,
                vec![DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(FIELD_ID),
                        required: true,
                        value,
                    },
                }],
            )
            .expect("a well-formed declaration");
        let root_name = draft.intern_string("r");
        draft
            .add_root_occurrence(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                RootOccurrenceDef {
                    name: root_name,
                    keys: (0..self.keys)
                        .map(|column| KeyColumn {
                            scalar: Scalar::Int,
                            id: key_id(column),
                        })
                        .collect(),
                    placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                    indexes: Vec::new(),
                },
            )
            .expect("the Product is declared");
        let src = draft.intern_string("src/main.mw");
        let main_name = draft.intern_string("main");
        let zero = draft.intern_int(0);
        let code = match self.code {
            Code::Short => vec![Instr::ConstLoad(zero.index()), Instr::Return],
            // `ConstLoad` is three bytes, so this is comfortably past the limit while
            // staying well inside the instruction count the draft admits.
            Code::OverCodeBytes => {
                let mut code = vec![Instr::ConstLoad(zero.index()); MAX_CODE_BYTES / 2];
                code.push(Instr::Return);
                code
            }
        };
        let main = draft
            .add_function(FunctionDef {
                name: main_name,
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
        draft.encode().map(|_| ())
    }
}

fn key_id(column: usize) -> LedgerIdBytes {
    // One seed byte carries the distinctness this helper promises.
    assert!(column <= u8::MAX as usize, "key seed exceeds its one byte");
    let mut bytes = [0x30u8; 16];
    bytes[0] = column as u8;
    match column {
        0 => LedgerIdBytes::from_bytes(KEY_ID),
        _ => LedgerIdBytes::from_bytes(bytes),
    }
}

/// The bridge changes nothing about a clean draft.
#[test]
fn a_clean_draft_encodes() {
    assert_eq!(Fixture::clean().encode(), Ok(()));
}

/// A DURABLE body larger than any image draws the whole-image ceiling result it has
/// always drawn — now decided in the bytes the ceiling admits rather than in the bytes
/// the expansion would have produced.
#[test]
fn a_body_past_the_ceiling_draws_the_image_ceiling_result() {
    assert_eq!(
        Fixture::clean().value(Value::OverCeiling).encode(),
        Err(ImageBuildError::ImageTooLarge),
    );
}

/// CodeBytes keeps its position ahead of the body's ceiling result. The old encoder
/// built the body first and refused it last, so a draft with both faults reported
/// CodeBytes; counting the body earlier must not overtake that.
#[test]
fn code_bytes_outranks_the_body_ceiling() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverCeiling)
            .code(Code::OverCodeBytes)
            .encode(),
        Err(ImageBuildError::CodeTooLong),
    );
    assert_eq!(
        Fixture::clean().code(Code::OverCodeBytes).encode(),
        Err(ImageBuildError::CodeTooLong),
        "CodeBytes alone reports the same result",
    );
}

/// A fixed declaration-graph bound outranks CodeBytes and the body ceiling alike: it is
/// `check_bounds`, which the preflight replays first and in its own order.
#[test]
fn a_declaration_bound_outranks_code_bytes_and_the_body_ceiling() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverWideStruct)
            .code(Code::OverCodeBytes)
            .encode(),
        Err(ImageBuildError::TooManyStructLeaves),
    );
}

/// An occurrence-level bound is reported from the same first pass, likewise ahead of
/// CodeBytes and the body ceiling.
#[test]
fn an_occurrence_bound_outranks_code_bytes_and_the_body_ceiling() {
    assert_eq!(
        Fixture::clean()
            .over_wide_key()
            .value(Value::OverCeiling)
            .code(Code::OverCodeBytes)
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
    );
}

/// The durable graph's own coherence — the application identity a non-empty graph is
/// anchored by — is decided after every fixed bound and before CodeBytes, exactly where
/// the old encoder decided it at the head of the DURABLE section.
#[test]
fn the_graph_anchor_outranks_code_bytes_and_the_body_ceiling() {
    assert_eq!(
        Fixture::clean()
            .without_application()
            .value(Value::OverCeiling)
            .code(Code::OverCodeBytes)
            .encode(),
        Err(ImageBuildError::InvalidReference("application identity")),
    );
    assert_eq!(
        Fixture::clean()
            .without_application()
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
        "a fixed bound still precedes the anchor",
    );
}

/// The construction budget this file's fixtures are admitted under.
///
/// The compiler-free tier states a census the way the compiler's admission owner does: a
/// plan minted before construction, whose terms `admit` checks against what a ProgramImage
/// can hold. These fixtures build small graphs, so the census is the image's own ceilings
/// rather than a second, narrower policy stated here — what the plan closes is unadmitted
/// intake, not fixture size.
fn admitted_plan() -> marrow_image::AdmittedGraphInputPlan {
    marrow_image::AdmittedGraphInputPlan::admit(
        marrow_image::bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS,
        marrow_image::bounds::MAX_ADMITTED_ROOT_OCCURRENCES,
        marrow_image::bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
    )
    .expect("the image's own ceilings are admitted counts")
}
