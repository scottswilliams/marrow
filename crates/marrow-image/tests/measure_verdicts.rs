//! The measure core's verdict pins: which producer-side result a draft with several
//! faults reports, and what the encoder allocates on the way to reporting it.
//!
//! A durable field's value is spelled on the wire as its full expansion, so a draft
//! inside every declared bound can still describe a DURABLE body larger than any image
//! may be. Every section is refused by counting rather than by building: the measure
//! core decides coherence, then policy, then the measured ceiling, before assembling
//! anything.
//!
//! Each case below combines faults across those steps and pins the exact result and
//! the exact offending row. The ordering they pin is the measure core's installed
//! order: every invariant-classified decision first, in the stable-partition sequence
//! (the `check_bounds` invariant subsequence, the durable graph's coherence, then the
//! hoisted emission-order reference checks); then every policy cap in the legacy
//! candidate order; then per-function CodeBytes; then the measured whole-image
//! ceiling. Flipped verdicts cite the pre-restructure pins they flipped in their doc
//! comments; git history carries the pre-restructure baselines.
//!
//! That no contract hash is ever computed over bytes no image can carry is structural
//! rather than measured: the producer mints an identity at exactly one site, inside the
//! planned emission the measured ceiling already admitted, and the identity owner
//! refuses a canonical payload past its own ceiling whoever asks (see the absence gate
//! `the_contract_identity_has_one_mint_per_side_and_a_bound_of_its_own`).
//!
//! # The mixed-corruption matrix
//!
//! The tests below the six bridge cases pin the CURRENT verdict for one draft carrying a
//! resource-policy-class defect (an aggregate table cap, a string-length cap, CodeBytes,
//! or the whole-image byte ceiling) and an invariant-class defect (an over-wide key tuple
//! or struct, an over-deep durable value, a broken frame, a missing application anchor)
//! at the same time. The pre-restructure verdict was purely positional — `check_bounds`
//! order, then the application anchor, then CodeBytes, then the ceiling — so a cap
//! declared before the durable-graph walk outranked every invariant while a cap
//! declared after it did not. The sanctioned invariant-over-resource restructure
//! flipped exactly the twelve sanctioned cells; every flip cites its pin, and every
//! other cell keeps its verdict.
//!
//! Every resource-policy candidate the encoder reports today is reachable through this
//! fixture, so no pair in the matrix is skipped as unconstructible: the caps sitting on
//! aggregate tables (strings, consts, types, enums, collections, roots, sites, functions,
//! exports, test entries) are driven by appending rows, and the byte-shaped caps
//! (StringBytes, CodeBytes, the whole-image ceiling) by one oversized element each.

use marrow_image::bounds::{
    MAX_CODE_BYTES, MAX_COLLECTIONS, MAX_CONSTS, MAX_DURABLE_VALUE_DEPTH, MAX_ENUMS, MAX_EXPORTS,
    MAX_FUNCTIONS, MAX_KEY_COLUMNS, MAX_LOCALS, MAX_RECORD_FIELDS, MAX_ROOTS, MAX_SITES,
    MAX_STRING_BYTES, MAX_STRINGS, MAX_STRUCT_LEAVES, MAX_TEST_ENTRIES, MAX_TYPES, MAX_VARIANTS,
};
use marrow_image::{
    CollTypeId, CollectionTypeDef, ConstId, DeclarationMemberDef, DeclarationMemberShape,
    DurableIndexComponent, DurableIndexShape, EnumId, EnumTypeDef, ExportId, FieldDef, FuncId,
    FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes,
    RecordTypeDef, RootId, RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry, StrId, TypeId,
    ValueShapeNodeId, VariantDef,
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
    /// A node id minted by a larger arena in a different draft, out of range for this
    /// draft's own arena: the raw-indexing defect the checked-conversion class covers.
    Forged,
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
            Value::Forged => {
                // Index 2 in a three-node arena; the fixture draft's arena holds one.
                let mut other = ImageDraft::new();
                let foreign = other.value_shapes_mut();
                foreign.scalar(Scalar::Int);
                foreign.scalar(Scalar::Bool);
                foreign.scalar(Scalar::Text)
            }
        }
    }
}

/// An instruction operand no table row answers: the raw-indexing defect the
/// checked-conversion class covers.
const OUT_OF_RANGE: u16 = u16::MAX;

// Forged string-pool ids (`StrId::from_index` is public: a logical string id is a pool
// position, not a capability). Every value is far past any fixture's pool, and each
// site gets a DISTINCT index so an out-of-bounds panic message names the site that
// panicked first.
const FORGED_RECORD_NAME: u16 = 60000;
const FORGED_TEST_ENTRY_NAME: u16 = 60002;
const FORGED_ENUM_NAME: u16 = 60003;
const FORGED_BRANCH_NAME: u16 = 61000;

/// A type reference naming a TYPES row no fixture declares: the raw table ordinal is
/// written to the wire unchecked today.
const FORGED_TYPE: ImageType = ImageType::Record {
    idx: TypeId::from_index(u16::MAX),
    optional: false,
};

/// The optional spelling of [`FORGED_TYPE`], for `VacantLoad`: the operand must be
/// optional for verification to reach the record-ordinal check at all.
const FORGED_OPT_TYPE: ImageType = ImageType::Record {
    idx: TypeId::from_index(u16::MAX),
    optional: true,
};

