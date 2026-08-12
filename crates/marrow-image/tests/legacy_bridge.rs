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
//!
//! # The mixed-corruption matrix
//!
//! The tests below the six bridge cases pin the CURRENT verdict for one draft carrying a
//! resource-policy-class defect (an aggregate table cap, a string-length cap, CodeBytes,
//! or the whole-image byte ceiling) and an invariant-class defect (an over-wide key tuple
//! or struct, an over-deep durable value, a broken frame, a missing application anchor)
//! at the same time. Today that verdict is purely positional — `check_bounds` order, then
//! the application anchor, then CodeBytes, then the ceiling — so a cap declared before
//! the durable-graph walk outranks every invariant while a cap declared after it does
//! not. Each pin is the differential baseline for the sanctioned invariant-over-resource
//! restructure; a flipped verdict must cite the pin it flips.
//!
//! Every resource-policy candidate the encoder reports today is reachable through this
//! fixture, so no pair in the matrix is skipped as unconstructible: the caps sitting on
//! aggregate tables (strings, consts, types, enums, collections, roots, sites, functions,
//! exports, test entries) are driven by appending rows, and the byte-shaped caps
//! (StringBytes, CodeBytes, the whole-image ceiling) by one oversized element each.

use marrow_image::bounds::{
    MAX_CODE_BYTES, MAX_COLLECTIONS, MAX_CONSTS, MAX_DURABLE_VALUE_DEPTH, MAX_ENUMS, MAX_EXPORTS,
    MAX_FUNCTIONS, MAX_KEY_COLUMNS, MAX_LOCALS, MAX_RECORD_FIELDS, MAX_ROOTS, MAX_SITES,
    MAX_STRING_BYTES, MAX_STRINGS, MAX_STRUCT_LEAVES, MAX_TEST_ENTRIES, MAX_TYPES,
};
use marrow_image::{
    CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape, EnumTypeDef, ExportId,
    FieldDef, FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes,
    RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry, ValueShapeNodeId,
};

#[path = "common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

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
    /// A single-leaf struct nest one level past `MAX_DURABLE_VALUE_DEPTH`: a two-byte
    /// expansion whose depth the declaration-graph walk refuses.
    OverDeep,
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
            Value::OverDeep => {
                let mut level = int;
                for _ in 0..MAX_DURABLE_VALUE_DEPTH {
                    level = values.struct_shape(vec![level]);
                }
                level
            }
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

/// How the fixture's `main` frame is shaped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// No params, no locals.
    Fits,
    /// One local slot past `MAX_LOCALS`: refused near the end of `check_bounds`.
    OverLocals,
    /// One `int` param but zero local slots, so the frame cannot hold its own params:
    /// refused as the last `check_bounds` result.
    LocalsBelowParams,
}

/// One resource-policy-class overflow, applied to an otherwise complete draft. Each
/// variant drives exactly one aggregate cap over its bound without touching any other.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overflow {
    /// One distinct interned string past `MAX_STRINGS`.
    Strings,
    /// One interned string one byte past `MAX_STRING_BYTES`.
    StringBytes,
    /// One distinct constant past `MAX_CONSTS`.
    Consts,
    /// One record type past `MAX_TYPES`.
    Types,
    /// One enum type past `MAX_ENUMS`.
    Enums,
    /// One collection instantiation past `MAX_COLLECTIONS`.
    Collections,
    /// One root occurrence past `MAX_ROOTS` — exactly the one nonblocking overshoot
    /// the admitted plan permits.
    Roots,
    /// One demanded operation site past `MAX_SITES`, through a second wide Product
    /// whose every field leaf is demanded plus its whole-payload site.
    Sites,
    /// One function past `MAX_FUNCTIONS`.
    Functions,
    /// One export past `MAX_EXPORTS`.
    Exports,
    /// One test entry past `MAX_TEST_ENTRIES`.
    TestEntries,
}

/// One fixture: a keyed root over a Product with one durable field, plus a `main`.
///
/// The knobs are independent, so a case states exactly the faults it wants and
/// the encode result names which one the producer reports.
struct Fixture {
    value: Value,
    code: Code,
    keys: usize,
    application: bool,
    frame: Frame,
    policy: Option<Overflow>,
    wide_record: bool,
    conflicting_product: bool,
}