/// How a fixture's `main` is shaped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Code {
    /// Two instructions.
    Short,
    /// Past `MAX_CODE_BYTES`, so the function section refuses it.
    OverCodeBytes,
    /// A `ConstLoad` whose index no constant answers.
    BadConst,
    /// A jump whose target names no instruction.
    BadJump,
    /// Past `MAX_CODE_BYTES` and every `ConstLoad` index unanswered.
    OverCodeBytesBadConst,
    /// Past `MAX_CODE_BYTES` with a jump whose target names no instruction.
    OverCodeBytesBadJump,
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
    bad_span: bool,
    wide_enum_definition: bool,
    wide_enum_value_node: bool,
    branch_wide_key: bool,
    forged_record_name: bool,
    forged_function_name: bool,
    forged_enum_name: bool,
    forged_branch_name: bool,
    forged_test_entry_name: bool,
    forged_export_target: bool,
    forged_test_entry_target: bool,
    forged_field_type: bool,
    forged_enum_payload_type: bool,
    forged_collection_elem: bool,
    forged_entry_record: bool,
    forged_branch_record: bool,
    forged_param_type: bool,
    dangling_traversal: bool,
    dangling_index_scan: bool,
    duplicate_export: bool,
    final_overage: bool,
    extra_instrs: Vec<Instr>,
    extra_function: Option<Code>,
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
            bad_span: false,
            wide_enum_definition: false,
            wide_enum_value_node: false,
            branch_wide_key: false,
            forged_record_name: false,
            forged_function_name: false,
            forged_enum_name: false,
            forged_branch_name: false,
            forged_test_entry_name: false,
            forged_export_target: false,
            forged_test_entry_target: false,
            forged_field_type: false,
            forged_enum_payload_type: false,
            forged_collection_elem: false,
            forged_entry_record: false,
            forged_branch_record: false,
            forged_param_type: false,
            dangling_traversal: false,
            dangling_index_scan: false,
            duplicate_export: false,
            final_overage: false,
            extra_instrs: Vec::new(),
            extra_function: None,
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

    /// A span entry whose `instr_index` names no instruction of `main`: the
    /// raw-indexing defect the checked-conversion class covers.
    fn with_bad_span(mut self) -> Self {
        self.bad_span = true;
        self
    }

    /// An enum DEFINITION one variant past `MAX_VARIANTS`: the definition-site
    /// `TooManyVariants`, decided among the fixed table bounds.
    fn with_wide_enum_definition(mut self) -> Self {
        self.wide_enum_definition = true;
        self
    }

    /// A value-DAG enum node one member past `MAX_VARIANTS`: the arena-site
    /// `TooManyVariants`, decided by the whole-arena walk after the fixed table bounds.
    fn with_wide_enum_value_node(mut self) -> Self {
        self.wide_enum_value_node = true;
        self
    }

    /// A declaration BRANCH member one key column past `MAX_KEY_COLUMNS`: the
    /// declaration-site `TooManyKeyColumns`, decided in the declaration-graph walk
    /// before the occurrence loop.
    fn with_branch_wide_key(mut self) -> Self {
        self.branch_wide_key = true;
        self
    }

    /// The base record's TYPES-table name is a pool id no string answers.
    fn with_forged_record_name(mut self) -> Self {
        self.forged_record_name = true;
        self
    }

    /// `main`'s FUNCTIONS-table name is a pool id no string answers.
    fn with_forged_function_name(mut self) -> Self {
        self.forged_function_name = true;
        self
    }

    /// An ENUMS-table definition whose name is a pool id no string answers.
    fn with_forged_enum_name(mut self) -> Self {
        self.forged_enum_name = true;
        self
    }

    /// A declaration BRANCH member (otherwise valid) whose DURABLE-section name is a
    /// pool id no string answers.
    fn with_forged_branch_name(mut self) -> Self {
        self.forged_branch_name = true;
        self
    }

    /// A TEST-ENTRY row whose name is a pool id no string answers.
    fn with_forged_test_entry_name(mut self) -> Self {
        self.forged_test_entry_name = true;
        self
    }

    /// An EXPORTS row whose function index no row answers.
    fn with_forged_export_target(mut self) -> Self {
        self.forged_export_target = true;
        self
    }

    /// A TEST-ENTRY row (valid name) whose function index no row answers.
    fn with_forged_test_entry_target(mut self) -> Self {
        self.forged_test_entry_target = true;
        self
    }

    /// The base record carries one field whose `ImageType` names no TYPES row.
    fn with_forged_field_type(mut self) -> Self {
        self.forged_field_type = true;
        self
    }

    /// An ENUMS definition whose variant payload leaf names no TYPES row.
    fn with_forged_enum_payload_type(mut self) -> Self {
        self.forged_enum_payload_type = true;
        self
    }

    /// A COLLTYPES row whose element `ImageType` names no TYPES row.
    fn with_forged_collection_elem(mut self) -> Self {
        self.forged_collection_elem = true;
        self
    }

    /// The Product declaration's root entry record names no TYPES row (`TypeId` is a
    /// raw newtype with a public `from_index`).
    fn with_forged_entry_record(mut self) -> Self {
        self.forged_entry_record = true;
        self
    }

    /// A declaration BRANCH member (otherwise valid) whose entry record names no
    /// TYPES row.
    fn with_forged_branch_record(mut self) -> Self {
        self.forged_branch_record = true;
        self
    }

    /// `main` carries a bounded traversal over a live whole-payload site whose
    /// `list_ty` names no COLLTYPES row: the operand is a public raw ordinal written
    /// unchecked today.
    fn with_dangling_traversal(mut self) -> Self {
        self.dangling_traversal = true;
        self
    }

    /// `main`'s one parameter `ImageType` names no TYPES row.
    fn with_forged_param_type(mut self) -> Self {
        self.forged_param_type = true;
        self
    }

    /// The root gains a nonunique managed index, and `main` carries an index scan over
    /// its live site whose `list_ty` names no COLLTYPES row.
    fn with_dangling_index_scan(mut self) -> Self {
        self.dangling_index_scan = true;
        self
    }

    /// A second export naming `main`'s function index: a duplicate export target, a
    /// relation the encoder accepts unchecked today.
    fn with_duplicate_export(mut self) -> Self {
        self.duplicate_export = true;
        self
    }

    /// A string pool inside both of its caps whose bytes alone exceed the whole-image
    /// ceiling: every section fits its own bound and the DURABLE body alone fits, so
    /// only the measured whole-image total can refuse the draft.
    fn with_final_overage(mut self) -> Self {
        self.final_overage = true;
        self
    }

    /// Instructions appended to `main`'s body after its [`Code`] shape.
    fn with_extra_instrs(mut self, instrs: Vec<Instr>) -> Self {
        self.extra_instrs = instrs;
        self
    }

    /// A second function added after `main`, shaped by its own [`Code`].
    fn with_extra_function(mut self, code: Code) -> Self {
        self.extra_function = Some(code);
        self
    }

    fn encode(self) -> Result<(), ImageBuildError> {
        let mut draft = ImageDraft::new();
        let value = self.value.shape(&mut draft);
        let type_name = if self.forged_record_name {
            StrId::from_index(FORGED_RECORD_NAME)
        } else {
            draft.intern_string("R")
        };
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
        } else if self.forged_field_type {
            let field_name = draft.intern_string("f");
            vec![FieldDef {
                name: field_name,
                ty: FORGED_TYPE,
                required: true,
            }]
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
        if self.forged_enum_name {
            draft.add_enum_type(EnumTypeDef {
                name: StrId::from_index(FORGED_ENUM_NAME),
                variants: Vec::new(),
            });
        }
        if self.wide_enum_definition {
            let name = draft.intern_string("WideEnum");
            let variant_name = draft.intern_string("v");
            draft.add_enum_type(EnumTypeDef {
                name,
                variants: vec![
                    VariantDef {
                        name: variant_name,
                        category: false,
                        payload: Vec::new(),
                    };
                    MAX_VARIANTS + 1
                ],
            });
        }
        if self.wide_enum_value_node {
            // The arena is validated as a whole, so the node needs no referencing field.
            let members = (0..=MAX_VARIANTS)
                .map(|index| (seeded_id(0x61, index), Vec::new()))
                .collect();
            draft
                .value_shapes_mut()
                .enum_shape(seeded_id(0x60, 0), members);
        }
        let mut members = vec![DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: LedgerIdBytes::from_bytes(FIELD_ID),
                required: true,
                value,
            },
        }];
        if self.branch_wide_key {
            let branch_name = draft.intern_string("b");
            members.push(DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Branch {
                    placement: seeded_id(0x71, 0),
                    name: branch_name,
                    record,
                    keys: (0..=MAX_KEY_COLUMNS)
                        .map(|column| KeyColumn {
                            scalar: Scalar::Int,
                            id: seeded_id(0x72, column),
                        })
                        .collect(),
                },
            });
        }
        if self.forged_branch_name {
            members.push(DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Branch {
                    placement: seeded_id(0x74, 0),
                    name: StrId::from_index(FORGED_BRANCH_NAME),
                    record,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: seeded_id(0x73, 0),
                    }],
                },
            });
        }
        if self.forged_branch_record {
            let branch_name = draft.intern_string("fb");
            members.push(DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Branch {
                    placement: seeded_id(0x76, 0),
                    name: branch_name,
                    record: TypeId::from_index(u16::MAX),
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: seeded_id(0x75, 0),
                    }],
                },
            });
        }
        let entry_record = if self.forged_entry_record {
            TypeId::from_index(u16::MAX)
        } else {
            record
        };
        draft
            .declare_product(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                entry_record,
                members,
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
        let indexes = if self.dangling_index_scan {
            vec![DurableIndexShape {
                id: seeded_id(0x77, 0),
                unique: false,
                components: vec![
                    DurableIndexComponent::Field(LedgerIdBytes::from_bytes(FIELD_ID)),
                    DurableIndexComponent::Key(key_id(0)),
                ],
            }]
        } else {
            Vec::new()
        };
        let root = draft
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
                    indexes: indexes.into(),
                },
            )
            .expect("the Product is declared");
        let src = draft.intern_string("src/main.mw");
        let main_name = if self.forged_function_name {
            StrId::from_index(OUT_OF_RANGE)
        } else {
            draft.intern_string("main")
        };
        let zero = draft.intern_int(0);
        let mut code = body(self.code, zero);
        code.extend(self.extra_instrs.iter().cloned());
        if self.dangling_traversal {
            let handle = draft
                .bind_occurrence_site(
                    root.occurrence(),
                    root.placement_path(),
                    SemanticTarget::WholePayload,
                )
                .expect("a keyed placement");
            let site = draft.request_site(&handle).expect("a live demand");
            code.push(Instr::DurIterateBounded {
                site,
                limit: 2,
                from: false,
                list_ty: CollTypeId::from_index(u16::MAX),
            });
        }
        if self.dangling_index_scan {
            let scan_path = root.index_paths()[0].clone();
            let handle = draft
                .bind_occurrence_site(root.occurrence(), &scan_path, SemanticTarget::IndexScan)
                .expect("a managed index");
            let site = draft.request_site(&handle).expect("a live demand");
            code.push(Instr::DurIndexScan {
                site,
                limit: 2,
                from: false,
                list_ty: CollTypeId::from_index(u16::MAX),
            });
        }
        let params = if self.forged_param_type {
            vec![FORGED_TYPE]
        } else {
            match self.frame {
                Frame::LocalsBelowParams => vec![ImageType::scalar(Scalar::Int)],
                Frame::Fits | Frame::OverLocals => Vec::new(),
            }
        };
        let local_count = if self.forged_param_type {
            1
        } else {
            match self.frame {
                Frame::OverLocals => (MAX_LOCALS + 1) as u16,
                Frame::Fits | Frame::LocalsBelowParams => 0,
            }
        };
        let main = draft
            .add_function(FunctionDef {
                name: main_name,
                source: src,
                params,
                ret: ImageType::scalar(Scalar::Int),
                local_count,
                spans: vec![SpanEntry {
                    // `u32::MAX` names no instruction of any fixture body, however long.
                    instr_index: if self.bad_span { u32::MAX } else { 0 },
                    line: 1,
                    column: 1,
                }],
                code,
            })
            .expect("every site operand is live");
        draft.add_export(ExportId::of_local("", "main"), main);
        if self.duplicate_export {
            draft.add_export(ExportId::of_local("", "dup"), main);
        }
        if let Some(extra) = self.extra_function {
            let aux_name = draft.intern_string("aux");
            draft
                .add_function(FunctionDef {
                    name: aux_name,
                    source: src,
                    params: Vec::new(),
                    ret: ImageType::scalar(Scalar::Int),
                    local_count: 0,
                    spans: Vec::new(),
                    code: body(extra, zero),
                })
                .expect("every site operand is live");
        }
        if self.forged_test_entry_name {
            draft.add_test_entry(StrId::from_index(FORGED_TEST_ENTRY_NAME), main);
        }
        if self.final_overage {
            // 132 distinct strings of 4,004 bytes: 528,528 string bytes — every string
            // inside MAX_STRING_BYTES, the count far inside MAX_STRINGS, and the sum
            // past MAX_IMAGE_BYTES (524,288) on its own.
            for index in 0..132 {
                draft.intern_string(&format!("{:04}{}", index, "x".repeat(4000)));
            }
        }
        if self.forged_test_entry_target {
            let entry_name = draft.intern_string("tt");
            draft.add_test_entry(entry_name, forged_func_id());
        }
        if self.forged_export_target {
            draft.add_export(ExportId::of_local("", "ghost"), forged_func_id());
        }
        if self.forged_enum_payload_type {
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
        }
        if self.forged_collection_elem {
            draft.add_collection_type(CollectionTypeDef::List { elem: FORGED_TYPE });
        }
        if let Some(policy) = self.policy {
            apply_policy(policy, &mut draft);
        }
        draft.encode().map(|_| ())
    }
}

/// One function body per [`Code`] shape, over the fixture's zero constant.
fn body(code: Code, zero: ConstId) -> Vec<Instr> {
    match code {
        Code::Short => vec![Instr::ConstLoad(zero), Instr::Return],
        // `ConstLoad` is three bytes, so this is comfortably past the limit while
        // staying well inside the instruction count the draft admits.
        Code::OverCodeBytes => {
            let mut code = vec![Instr::ConstLoad(zero); MAX_CODE_BYTES / 2];
            code.push(Instr::Return);
            code
        }
        Code::BadConst => vec![
            Instr::ConstLoad(ConstId::from_index(OUT_OF_RANGE)),
            Instr::Return,
        ],
        Code::BadJump => vec![
            Instr::ConstLoad(zero),
            Instr::Jump(u32::from(OUT_OF_RANGE)),
            Instr::Return,
        ],
        Code::OverCodeBytesBadConst => {
            let mut code =
                vec![Instr::ConstLoad(ConstId::from_index(OUT_OF_RANGE)); MAX_CODE_BYTES / 2];
            code.push(Instr::Return);
            code
        }
        Code::OverCodeBytesBadJump => {
            let mut code = vec![Instr::ConstLoad(zero); MAX_CODE_BYTES / 2];
            code.push(Instr::Jump(u32::from(OUT_OF_RANGE)));
            code.push(Instr::Return);
            code
        }
    }
}