impl Fixture {
    fn clean() -> Self {
        Self {
            value: Value::Scalar,
            code: Code::Short,
            keys: 1,
            application: true,
            frame: Frame::Fits,
            policy: None,
            wide_record: false,
            conflicting_product: false,
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

    fn frame(mut self, frame: Frame) -> Self {
        self.frame = frame;
        self
    }

    fn policy(mut self, policy: Overflow) -> Self {
        self.policy = Some(policy);
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

    /// A record type one field past `MAX_RECORD_FIELDS`: the per-record width fault,
    /// reported early in `check_bounds`.
    fn over_wide_record(mut self) -> Self {
        self.wide_record = true;
        self
    }

    /// Declare the base Product identity a second time with a different member graph:
    /// two declarations wearing one identity, recorded at declaration and refused by
    /// `check_bounds` as the Product claim conflict.
    fn with_conflicting_product(mut self) -> Self {
        self.conflicting_product = true;
        self
    }

    fn encode(self) -> Result<(), ImageBuildError> {
        let mut draft = ImageDraft::new();
        let value = self.value.shape(&mut draft);
        let type_name = draft.intern_string("R");
        let fields = if self.wide_record {
            let field_name = draft.intern_string("wide");
            vec![
                FieldDef {
                    name: field_name,
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                };
                MAX_RECORD_FIELDS + 1
            ]
        } else {
            Vec::new()
        };
        let record = draft.add_record_type(RecordTypeDef {
            name: type_name,
            fields,
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
        if self.conflicting_product {
            // The later declaration still resolves to the bound row; the conflict is
            // recorded and reported by the encoder, not refused here.
            draft
                .declare_product(
                    &admitted_plan(),
                    LedgerIdBytes::from_bytes(PRODUCT_ID),
                    record,
                    vec![DeclarationMemberDef {
                        parent: None,
                        shape: DeclarationMemberShape::Field {
                            id: seeded_id(0x51, 0),
                            required: true,
                            value,
                        },
                    }],
                )
                .expect("a well-formed declaration");
        }
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
                    indexes: Vec::new().into(),
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
        let params = match self.frame {
            Frame::LocalsBelowParams => vec![ImageType::scalar(Scalar::Int)],
            Frame::Fits | Frame::OverLocals => Vec::new(),
        };
        let local_count = match self.frame {
            Frame::OverLocals => (MAX_LOCALS + 1) as u16,
            Frame::Fits | Frame::LocalsBelowParams => 0,
        };
        let main = draft
            .add_function(FunctionDef {
                name: main_name,
                source: src,
                params,
                ret: ImageType::scalar(Scalar::Int),
                local_count,
                spans: vec![SpanEntry {
                    instr_index: 0,
                    line: 1,
                    column: 1,
                }],
                code,
            })
            .expect("every site operand is live");
        draft.add_export(ExportId::of_local("", "main"), main);
        if let Some(policy) = self.policy {
            apply_policy(policy, &mut draft);
        }
        draft.encode().map(|_| ())
    }
}

/// Drive exactly one resource-policy aggregate over its cap on an otherwise complete
/// draft, leaving every other table inside its bound.
fn apply_policy(policy: Overflow, draft: &mut ImageDraft) {
    match policy {
        // The base draft interns a handful of strings, so a full extra pool is over.
        Overflow::Strings => {
            for index in 0..MAX_STRINGS {
                draft.intern_string(&format!("s{index}"));
            }
        }
        Overflow::StringBytes => {
            draft.intern_string(&"x".repeat(MAX_STRING_BYTES + 1));
        }
        // Zero is already interned by the base draft, so the pool ends one past the cap.
        Overflow::Consts => {
            for value in 1..=MAX_CONSTS as i64 {
                draft.intern_int(value);
            }
        }
        Overflow::Types => {
            let name = draft.intern_string("T");
            for _ in 0..MAX_TYPES {
                draft.add_record_type(RecordTypeDef {
                    name,
                    fields: Vec::new(),
                });
            }
        }
        Overflow::Enums => {
            let name = draft.intern_string("E");
            for _ in 0..=MAX_ENUMS {
                draft.add_enum_type(EnumTypeDef {
                    name,
                    variants: Vec::new(),
                });
            }
        }
        Overflow::Collections => {
            for _ in 0..=MAX_COLLECTIONS {
                draft.add_collection_type(CollectionTypeDef::List {
                    elem: ImageType::scalar(Scalar::Int),
                });
            }
        }
        // The admitted plan permits exactly one occurrence past the root bound, so the
        // graph is complete and the encoder — not the intake — reports the cap.
        Overflow::Roots => {
            for index in 0..MAX_ROOTS {
                let name = draft.intern_string(&format!("extra{index}"));
                draft
                    .add_root_occurrence(
                        &admitted_plan(),
                        LedgerIdBytes::from_bytes(PRODUCT_ID),
                        RootOccurrenceDef {
                            name,
                            keys: vec![KeyColumn {
                                scalar: Scalar::Int,
                                id: seeded_id(0x21, index),
                            }],
                            placement: seeded_id(0x22, index),
                            indexes: Vec::new().into(),
                        },
                    )
                    .expect("the Product is declared");
            }
        }
        // A second Product as wide as the site table itself: demanding every field leaf
        // fills the table, and the root's whole-payload demand is the crossing.
        Overflow::Sites => {
            let value = draft.value_shapes_mut().scalar(Scalar::Int);
            let entry_name = draft.intern_string("S");
            let entry = draft.add_record_type(RecordTypeDef {
                name: entry_name,
                fields: Vec::new(),
            });
            let members = (0..MAX_SITES)
                .map(|index| DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: seeded_id(0x33, index),
                        required: true,
                        value,
                    },
                })
                .collect();
            let fields = draft
                .declare_product(&admitted_plan(), seeded_id(0x31, 0), entry, members)
                .expect("a well-formed declaration");
            let root_name = draft.intern_string("sites");
            let root = draft
                .add_root_occurrence(
                    &admitted_plan(),
                    seeded_id(0x31, 0),
                    RootOccurrenceDef {
                        name: root_name,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Int,
                            id: seeded_id(0x34, 0),
                        }],
                        placement: seeded_id(0x35, 0),
                        indexes: Vec::new().into(),
                    },
                )
                .expect("the Product is declared");
            for member in &fields {
                let handle = draft
                    .bind_occurrence_site(
                        root.occurrence(),
                        member.path(),
                        SemanticTarget::FieldLeaf,
                    )
                    .expect("a declared field leaf");
                draft.request_site(&handle).expect("a live demand");
            }
            let payload = draft
                .bind_occurrence_site(
                    root.occurrence(),
                    root.placement_path(),
                    SemanticTarget::WholePayload,
                )
                .expect("a keyed placement");
            // The crossing is nonblocking: the plan records a receipt and the encoder
            // reports the Sites bound.
            draft.request_site(&payload).expect("a live demand");
        }
        Overflow::Functions => {
            let name = draft.intern_string("f");
            let src = draft.intern_string("src/extra.mw");
            for _ in 0..MAX_FUNCTIONS {
                draft
                    .add_function(FunctionDef {
                        name,
                        source: src,
                        params: Vec::new(),
                        ret: ImageType::scalar(Scalar::Int),
                        local_count: 0,
                        spans: Vec::new(),
                        code: vec![Instr::Return],
                    })
                    .expect("every site operand is live");
            }
        }
        // Each extra export targets its own structurally valid function, honoring v0's
        // one-export-per-function relation while only the export table crosses its cap.
        Overflow::Exports => {
            let src = draft.intern_string("src/extra.mw");
            let zero = draft.intern_int(0);
            for index in 0..MAX_EXPORTS {
                let name = draft.intern_string(&format!("extra{index}"));
                let func = draft
                    .add_function(FunctionDef {
                        name,
                        source: src,
                        params: Vec::new(),
                        ret: ImageType::scalar(Scalar::Int),
                        local_count: 0,
                        spans: Vec::new(),
                        code: vec![Instr::ConstLoad(zero.index()), Instr::Return],
                    })
                    .expect("every site operand is live");
                draft.add_export(ExportId::of_local("", &format!("extra{index}")), func);
            }
        }
        // Each test entry names its own unexported zero-argument unit function, honoring
        // the unique-test-function, export/test-disjointness, and unit-return relations
        // while only the test-entry table crosses its cap.
        Overflow::TestEntries => {
            let src = draft.intern_string("src/tests.mw");
            for index in 0..=MAX_TEST_ENTRIES {
                let name = draft.intern_string(&format!("t{index}"));
                let func = draft
                    .add_function(FunctionDef {
                        name,
                        source: src,
                        params: Vec::new(),
                        ret: ImageType::Unit,
                        local_count: 0,
                        spans: Vec::new(),
                        code: vec![Instr::Return],
                    })
                    .expect("every site operand is live");
                draft.add_test_entry(name, func);
            }
        }
    }
}

/// A distinct 16-byte ledger id from one tag byte and one two-byte seed, disjoint from
/// the base fixture's constant-fill ids.
fn seeded_id(tag: u8, index: usize) -> LedgerIdBytes {
    let index = u16::try_from(index).expect("a seed fits two bytes");
    let mut bytes = [0x40u8; 16];
    bytes[0] = tag;
    bytes[1..3].copy_from_slice(&index.to_be_bytes());
    LedgerIdBytes::from_bytes(bytes)
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

// ---- The mixed-corruption matrix (see the module header). Each test pins the
// pre-restructure verdict of one resource-policy cap crossed with one invariant defect.

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `TooManyStructLeaves`, and the flip must cite this pin.
#[test]
fn over_strings_with_an_over_wide_struct_currently_draws_the_string_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Strings)
            .value(Value::OverWideStruct)
            .encode(),
        Err(ImageBuildError::TooManyStrings),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `TooManyStructLeaves`, and the flip must cite this pin.
#[test]
fn an_over_long_string_with_an_over_wide_struct_currently_draws_the_string_length_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::StringBytes)
            .value(Value::OverWideStruct)
            .encode(),
        Err(ImageBuildError::StringTooLong),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `TooManyKeyColumns`, and the flip must cite this pin.