/// A function index no fixture's final table can answer — `u16::MAX`, minted by a
/// draft that fills the whole index space. A `FuncId` is a table position, not a
/// capability bound to its draft, and the ordinal must sit beyond every table a policy
/// overflow can grow: an ordinal of 1 would be healed into validity by the fixtures
/// that append real functions (the TestEntries and Exports overflows).
fn forged_func_id() -> FuncId {
    let mut other = ImageDraft::new();
    let src = other.intern_string("s");
    let name = other.intern_string("f");
    let def = FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: Vec::new(),
        code: vec![Instr::Return],
    };
    let mut last = other
        .add_function(def.clone())
        .expect("every site operand is live");
    while last.index() < u16::MAX {
        last = other
            .add_function(def.clone())
            .expect("every site operand is live");
    }
    last
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
                        code: vec![Instr::ConstLoad(zero), Instr::Return],
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

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the struct-leaf invariant now outranks the string cap.
#[test]
fn over_strings_with_an_over_wide_struct_draws_the_struct_leaf_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Strings)
            .value(Value::OverWideStruct)
            .encode(),
        Err(ImageBuildError::TooManyStructLeaves),
    );
}

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the struct-leaf invariant now outranks the string-length cap.
#[test]
fn an_over_long_string_with_an_over_wide_struct_draws_the_struct_leaf_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::StringBytes)
            .value(Value::OverWideStruct)
            .encode(),
        Err(ImageBuildError::TooManyStructLeaves),
    );
}

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the key-column invariant now outranks the const cap.
#[test]
fn over_consts_with_an_over_wide_key_draws_the_key_column_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Consts)
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
    );
}

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the struct-leaf invariant now outranks the type cap.
#[test]
fn over_types_with_an_over_wide_struct_draws_the_struct_leaf_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Types)
            .value(Value::OverWideStruct)
            .encode(),
        Err(ImageBuildError::TooManyStructLeaves),
    );
}

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the value-depth invariant now outranks the enum cap.
#[test]
fn over_enums_with_an_over_deep_value_draws_the_value_depth_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Enums)
            .value(Value::OverDeep)
            .encode(),
        Err(ImageBuildError::DurableValueTooDeep),
    );
}

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the key-column invariant now outranks the collection cap.
#[test]
fn over_collections_with_an_over_wide_key_draws_the_key_column_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Collections)
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
    );
}

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the application anchor (and the key-column invariant) now outranks the root cap.
#[test]
fn over_roots_with_a_missing_application_anchor_draws_the_anchor_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Roots)
            .without_application()
            .encode(),
        Err(ImageBuildError::InvalidReference("application identity")),
    );
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Roots)
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
        "the key-column invariant likewise outranks the root cap",
    );
}

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the local invariant now outranks the site cap.
#[test]
fn over_sites_with_over_locals_draws_the_local_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Sites)
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManyLocals),
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

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the local invariant now outranks the function cap.
#[test]
fn over_functions_with_over_locals_draws_the_local_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Functions)
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManyLocals),
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

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the frame invariant now outranks the export cap.
#[test]
fn over_exports_with_locals_below_params_draws_the_frame_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::Exports)
            .frame(Frame::LocalsBelowParams)
            .encode(),
        Err(ImageBuildError::LocalCountBelowParams),
    );
}