#[test]
fn over_consts_with_an_over_wide_key_currently_draws_the_const_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Consts)
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyConsts),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `TooManyStructLeaves`, and the flip must cite this pin.
#[test]
fn over_types_with_an_over_wide_struct_currently_draws_the_type_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Types)
            .value(Value::OverWideStruct)
            .encode(),
        Err(ImageBuildError::TooManyTypes),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `DurableValueTooDeep`, and the flip must cite this pin.
#[test]
fn over_enums_with_an_over_deep_value_currently_draws_the_enum_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Enums)
            .value(Value::OverDeep)
            .encode(),
        Err(ImageBuildError::TooManyEnums),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `TooManyKeyColumns`, and the flip must cite this pin.
#[test]
fn over_collections_with_an_over_wide_key_currently_draws_the_collection_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Collections)
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyCollections),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip either case to the invariant, and the flip must cite this pin.
#[test]
fn over_roots_with_a_missing_application_anchor_currently_draws_the_root_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Roots)
            .without_application()
            .encode(),
        Err(ImageBuildError::TooManyRoots),
    );
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Roots)
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyRoots),
        "the root cap likewise precedes the key-column invariant today",
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `TooManyLocals`, and the flip must cite this pin.
#[test]
fn over_sites_with_over_locals_currently_draws_the_site_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Sites)
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManySites),
    );
}

/// Pins a pre-restructure verdict the restructure must keep: the key-column invariant
/// already outranks the site cap today, so the sanctioned correction changes nothing here.
#[test]
fn over_sites_with_an_over_wide_key_currently_draws_the_key_column_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Sites)
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `TooManyLocals`, and the flip must cite this pin.
#[test]
fn over_functions_with_over_locals_currently_draws_the_function_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Functions)
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManyFunctions),
    );
}

/// Pins a pre-restructure verdict the restructure must keep: the struct-leaf invariant
/// already outranks the function cap today, so the sanctioned correction changes nothing
/// here.
#[test]
fn over_functions_with_an_over_wide_struct_currently_draws_the_struct_leaf_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Functions)
            .value(Value::OverWideStruct)
            .encode(),
        Err(ImageBuildError::TooManyStructLeaves),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `LocalCountBelowParams`, and the flip must cite this pin.
#[test]
fn over_exports_with_locals_below_params_currently_draws_the_export_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Exports)
            .frame(Frame::LocalsBelowParams)
            .encode(),
        Err(ImageBuildError::TooManyExports),
    );
}

/// Pins the pre-restructure verdict; the sanctioned invariant-over-resource correction
/// may flip it to `TooManyLocals`, and the flip must cite this pin.
#[test]
fn over_test_entries_with_over_locals_currently_draws_the_test_entry_cap() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::TestEntries)
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManyTestEntries),
    );
}