/// Flipped by the sanctioned invariant-over-resource restructure (design §B),
/// citing the pre-restructure pin this test carried: coherence decides every
/// invariant before any policy candidate, so the local invariant now outranks the test-entry cap.
#[test]
fn over_test_entries_with_over_locals_draws_the_local_invariant() {
    assert_eq!(
        Fixture::clean()
            .policy(Overflow::TestEntries)
            .frame(Frame::OverLocals)
            .encode(),
        Err(ImageBuildError::TooManyLocals),
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

// ---- The checked-conversion pins: raw indexing on a caller-supplied id, crossed with
// the policy cap that decides first today. The restructure is sanctioned to convert the
// raw indexing into checked lookups; what each defect draws when it stands alone is
// pinned beside its pair so the conversion has both baselines.

/// Flipped under the sanctioned checked-conversion class (`value shape`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the string cap.
#[test]
fn a_forged_value_node_with_over_strings_draws_the_value_shape_reference() {
    assert_eq!(
        Fixture::clean()
            .value(Value::Forged)
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("value shape")),
    );
}

/// Flipped under the sanctioned panic-to-typed conversion, citing the
/// pre-restructure panic pin this test carried: the raw depth lookup is now the
/// declaration walk's checked arena lookup, `InvalidReference("value shape")`.
#[test]
fn a_forged_value_node_alone_draws_the_value_shape_reference() {
    assert_eq!(
        Fixture::clean().value(Value::Forged).encode(),
        Err(ImageBuildError::InvalidReference("value shape")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`constant`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn an_out_of_range_const_load_with_over_code_bytes_draws_the_constant_reference() {
    assert_eq!(
        Fixture::clean().code(Code::OverCodeBytesBadConst).encode(),
        Err(ImageBuildError::InvalidReference("constant")),
    );
}

/// Flipped under the sanctioned panic-to-typed conversion, citing the
/// pre-restructure panic pin this test carried: the raw const-map remap is now the
/// tape walk's `InvalidReference("constant")` range check.
#[test]
fn an_out_of_range_const_load_alone_draws_the_constant_reference() {
    assert_eq!(
        Fixture::clean().code(Code::BadConst).encode(),
        Err(ImageBuildError::InvalidReference("constant")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`span instruction`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn an_out_of_range_span_with_a_body_past_the_ceiling_draws_the_span_reference() {
    assert_eq!(
        Fixture::clean()
            .with_bad_span()
            .value(Value::OverCeiling)
            .encode(),
        Err(ImageBuildError::InvalidReference("span instruction")),
    );
}

/// Flipped under the sanctioned panic-to-typed conversion, citing the
/// pre-restructure panic pin this test carried: the raw offset lookup is now the
/// SPANS coherence item's `InvalidReference("span instruction")` range check.
#[test]
fn an_out_of_range_span_alone_draws_the_span_reference() {
    assert_eq!(
        Fixture::clean().with_bad_span().encode(),
        Err(ImageBuildError::InvalidReference("span instruction")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`jump target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn a_bad_jump_with_over_code_bytes_draws_the_jump_target_reference() {
    assert_eq!(
        Fixture::clean().code(Code::OverCodeBytesBadJump).encode(),
        Err(ImageBuildError::InvalidReference("jump target")),
    );
}

/// The jump target is the one code operand already answered with a typed error today;
/// the checked-conversion class must keep this exact result.
#[test]
fn a_bad_jump_alone_currently_draws_the_jump_target_reference() {
    assert_eq!(
        Fixture::clean().code(Code::BadJump).encode(),
        Err(ImageBuildError::InvalidReference("jump target")),
    );
}

// ---- The decision-site pins: one error variant, two decision sites. The intra-class
// order must not silently reorder, so each pair is pinned with both sites armed and
// each site alone.

/// Decision-site order is frozen: `TooManyVariants` is decided at the enum DEFINITION
/// (the fixed table bounds) before the value-DAG arena walk, and both sites draw the
/// same variant, so the combined draft must keep this exact result.
#[test]
fn an_enum_definition_and_a_value_dag_node_both_over_variants_currently_draw_too_many_variants() {
    assert_eq!(
        Fixture::clean()
            .with_wide_enum_definition()
            .with_wide_enum_value_node()
            .encode(),
        Err(ImageBuildError::TooManyVariants),
    );
    assert_eq!(
        Fixture::clean().with_wide_enum_definition().encode(),
        Err(ImageBuildError::TooManyVariants),
        "the definition site draws the variant alone",
    );
    assert_eq!(
        Fixture::clean().with_wide_enum_value_node().encode(),
        Err(ImageBuildError::TooManyVariants),
        "the value-DAG site draws the variant alone",
    );
}

/// Decision-site order is frozen: `TooManyKeyColumns` is decided at the declaration
/// BRANCH (the declaration-graph walk) before the root-occurrence loop, and both sites
/// draw the same variant, so the combined draft must keep this exact result.
#[test]
fn a_branch_and_an_occurrence_both_over_key_columns_currently_draw_too_many_key_columns() {
    assert_eq!(
        Fixture::clean()
            .with_branch_wide_key()
            .over_wide_key()
            .encode(),
        Err(ImageBuildError::TooManyKeyColumns),
    );
    assert_eq!(
        Fixture::clean().with_branch_wide_key().encode(),
        Err(ImageBuildError::TooManyKeyColumns),
        "the branch site draws the variant alone",
    );
    assert_eq!(
        Fixture::clean().over_wide_key().encode(),
        Err(ImageBuildError::TooManyKeyColumns),
        "the occurrence site draws the variant alone",
    );
}

/// Decision-site order is frozen: `InvalidReference` is decided at the application
/// anchor (the preflight) before jump-target validation inside code encoding, and the
/// two sites carry distinguishable payloads, so this winner is directly observable.
#[test]
fn a_missing_application_anchor_and_a_bad_jump_currently_draw_the_application_reference() {
    assert_eq!(
        Fixture::clean()
            .without_application()
            .code(Code::BadJump)
            .encode(),
        Err(ImageBuildError::InvalidReference("application identity")),
    );
}

// ---- The F-A boundary cells: each hoisted reference family crossed with the FIRST
// policy cap in candidate order (Strings) and the LAST (TestEntries). Coherence
// decides before any policy candidate, so the hoisted reference check wins in every
// cell; each pin cites the pre-restructure cap verdict it flipped. What each
// reference defect draws alone is pinned in the checked-conversion family above.

/// Flipped under the sanctioned checked-conversion class (`record name`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_forged_record_name_with_over_strings_draws_the_record_name_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_record_name()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("record name")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`constant`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn an_out_of_range_const_load_with_over_strings_draws_the_constant_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::BadConst)
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("constant")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`jump target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_jump_with_over_strings_draws_the_jump_target_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::BadJump)
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("jump target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`span instruction`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn an_out_of_range_span_with_over_strings_draws_the_span_reference() {
    assert_eq!(
        Fixture::clean()
            .with_bad_span()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("span instruction")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`record name`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn a_forged_record_name_with_over_test_entries_draws_the_record_name_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_record_name()
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("record name")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`constant`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn an_out_of_range_const_load_with_over_test_entries_draws_the_constant_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::BadConst)
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("constant")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`jump target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn a_bad_jump_with_over_test_entries_draws_the_jump_target_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::BadJump)
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("jump target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`span instruction`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn an_out_of_range_span_with_over_test_entries_draws_the_span_reference() {
    assert_eq!(
        Fixture::clean()
            .with_bad_span()
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("span instruction")),
    );
}

// ---- The F-B/F-C completion cells: the two byte-shaped results (CodeBytes and the
// whole-image ceiling) crossed with reference defects; the hoisted checks decide
// before either byte-shaped candidate.

/// Flipped under the sanctioned checked-conversion class (`span instruction`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn an_out_of_range_span_with_over_code_bytes_draws_the_span_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_bad_span()
            .encode(),
        Err(ImageBuildError::InvalidReference("span instruction")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`function name`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn a_forged_function_name_with_over_code_bytes_draws_the_function_name_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_forged_function_name()
            .encode(),
        Err(ImageBuildError::InvalidReference("function name")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`record name`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn a_forged_record_name_with_a_body_past_the_ceiling_draws_the_record_name_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_record_name()
            .value(Value::OverCeiling)
            .encode(),
        Err(ImageBuildError::InvalidReference("record name")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`enum name`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn a_forged_enum_name_with_a_body_past_the_ceiling_draws_the_enum_name_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_enum_name()
            .value(Value::OverCeiling)
            .encode(),
        Err(ImageBuildError::InvalidReference("enum name")),
    );
}

// ---- The intra-class reference-order pins: two forged references in one draft, in
// different sections. Both sites panicked before the restructure; each pair is now
// pinned by the typed reference of the EARLIER site, proving the coherence walk kept
// the emission-order subsequence the panics measured.
//
// One pair from the mandate is not constructible and is documented instead of forced:
// TYPES record-name vs CONSTS text reference. A text constant's string id is minted
// only by `ImageDraft::intern_text`, which interns the text itself, so no public path
// binds a raw `StrId` to a constant. Were it constructible, the CONSTS site would win
// today: the const sort key resolves text ids through the string sort map *before*
// function or section encoding begins.

/// Reference-order pin, flipped under the sanctioned panic-to-typed conversion and
/// citing the pre-restructure panic pin (`the index is 65535`): the per-function
/// item still precedes the TYPES item, so the earlier site's hoisted check —
/// `InvalidReference("function name")` — decides, never the record name's.
#[test]
fn a_forged_function_name_draws_its_reference_before_a_forged_record_name() {
    assert_eq!(
        Fixture::clean()
            .with_forged_function_name()
            .with_forged_record_name()
            .encode(),
        Err(ImageBuildError::InvalidReference("function name")),
    );
}

/// Reference-order pin, flipped under the sanctioned panic-to-typed conversion and
/// citing the pre-restructure panic pin (`the index is 4294967295`): the SPANS item
/// still precedes the TEST-ENTRY item, so the earlier site's hoisted check —
/// `InvalidReference("span instruction")` — decides, never the entry name's.
#[test]
fn an_out_of_range_span_draws_its_reference_before_a_forged_test_entry_name() {
    assert_eq!(
        Fixture::clean()
            .with_bad_span()
            .with_forged_test_entry_name()
            .encode(),
        Err(ImageBuildError::InvalidReference("span instruction")),
    );
}

/// Reference-order pin, flipped under the sanctioned panic-to-typed conversion and
/// citing the pre-restructure panic pin (`the index is 61000`): the DURABLE item
/// still precedes the TYPES item, so the earlier site's hoisted check —
/// `InvalidReference("branch name")` — decides, never the record name's.
#[test]
fn a_forged_branch_name_draws_its_reference_before_a_forged_record_name() {
    assert_eq!(
        Fixture::clean()
            .with_forged_branch_name()
            .with_forged_record_name()
            .encode(),
        Err(ImageBuildError::InvalidReference("branch name")),
    );
}

// ---- The named missing byte-fence cells (§A): CodeBytes and the whole-image ceiling
// crossed with once-unchecked table ordinals, now hoisted coherence checks that
// decide before either policy verdict.

/// Flipped under the sanctioned checked-conversion class (`call target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn a_bad_call_target_with_over_code_bytes_draws_the_call_target_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_extra_instrs(vec![Instr::Call(u16::MAX)])
            .encode(),
        Err(ImageBuildError::InvalidReference("call target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`test target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn a_bad_test_entry_target_with_over_code_bytes_draws_the_test_target_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_forged_test_entry_target()
            .encode(),
        Err(ImageBuildError::InvalidReference("test target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`call target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn a_bad_call_target_with_a_body_past_the_ceiling_draws_the_call_target_reference() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverCeiling)
            .with_extra_instrs(vec![Instr::Call(u16::MAX)])
            .encode(),
        Err(ImageBuildError::InvalidReference("call target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`export target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn a_bad_export_target_with_a_body_past_the_ceiling_draws_the_export_target_reference() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverCeiling)
            .with_forged_export_target()
            .encode(),
        Err(ImageBuildError::InvalidReference("export target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn a_bad_type_ordinal_with_a_body_past_the_ceiling_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverCeiling)
            .with_extra_instrs(vec![Instr::RecordNew(TypeId::from_index(u16::MAX))])
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

// ---- First-cap boundary cells for the remaining §B.3 reference families. Every
// ordinal here was written unchecked before the restructure, so the first policy cap
// decided trivially; the coherence hoist moved every one of these boundaries, and
// each cell cites the cap verdict it flipped. Standalone baselines for the same
// families live in `marrow-verify/tests/legacy_ok_pins.rs` (they encoded Ok before
// the hoist; they now pin the producer refusal).
//
// # Derivation law — member-by-member policy cutpoints, restated for the installed
// # measure core
//
// The FOUR pre-restructure cutpoints (the END of `check_bounds`, the PER-FUNCTION
// CodeBytes check, the DURABLE fence, the FINAL assembled-image ceiling) collapse
// under the measure core into ONE law: every coherence item — in the stable
// partition's emission-order sequence — decides before every policy candidate, the
// policy candidates keep their legacy candidate order (the eleven caps, then
// CodeBytes per function), and the whole-image ceiling is decided last, by the
// measured plan, envelope and frames included, before any section is assembled. A
// derived member's crossing verdicts therefore follow its representative's iff both
// defects are coherence-classified at the same sequence item — which §B.3's position
// assignment states — and every crossing against any policy candidate resolves to
// the coherence side. The final-overage cells at the end of this file pin the
// measured ceiling's position literally: coherence items (the span check, the
// duplicate-export relation) decide before it, and a final-only overage keeps its
// ImageTooLarge verdict, decided at measurement rather than after assembly.
//
// Correction to design draft 8's exclusion (the design correction rides here): calls
// INTO test entries were excluded as "needing call closure"; that rationale is false —
// the verifier scans DIRECT tape call targets in its seal phase, so the relation is
// pinned standalone in `legacy_ok_pins.rs` like every other export/test relation, and
// its policy crossings derive by the TEST-ENTRY-position argument below.
//
// Member — position — representative:
//
// - `ListNew`, `MapNew`, `TextSplit`, `TextLines` (collection ordinals): tape of the
//   carrying function, after its own CodeBytes check → `RecordNew` for
//   Strings/TestEntries/CodeBytes; ImageBytes via the unchecked-write-completes
//   argument (`ListNew` and `MapNew` standalone Ok-pins are literal;
//   `TextSplit`/`TextLines` share their exact operand kind and tape position).
// - `EnumConstruct.enum_idx` (+ its subordinate `variant`, checked against the
//   resolved enum): same tape position → `RecordNew`; literal Strings cell kept;
//   ImageBytes via the unchecked-write-completes argument.
// - `VacantLoad` embedded type: same tape position → `RecordNew`; literal Strings
//   cell kept; ImageBytes via the unchecked-write-completes argument.
// - `MakeIdentity.root`: same tape position → `RecordNew`; literal Strings cell kept;
//   ImageBytes via the unchecked-write-completes argument. Its `cols` relation is
//   reachable only past a VALID root (a dangling root ordinal is rejected first), and
//   under that precondition `cols` is the same unchecked tape write.
// - `DurIterateBounded.list_ty`: tape position, written after the operand's fallible
//   site validation — which cannot fire in these fixtures, because every crossing
//   carries a live provenance-validated site — so past that precondition it is an
//   unchecked write at the same tape position → `RecordNew`; literal Strings cell
//   kept; ImageBytes via the unchecked-write-completes argument.
// - `DurIndexScan.list_ty`: same precondition, its own instruction arm → literal
//   Strings cell below (not derived from the iterate arm); TestEntries/CodeBytes →
//   `RecordNew`; ImageBytes via the unchecked-write-completes argument.
// - Signature param/return `ImageType` refs: written inside the function row AFTER
//   that row's CodeBytes check and before its tape — so the Strings and CodeBytes
//   cells are literal below (the CodeBytes one measured, and consistent with the
//   check-first order); TestEntries → `RecordNew` (the cap decides in `check_bounds`,
//   before any row); ImageBytes via the unchecked-write-completes argument.
// - TYPES field-`ImageType`, ENUMS payload-`ImageType`, and the COLLTYPES sub-members
//   `List.elem`, `Map.key`, and `Map.value` (the value ref is written only after its
//   row's key ref, so reaching a `Map.value` defect presupposes a valid key): their
//   sections assemble after the FIRST THREE cutpoints and before cutpoint four, the
//   final assembled-image ceiling (different writers, but position is all the law
//   consumes) → export-target/test-target rows; literal Strings cells
//   kept (the COLLTYPES cell carries `List.elem`; the `Map` sub-members share its
//   section position and unchecked-write mechanics).
// - Branch record: DURABLE body at a deeper recursive position than the root entry
//   record, but the fence decides on the counted total before any of the body is
//   assembled, so both records sit identically after/before each cutpoint → root
//   entry record; literal Strings and ImageBytes cells kept.
// - Duplicate export target, duplicate export id, duplicate test target, duplicate
//   test name, export/test overlap, the test-signature law (both decision sites:
//   nonzero params and non-unit return), a call into a test entry, `Assert`
//   membership (a tape-position instruction whose only check is the verifier's
//   TEST-ENTRY seal scan — the encoder writes the opcode unchecked), and the
//   test-driver-mix law (a direct durable op beside a call to a transaction-owning
//   export — both halves ordinary unchecked tape writes): nothing here sits after
//   every encode cutpoint — the tape-carried members precede the DURABLE fence and
//   the final ceiling, and the EXPORTS/TEST-ENTRY rows precede the final ceiling;
//   only the VERIFIER-side relation decisions occur after a successful encode. The
//   derivation is therefore the unchecked-write one throughout: each relation's
//   defect is a write the encoder completes without an error, so every policy
//   decision that runs at all decides — the caps in `check_bounds`, then CodeBytes,
//   then the fence, then the final ceiling — and any pre-final PANIC (the map-indexed
//   lookups) wins before the final ceiling can. Representative rows:
//   export-target/test-target; a literal Strings × duplicate-export-target cell sits
//   below, their CodeBytes cells derive by the position argument, and their
//   ImageBytes cells split: fence overage via unchecked-write-completes, final
//   overage via the literal final-overage × duplicate-export-target cell.

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_record_new_ordinal_with_over_strings_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::RecordNew(TypeId::from_index(u16::MAX))])
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`collection type`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_list_new_ordinal_with_over_strings_draws_the_collection_type_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::ListNew(CollTypeId::from_index(u16::MAX))])
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`enum type`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_enum_construct_ordinal_with_over_strings_draws_the_enum_type_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::EnumConstruct {
                enum_idx: EnumId::from_index(u16::MAX),
                variant: 0,
            }])
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("enum type")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_vacant_load_type_with_over_strings_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::VacantLoad(FORGED_OPT_TYPE)])
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`root table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_make_identity_root_with_over_strings_draws_the_root_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::MakeIdentity {
                root: RootId::from_index(u16::MAX),
                cols: 0,
            }])
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("root table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_field_type_ordinal_with_over_strings_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_field_type()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_enum_payload_type_with_over_strings_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_enum_payload_type()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_collection_elem_type_with_over_strings_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_collection_elem()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

// ---- The across-function permutation: the const defect and the CodeBytes policy
// result in two different functions, both orders. All of coherence precedes all
// policy, so the reference decides in BOTH orders — the positional whichever-comes-
// first law these pins measured before the restructure is gone, and both flips cite
// their pins.

/// Flipped under the sanctioned panic-to-typed conversion, citing the
/// pre-restructure panic pin this test carried: all of coherence precedes all
/// policy, so the earlier function's bad const draws `InvalidReference("constant")`
/// before any function's length is checked.
#[test]
fn a_bad_const_in_the_earlier_function_draws_the_constant_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::BadConst)
            .with_extra_function(Code::OverCodeBytes)
            .encode(),
        Err(ImageBuildError::InvalidReference("constant")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`constant`), citing
/// the pre-restructure pin this test carried: all of coherence precedes all
/// policy, so even a LATER function's bad const outranks an earlier
/// function's CodeBytes — the positional law the restructure replaces.
#[test]
fn a_bad_const_in_the_later_function_draws_the_constant_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_extra_function(Code::BadConst)
            .encode(),
        Err(ImageBuildError::InvalidReference("constant")),
    );
}

// ---- The two once-omitted DURABLE type-table ordinals: the root
// entry record and a branch's entry record, both raw `TypeId` newtypes once written
// to the body unchecked, now checked at their exact body positions. Standalone
// producer-refusal pins live in `marrow-verify/tests/legacy_ok_pins.rs`.

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_forged_entry_record_with_over_strings_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_entry_record()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_forged_branch_record_with_over_strings_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_branch_record()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn a_forged_entry_record_with_a_body_past_the_ceiling_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_entry_record()
            .value(Value::OverCeiling)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn a_forged_branch_record_with_a_body_past_the_ceiling_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_branch_record()
            .value(Value::OverCeiling)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

// ---- Across-function outcome classes 2 and 3 (class 1 — the once-panicking const
// defect — is pinned above): the three classes now all resolve to the reference,
// pinning "all of step 1 precedes all policy" from three different starting laws.

/// Class 2, verdict preserved through the restructure: the earlier function's
/// bad jump draws its typed reference — now from the hoisted coherence walk,
/// which precedes every policy candidate — exactly as it did positionally.
#[test]
fn a_bad_jump_in_the_earlier_function_draws_its_reference_before_the_later_code_too_long() {
    assert_eq!(
        Fixture::clean()
            .code(Code::BadJump)
            .with_extra_function(Code::OverCodeBytes)
            .encode(),
        Err(ImageBuildError::InvalidReference("jump target")),
    );
}

/// Class 3, flipped under the sanctioned checked-conversion class
/// (`call target`) and citing the pre-restructure pin this test carried: the
/// once-unchecked `Call` ordinal is now a hoisted coherence check, so it
/// decides before the later function's CodeBytes.
#[test]
fn an_unchecked_call_in_the_earlier_function_draws_the_call_target_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::Call(u16::MAX)])
            .with_extra_function(Code::OverCodeBytes)
            .encode(),
        Err(ImageBuildError::InvalidReference("call target")),
    );
}

// ---- The complete coarse F-E matrix: every remaining
// boundary × {Call, export target, test target, type table} cell, pinned literally.
// The hoisted reference wins in every cell; each pin cites the policy verdict it
// flipped.

/// Flipped under the sanctioned checked-conversion class (`call target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_call_target_with_over_strings_draws_the_call_target_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::Call(u16::MAX)])
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("call target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`export target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_export_target_with_over_strings_draws_the_export_target_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_export_target()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("export target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`test target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_bad_test_entry_target_with_over_strings_draws_the_test_target_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_test_entry_target()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("test target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`call target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn a_bad_call_target_with_over_test_entries_draws_the_call_target_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::Call(u16::MAX)])
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("call target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`export target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn a_bad_export_target_with_over_test_entries_draws_the_export_target_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_export_target()
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("export target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`test target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn a_bad_test_entry_target_with_over_test_entries_draws_the_test_target_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_test_entry_target()
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("test target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn a_bad_type_ordinal_with_over_test_entries_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_extra_instrs(vec![Instr::RecordNew(TypeId::from_index(u16::MAX))])
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`export target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn a_bad_export_target_with_over_code_bytes_draws_the_export_target_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_forged_export_target()
            .encode(),
        Err(ImageBuildError::InvalidReference("export target")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn a_bad_type_ordinal_with_over_code_bytes_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_extra_instrs(vec![Instr::RecordNew(TypeId::from_index(u16::MAX))])
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`test target`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the whole-image ceiling.
#[test]
fn a_bad_test_entry_target_with_a_body_past_the_ceiling_draws_the_test_target_reference() {
    assert_eq!(
        Fixture::clean()
            .value(Value::OverCeiling)
            .with_forged_test_entry_target()
            .encode(),
        Err(ImageBuildError::InvalidReference("test target")),
    );
}

// ---- The DURABLE-class representative rows the derivation law names: the root
// entry record's remaining boundary cells.

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the last policy cap.
#[test]
fn a_forged_entry_record_with_over_test_entries_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_entry_record()
            .policy(Overflow::TestEntries)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn a_forged_entry_record_with_over_code_bytes_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_forged_entry_record()
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`collection type`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_dangling_traversal_list_type_with_over_strings_draws_the_collection_type_reference() {
    assert_eq!(
        Fixture::clean()
            .with_dangling_traversal()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`collection type`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_dangling_index_scan_list_type_with_over_strings_draws_the_collection_type_reference() {
    assert_eq!(
        Fixture::clean()
            .with_dangling_index_scan()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("collection type")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_forged_param_type_with_over_strings_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .with_forged_param_type()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`type table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the CodeBytes policy candidate.
#[test]
fn a_forged_param_type_with_over_code_bytes_draws_the_type_table_reference() {
    assert_eq!(
        Fixture::clean()
            .code(Code::OverCodeBytes)
            .with_forged_param_type()
            .encode(),
        Err(ImageBuildError::InvalidReference("type table")),
    );
}

/// Flipped under the sanctioned checked-conversion class (`export table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the first policy cap.
#[test]
fn a_duplicate_export_target_with_over_strings_draws_the_export_table_relation() {
    assert_eq!(
        Fixture::clean()
            .with_duplicate_export()
            .policy(Overflow::Strings)
            .encode(),
        Err(ImageBuildError::InvalidReference("export table")),
    );
}

// ---- The measured ceiling's own cells: a draft every coherence item and
// every cap admits, refused only by the measured whole-image ceiling — decided at
// measurement, envelope and frames included, before any section is assembled.

/// The final-only overage keeps its ImageTooLarge verdict through the
/// sanctioned measurement-first change: the whole-image ceiling is now decided
/// by the measured plan — envelope and frames included — before any section is
/// assembled, so the same draft draws the same result earlier.
#[test]
fn a_final_only_overage_alone_draws_the_image_ceiling() {
    assert_eq!(
        Fixture::clean().with_final_overage().encode(),
        Err(ImageBuildError::ImageTooLarge),
    );
}

/// Flipped under the sanctioned checked-conversion class (`export table`), citing the
/// pre-restructure pin this test carried: the hoisted coherence check decides
/// before the final whole-image ceiling.
#[test]
fn a_duplicate_export_target_with_a_final_only_overage_draws_the_export_table_relation() {
    assert_eq!(
        Fixture::clean()
            .with_final_overage()
            .with_duplicate_export()
            .encode(),
        Err(ImageBuildError::InvalidReference("export table")),
    );
}

/// Flipped under the sanctioned panic-to-typed conversion, citing the
/// pre-restructure panic pin this test carried: the SPANS raw offset lookup is now
/// the hoisted `InvalidReference("span instruction")` check, which — like every
/// coherence item — decides before the measured whole-image ceiling.
#[test]
fn an_out_of_range_span_with_a_final_only_overage_draws_the_span_reference() {
    assert_eq!(
        Fixture::clean()
            .with_final_overage()
            .with_bad_span()
            .encode(),
        Err(ImageBuildError::InvalidReference("span instruction")),
    );
}