/// Pins a pre-restructure verdict the restructure must keep: the local invariant already
/// outranks CodeBytes today, so the sanctioned correction changes nothing here.
#[test]
fn over_code_bytes_with_over_locals_currently_draws_the_local_invariant() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManyLocals),
    );
}

/// Pins a pre-restructure verdict the restructure must keep: the value-depth invariant
/// already outranks CodeBytes today, so the sanctioned correction changes nothing here.
#[test]
fn over_code_bytes_with_an_over_deep_value_currently_draws_the_value_depth_invariant() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .value(Value::OverDeep)
            .encode(),
        Err(ImageBuildError::DurableValueTooDeep),
    );
}

/// Pins a pre-restructure verdict the restructure must keep: with the same draft over the
/// whole-image ceiling and carrying a key-column defect, the invariant already wins today.
#[test]
fn a_body_past_the_ceiling_with_an_over_wide_key_currently_draws_the_key_column_invariant() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverCeiling)
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
    );
}

/// Pins the pre-restructure verdict of the same draft over the whole-image ceiling AND
/// over a policy cap: the string cap wins today, and any reordering must cite this pin.
#[test]
fn a_body_past_the_ceiling_with_over_strings_currently_draws_the_string_cap() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverCeiling)
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::TooManyStrings),
    );
}

// ---- The invariant×invariant matrix: one draft carrying two invariant-classified
// defects, pinning which the encoder reports today. Invariant-relative order is frozen;
// no restructure may flip any of these.

/// Invariant-relative order is frozen; no restructure may flip this: the per-record
/// field width is decided before the occurrence key tuple.
#[test]
fn an_over_wide_record_with_an_over_wide_key_currently_draws_the_field_width_invariant() {
    assert_eq!(
        Fixture::clean().over_wide_record().over_wide_key().encode(),
        Err(ImageBuildError::TooManyFields),
    );
}

/// Invariant-relative order is frozen; no restructure may flip this: the value-shape
/// arena's struct width is decided before the function frame.
#[test]
fn an_over_wide_struct_with_over_locals_currently_draws_the_struct_leaf_invariant() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverWideStruct)
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManyStructLeaves),
    );
}

/// Invariant-relative order is frozen; no restructure may flip this: the occurrence key
/// tuple is decided before the function frame.
#[test]
fn an_over_wide_key_with_over_locals_currently_draws_the_key_column_invariant() {
    assert_eq!(
        Fixture::clean()
            .over_wide_key()
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
    );
}

/// Invariant-relative order is frozen; no restructure may flip this: the declaration
/// graph's value depth is decided before the application anchor.
#[test]
fn an_over_deep_value_with_a_missing_application_anchor_currently_draws_the_value_depth_invariant()
{
    assert_eq!(
        Fixture::clean()
            .value(Value::OverDeep)
            .without_application()
            .encode(),
        Err(ImageBuildError::DurableValueTooDeep),
    );
}

/// Invariant-relative order is frozen; no restructure may flip this: the function frame
/// is decided before the application anchor.
#[test]
fn over_locals_with_a_missing_application_anchor_currently_draws_the_local_invariant() {
    assert_eq!(
        Fixture::clean()
            .frame(Frame::OverLocals)
            .without_application()
            .encode(),
        Err(ImageBuildError::TooManyLocals),
    );
}

/// Invariant-relative order is frozen; no restructure may flip this: the Product claim
/// conflict is decided before the occurrence key tuple.
#[test]
fn a_product_conflict_with_an_over_wide_key_currently_draws_the_product_conflict() {
    assert_eq!(
        Fixture::clean()
            .with_conflicting_product()
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::ProductGraphConflict),
    );
}
