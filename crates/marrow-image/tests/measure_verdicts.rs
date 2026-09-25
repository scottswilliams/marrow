//! The measure core's verdict order: which result a draft carrying several faults draws.
//!
//! One law governs every row of [`CASES`]: each coherence item decides before every
//! policy candidate, in the stable partition's emission-order sequence; the policy
//! candidates then decide in their candidate order (the eleven aggregate caps, then
//! per-function CodeBytes); and the measured whole-image ceiling decides last, envelope
//! and frames included, before any section is assembled. Counting rather than building
//! is what lets a draft inside every declared bound be refused at all: a durable field's
//! value is spelled on the wire as its full expansion.

use marrow_image::bounds::{
    MAX_CODE_BYTES, MAX_COLLECTIONS, MAX_CONSTS, MAX_DURABLE_VALUE_DEPTH, MAX_ENUMS, MAX_EXPORTS,
    MAX_FUNCTIONS, MAX_KEY_COLUMNS, MAX_LOCALS, MAX_RECORD_FIELDS, MAX_ROOTS, MAX_SITES,
    MAX_STRING_BYTES, MAX_STRINGS, MAX_STRUCT_LEAVES, MAX_TEST_ENTRIES, MAX_TYPES, MAX_VARIANTS,
};
use marrow_image::{
    CollTypeId, CollectionTypeDef, ConstId, DeclarationMemberDef, DeclarationMemberShape,
    DraftStateError, DraftTxn, DurableIndexComponent, DurableIndexShape, EnumId, EnumTypeDef,
    ExportId, FieldDef, FuncId, FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr,
    KeyColumn, LedgerIdBytes, RecordTypeDef, ReferenceKind, RootId, Scalar, SemanticTarget,
    SpanEntry, StrId, TypeId, ValueShapeLeaf, ValueShapeNodeId, VariantDef,
};
use marrow_test_support::admitted_plan;

use marrow_test_support::ledger_ids::{
    APPLICATION_ID, FIELD_ID, PLACEMENT_ID, PRODUCT_ID, seeded_id,
};

use marrow_test_support::fixture_graph::{
    admit_root, declare_product, empty_record, unit_function,
};

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

/// A type reference naming a TYPES row no fixture declares: the coherence pass must
/// refuse the raw table ordinal before emission.
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

/// One resource-policy-class overflow, applied to an otherwise complete draft. Each
/// variant drives exactly one aggregate cap over its bound without touching any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overflow {
    /// One distinct interned string past `MAX_STRINGS`.
    Strings,
    /// One interned string one byte past `MAX_STRING_BYTES`.
    StringBytes,
    /// One stored struct leaf name one byte past `MAX_STRING_BYTES`, in a value shape no
    /// field references: the string bound covers every leaf name the arena holds.
    LeafNameBytes,
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

/// One defect a fixture draft carries. The faults are independent, so a case states
/// exactly the ones it wants and the encode result names which the producer reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// A durable value of ten nesting levels of four leaves each: 11 value-graph nodes
    /// whose `4^10`-leaf expansion is about 2 MiB of wire, four times what any image
    /// may be.
    BodyPastCeiling,
    /// A single-leaf struct nest one level past `MAX_DURABLE_VALUE_DEPTH`: a two-byte
    /// expansion whose depth the declaration-graph walk refuses.
    OverDeepValue,
    /// A value node minted by a larger arena in a different draft, out of range for
    /// this draft's own arena.
    ForgedValueNode,
    /// A `main` body past `MAX_CODE_BYTES`, so the function section refuses it.
    OverCodeBytes,
    /// Every `ConstLoad` of `main` names a constant no row answers.
    BadConst,
    /// `main` jumps to a target naming no instruction.
    BadJump,
    /// A second function after `main`, itself past `MAX_CODE_BYTES`.
    LaterFunctionOverCodeBytes,
    /// A second function after `main`, itself carrying an unanswered `ConstLoad`.
    LaterFunctionBadConst,
    /// One local slot past `MAX_LOCALS`: refused near the end of the fixed-width
    /// coherence subsequence.
    OverLocals,
    /// One `int` param but zero local slots, so the frame cannot hold its own params:
    /// refused as the last fixed-width coherence result.
    LocalsBelowParams,
    /// One resource-policy aggregate driven over its cap.
    Policy(Overflow),
    /// A key tuple one column past `MAX_KEY_COLUMNS`: an occurrence-level fault the
    /// coherence pass reports after the declaration graph and before CodeBytes.
    OverWideKey,
    /// Drop the application identity a non-empty durable graph is anchored by.
    WithoutApplication,
    /// A record type one field past `MAX_RECORD_FIELDS`: the per-record width fault,
    /// reported early in the coherence pass.
    OverWideRecord,
    /// The base Product identity declared a second time with a different member graph:
    /// two declarations wearing one identity, recorded at declaration and refused by
    /// the coherence pass as the Product claim conflict.
    ConflictingProduct,
    /// A span entry whose `instr_index` names no instruction of `main`.
    BadSpan,
    /// An enum DEFINITION one variant past `MAX_VARIANTS`: the definition-site
    /// `TooManyVariants`, decided among the fixed table bounds.
    WideEnumDefinition,
    /// A value-DAG enum node one member past `MAX_VARIANTS`, which the append surface
    /// refuses outright: the fixture asserts the typed refusal and the draft keeps
    /// nothing of it.
    WideEnumValueNode,
    /// A declaration BRANCH member one key column past `MAX_KEY_COLUMNS`: the
    /// declaration-site `TooManyKeyColumns`, decided in the declaration-graph walk
    /// before the occurrence loop.
    BranchWideKey,
    /// The base record's TYPES-table name is a pool id no string answers.
    ForgedRecordName,
    /// `main`'s FUNCTIONS-table name is a pool id no string answers.
    ForgedFunctionName,
    /// An ENUMS-table definition whose name is a pool id no string answers.
    ForgedEnumName,
    /// A declaration BRANCH member (otherwise valid) whose DURABLE-section name is a
    /// pool id no string answers.
    ForgedBranchName,
    /// A TEST-ENTRY row whose name is a pool id no string answers.
    ForgedTestEntryName,
    /// An EXPORTS row whose function index no row answers.
    ForgedExportTarget,
    /// A TEST-ENTRY row (valid name) whose function index no row answers.
    ForgedTestEntryTarget,
    /// The base record carries one field whose `ImageType` names no TYPES row.
    ForgedFieldType,
    /// An ENUMS definition whose variant payload leaf names no TYPES row.
    ForgedEnumPayloadType,
    /// A COLLTYPES row whose element `ImageType` names no TYPES row.
    ForgedCollectionElem,
    /// The Product declaration's root entry record names no TYPES row (`TypeId` is a
    /// raw newtype with a public `from_index`).
    ForgedEntryRecord,
    /// A declaration BRANCH member (otherwise valid) whose entry record names no
    /// TYPES row.
    ForgedBranchRecord,
    /// `main`'s one parameter `ImageType` names no TYPES row.
    ForgedParamType,
    /// `main` carries a bounded traversal over a live whole-payload site whose
    /// `list_ty` names no COLLTYPES row.
    DanglingTraversal,
    /// The root gains a nonunique managed index, and `main` carries an index scan over
    /// its live site whose `list_ty` names no COLLTYPES row.
    DanglingIndexScan,
    /// A second export naming `main`'s function index: a duplicate export target.
    DuplicateExport,
    /// A string pool inside both of its caps whose bytes alone exceed the whole-image
    /// ceiling: every section fits its own bound and the durable body alone fits, so
    /// only the measured whole-image total can refuse the draft.
    FinalOverage,
    /// `main` calls a function index no row answers.
    BadCallTarget,
    /// `main` constructs a record whose TYPES ordinal no row answers.
    BadRecordNewOrdinal,
    /// `main` constructs a list whose COLLTYPES ordinal no row answers.
    BadListNewOrdinal,
    /// `main` constructs an enum whose ENUMS ordinal no row answers.
    BadEnumConstructOrdinal,
    /// `main` loads a vacancy whose embedded TYPES ordinal no row answers.
    BadVacantLoadType,
    /// `main` mints an identity over a ROOTS ordinal no row answers.
    BadMakeIdentityRoot,
}

/// Ledger-id tags partitioning this fixture's seeded id space by role.
mod tag {
    pub const EXTRA_ROOT_KEY: u8 = 0x21;
    pub const EXTRA_ROOT_PLACEMENT: u8 = 0x22;
    pub const KEY_COLUMN: u8 = 0x30;
    pub const WIDE_PRODUCT: u8 = 0x31;
    pub const WIDE_ROOT_KEY: u8 = 0x34;
    pub const WIDE_ROOT_PLACEMENT: u8 = 0x35;
    pub const WIDE_FIELD: u8 = 0x36;
    pub const CONFLICTING_FIELD: u8 = 0x51;
    pub const ENUM_NODE: u8 = 0x60;
    pub const ENUM_MEMBER: u8 = 0x61;
    pub const BRANCH_PLACEMENT: u8 = 0x71;
    pub const BRANCH_KEY: u8 = 0x72;
    pub const INDEX: u8 = 0x77;
}

/// One fixture: a keyed root over a Product with one durable field, plus a `main`,
/// carrying exactly the faults it was given.
struct Fixture {
    faults: Vec<Fault>,
}

impl Fixture {
    /// A complete draft with no fault at all.
    fn clean() -> Self {
        Self { faults: Vec::new() }
    }

    fn fault(mut self, fault: Fault) -> Self {
        self.faults.push(fault);
        self
    }

    fn faults(mut self, faults: &[Fault]) -> Self {
        self.faults.extend_from_slice(faults);
        self
    }

    fn has(&self, fault: Fault) -> bool {
        self.faults.contains(&fault)
    }

    fn policy(&self) -> Option<Overflow> {
        self.faults.iter().find_map(|fault| match fault {
            Fault::Policy(overflow) => Some(*overflow),
            _ => None,
        })
    }

    /// The durable field's value shape.
    fn value(&self, draft: &mut DraftTxn<'_>) -> ValueShapeNodeId {
        let int = draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints");
        if self.has(Fault::BodyPastCeiling) {
            let mut level = int;
            for _ in 0..10 {
                level = draft
                    .value_struct(vec![ValueShapeLeaf::new("v", level); 4])
                    .expect("a within-bounds shape appends");
            }
            return level;
        }
        if self.has(Fault::OverDeepValue) {
            let mut level = int;
            for _ in 0..MAX_DURABLE_VALUE_DEPTH {
                level = draft
                    .value_struct(vec![ValueShapeLeaf::new("v", level)])
                    .expect("a within-bounds shape appends");
            }
            return level;
        }
        if self.has(Fault::ForgedValueNode) {
            // Index 2 in a three-node arena; the fixture draft's arena holds one.
            let mut other_owner = ImageDraft::new();
            let mut other = other_owner.begin_transaction();
            other
                .value_scalar(Scalar::Int)
                .expect("the test arena mints");
            other
                .value_scalar(Scalar::Bool)
                .expect("the test arena mints");
            return other
                .value_scalar(Scalar::Text)
                .expect("the test arena mints");
        }
        int
    }

    /// Instructions appended to `main`'s body after its shape.
    fn extra_instrs(&self) -> Vec<Instr> {
        self.faults
            .iter()
            .filter_map(|fault| match fault {
                Fault::BadCallTarget => Some(Instr::Call(OUT_OF_RANGE)),
                Fault::BadRecordNewOrdinal => {
                    Some(Instr::RecordNew(TypeId::from_index(OUT_OF_RANGE)))
                }
                Fault::BadListNewOrdinal => {
                    Some(Instr::ListNew(CollTypeId::from_index(OUT_OF_RANGE)))
                }
                Fault::BadEnumConstructOrdinal => Some(Instr::EnumConstruct {
                    enum_idx: EnumId::from_index(OUT_OF_RANGE),
                    variant: 0,
                }),
                Fault::BadVacantLoadType => Some(Instr::VacantLoad(FORGED_OPT_TYPE)),
                Fault::BadMakeIdentityRoot => Some(Instr::MakeIdentity {
                    root: RootId::from_index(OUT_OF_RANGE),
                    cols: 0,
                }),
                _ => None,
            })
            .collect()
    }

    /// The base record the Product's entry names.
    fn record(&self, draft: &mut DraftTxn<'_>) -> TypeId {
        let type_name = if self.has(Fault::ForgedRecordName) {
            StrId::from_index(FORGED_RECORD_NAME)
        } else {
            draft.intern_string("R").expect("a within-domain mint")
        };
        let fields = if self.has(Fault::OverWideRecord) {
            let field_name = draft.intern_string("wide").expect("a within-domain mint");
            vec![
                FieldDef {
                    name: field_name,
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                };
                MAX_RECORD_FIELDS + 1
            ]
        } else if self.has(Fault::ForgedFieldType) {
            let field_name = draft.intern_string("f").expect("a within-domain mint");
            vec![FieldDef {
                name: field_name,
                ty: FORGED_TYPE,
                required: true,
            }]
        } else {
            Vec::new()
        };
        draft
            .add_record_type(RecordTypeDef {
                name: type_name,
                fields,
            })
            .expect("a within-domain mint")
    }

    /// The ENUMS rows appended before the declaration.
    fn enum_prelude(&self, draft: &mut DraftTxn<'_>) {
        if self.has(Fault::ForgedEnumName) {
            draft
                .add_enum_type(EnumTypeDef {
                    name: StrId::from_index(FORGED_ENUM_NAME),
                    variants: Vec::new(),
                })
                .expect("a within-domain mint");
        }
        if self.has(Fault::WideEnumDefinition) {
            let name = draft
                .intern_string("WideEnum")
                .expect("a within-domain mint");
            let variant_name = draft.intern_string("v").expect("a within-domain mint");
            draft
                .add_enum_type(EnumTypeDef {
                    name,
                    variants: vec![
                        VariantDef {
                            name: variant_name,
                            category: false,
                            payload: Vec::new(),
                        };
                        MAX_VARIANTS + 1
                    ],
                })
                .expect("a within-domain mint");
        }
        if self.has(Fault::WideEnumValueNode) {
            // The append surface is where an over-wide value node is refused: nothing
            // enters the arena, whether or not any field would have referenced it.
            let members = (0..=MAX_VARIANTS)
                .map(|index| (seeded_id(tag::ENUM_MEMBER, index), Vec::new()))
                .collect();
            assert_eq!(
                draft.value_enum(seeded_id(tag::ENUM_NODE, 0), members),
                Err(DraftStateError::CarrierDomain),
            );
        }
    }

    /// The Product's declared member graph.
    fn members(
        &self,
        draft: &mut DraftTxn<'_>,
        record: TypeId,
        value: ValueShapeNodeId,
    ) -> Vec<DeclarationMemberDef> {
        let mut members = vec![DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: LedgerIdBytes::from_bytes(FIELD_ID),
                required: true,
                value,
            },
        }];
        if self.has(Fault::BranchWideKey) {
            let branch_name = draft.intern_string("b").expect("a within-domain mint");
            members.push(DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Branch {
                    placement: seeded_id(tag::BRANCH_PLACEMENT, 0),
                    name: branch_name,
                    record,
                    keys: (0..=MAX_KEY_COLUMNS)
                        .map(|column| KeyColumn {
                            scalar: Scalar::Int,
                            id: seeded_id(tag::BRANCH_KEY, column),
                        })
                        .collect(),
                },
            });
        }
        if self.has(Fault::ForgedBranchName) {
            members.push(DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Branch {
                    placement: seeded_id(tag::BRANCH_PLACEMENT, 1),
                    name: StrId::from_index(FORGED_BRANCH_NAME),
                    record,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: seeded_id(tag::BRANCH_KEY, 1),
                    }],
                },
            });
        }
        if self.has(Fault::ForgedBranchRecord) {
            let branch_name = draft.intern_string("fb").expect("a within-domain mint");
            members.push(DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Branch {
                    placement: seeded_id(tag::BRANCH_PLACEMENT, 2),
                    name: branch_name,
                    record: TypeId::from_index(OUT_OF_RANGE),
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: seeded_id(tag::BRANCH_KEY, 2),
                    }],
                },
            });
        }
        members
    }

    /// The one keyed root occurrence over the declared Product.
    fn admit_root(&self, draft: &mut DraftTxn<'_>) -> marrow_image::AdmittedRoot {
        let indexes = if self.has(Fault::DanglingIndexScan) {
            vec![DurableIndexShape {
                id: seeded_id(tag::INDEX, 0),
                unique: false,
                components: vec![
                    DurableIndexComponent::Field(LedgerIdBytes::from_bytes(FIELD_ID)),
                    DurableIndexComponent::Key(seeded_id(tag::KEY_COLUMN, 0)),
                ],
            }]
        } else {
            Vec::new()
        };
        let key_columns = if self.has(Fault::OverWideKey) {
            MAX_KEY_COLUMNS + 1
        } else {
            1
        };
        admit_root(
            draft,
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            "r",
            LedgerIdBytes::from_bytes(PLACEMENT_ID),
            (0..key_columns)
                .map(|column| KeyColumn {
                    scalar: Scalar::Int,
                    id: seeded_id(tag::KEY_COLUMN, column),
                })
                .collect(),
            indexes,
        )
    }

    fn encode(self) -> Result<(), ImageBuildError> {
        let mut draft_owner = ImageDraft::new();
        let mut draft = draft_owner.begin_transaction();
        let value = self.value(&mut draft);
        let record = self.record(&mut draft);
        if !self.has(Fault::WithoutApplication) {
            draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        }
        self.enum_prelude(&mut draft);
        let members = self.members(&mut draft, record, value);
        let entry_record = if self.has(Fault::ForgedEntryRecord) {
            TypeId::from_index(OUT_OF_RANGE)
        } else {
            record
        };
        declare_product(
            &mut draft,
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            entry_record,
            members,
        );
        if self.has(Fault::ConflictingProduct) {
            // The later declaration still resolves to the bound row; the conflict is
            // recorded and reported by the encoder, not refused here.
            declare_product(
                &mut draft,
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                record,
                vec![DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: seeded_id(tag::CONFLICTING_FIELD, 0),
                        required: true,
                        value,
                    },
                }],
            );
        }
        let root = self.admit_root(&mut draft);
        let src = draft
            .intern_string("src/main.mw")
            .expect("a within-domain mint");
        let main_name = if self.has(Fault::ForgedFunctionName) {
            StrId::from_index(OUT_OF_RANGE)
        } else {
            draft.intern_string("main").expect("a within-domain mint")
        };
        let zero = draft.intern_int(0).expect("a within-domain mint");
        let mut code = body(
            self.has(Fault::OverCodeBytes),
            self.has(Fault::BadConst),
            self.has(Fault::BadJump),
            zero,
        );
        code.extend(self.extra_instrs());
        if self.has(Fault::DanglingTraversal) {
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
                list_ty: CollTypeId::from_index(OUT_OF_RANGE),
            });
        }
        if self.has(Fault::DanglingIndexScan) {
            let scan_path = root.index_paths()[0].clone();
            let handle = draft
                .bind_occurrence_site(root.occurrence(), &scan_path, SemanticTarget::IndexScan)
                .expect("a managed index");
            let site = draft.request_site(&handle).expect("a live demand");
            code.push(Instr::DurIndexScan {
                site,
                limit: 2,
                from: false,
                list_ty: CollTypeId::from_index(OUT_OF_RANGE),
            });
        }
        let (params, local_count) = if self.has(Fault::ForgedParamType) {
            (vec![FORGED_TYPE], 1)
        } else if self.has(Fault::LocalsBelowParams) {
            (vec![ImageType::scalar(Scalar::Int)], 0)
        } else if self.has(Fault::OverLocals) {
            (Vec::new(), (MAX_LOCALS + 1) as u16)
        } else {
            (Vec::new(), 0)
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
                    instr_index: if self.has(Fault::BadSpan) {
                        u32::MAX
                    } else {
                        0
                    },
                    line: 1,
                    column: 1,
                }],
                code,
            })
            .expect("every site operand is live");
        draft.add_export(ExportId::of_local("", "main"), main);
        if self.has(Fault::DuplicateExport) {
            draft.add_export(ExportId::of_local("", "dup"), main);
        }
        if self.has(Fault::LaterFunctionOverCodeBytes) || self.has(Fault::LaterFunctionBadConst) {
            let aux_name = draft.intern_string("aux").expect("a within-domain mint");
            draft
                .add_function(FunctionDef {
                    name: aux_name,
                    source: src,
                    params: Vec::new(),
                    ret: ImageType::scalar(Scalar::Int),
                    local_count: 0,
                    spans: Vec::new(),
                    code: body(
                        self.has(Fault::LaterFunctionOverCodeBytes),
                        self.has(Fault::LaterFunctionBadConst),
                        false,
                        zero,
                    ),
                })
                .expect("every site operand is live");
        }
        self.tail_tables(&mut draft, main);
        if let Some(policy) = self.policy() {
            apply_policy(policy, &mut draft);
        }
        draft.encode().map(|_| ())
    }

    /// The rows appended after `main`: the forged test-entry and export targets, the
    /// late ENUMS and COLLTYPES references, and the string pool that alone crosses the
    /// measured ceiling.
    fn tail_tables(&self, draft: &mut DraftTxn<'_>, main: FuncId) {
        if self.has(Fault::ForgedTestEntryName) {
            draft.add_test_entry(StrId::from_index(FORGED_TEST_ENTRY_NAME), main);
        }
        if self.has(Fault::FinalOverage) {
            // 132 distinct strings of 4,004 bytes: 528,528 string bytes — every string
            // inside MAX_STRING_BYTES, the count far inside MAX_STRINGS, and the sum
            // past MAX_IMAGE_BYTES (524,288) on its own.
            for index in 0..132 {
                draft
                    .intern_string(&format!("{:04}{}", index, "x".repeat(4000)))
                    .expect("a within-domain mint");
            }
        }
        if self.has(Fault::ForgedTestEntryTarget) {
            let entry_name = draft.intern_string("tt").expect("a within-domain mint");
            draft.add_test_entry(entry_name, forged_func_id());
        }
        if self.has(Fault::ForgedExportTarget) {
            draft.add_export(ExportId::of_local("", "ghost"), forged_func_id());
        }
        if self.has(Fault::ForgedEnumPayloadType) {
            let name = draft.intern_string("P").expect("a within-domain mint");
            let variant_name = draft.intern_string("pv").expect("a within-domain mint");
            draft
                .add_enum_type(EnumTypeDef {
                    name,
                    variants: vec![VariantDef {
                        name: variant_name,
                        category: false,
                        payload: vec![FORGED_TYPE],
                    }],
                })
                .expect("a within-domain mint");
        }
        if self.has(Fault::ForgedCollectionElem) {
            draft
                .add_collection_type(CollectionTypeDef::List { elem: FORGED_TYPE })
                .expect("a within-domain mint");
        }
    }
}

/// One function body over the fixture's zero constant.
///
/// `ConstLoad` is three bytes, so `MAX_CODE_BYTES / 2` loads sit comfortably past the
/// byte limit while staying well inside the instruction count the draft admits.
fn body(over_code_bytes: bool, bad_const: bool, bad_jump: bool, zero: ConstId) -> Vec<Instr> {
    let load = if bad_const {
        Instr::ConstLoad(ConstId::from_index(OUT_OF_RANGE))
    } else {
        Instr::ConstLoad(zero)
    };
    let loads = if over_code_bytes {
        MAX_CODE_BYTES / 2
    } else {
        1
    };
    let mut code = vec![load; loads];
    if bad_jump {
        code.push(Instr::Jump(u32::from(OUT_OF_RANGE)));
    }
    code.push(Instr::Return);
    code
}

/// A function index no fixture's final table can answer — `u16::MAX`, minted by a
/// draft that fills the whole index space. A `FuncId` is a table position, not a
/// capability bound to its draft, and the ordinal must sit beyond every table a policy
/// overflow can grow: an ordinal of 1 would be healed into validity by the fixtures
/// that append real functions (the TestEntries and Exports overflows).
fn forged_func_id() -> FuncId {
    let mut other_owner = ImageDraft::new();
    let mut other = other_owner.begin_transaction();
    let src = other.intern_string("s").expect("a within-domain mint");
    let name = other.intern_string("f").expect("a within-domain mint");
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
fn apply_policy(policy: Overflow, draft: &mut DraftTxn<'_>) {
    match policy {
        // The base draft interns a handful of strings, so a full extra pool is over.
        Overflow::Strings => {
            for index in 0..MAX_STRINGS {
                draft
                    .intern_string(&format!("s{index}"))
                    .expect("a within-domain mint");
            }
        }
        Overflow::StringBytes => {
            draft
                .intern_string(&"x".repeat(MAX_STRING_BYTES + 1))
                .expect("a within-domain mint");
        }
        Overflow::LeafNameBytes => {
            let int = draft
                .value_scalar(Scalar::Int)
                .expect("the test arena mints");
            draft
                .value_struct(vec![ValueShapeLeaf::new(
                    "x".repeat(MAX_STRING_BYTES + 1),
                    int,
                )])
                .expect("the arena itself does not bound a name");
        }
        // Zero is already interned by the base draft, so the pool ends one past the cap.
        Overflow::Consts => {
            for value in 1..=MAX_CONSTS as i64 {
                draft.intern_int(value).expect("a within-domain mint");
            }
        }
        Overflow::Types => {
            let name = draft.intern_string("T").expect("a within-domain mint");
            for _ in 0..MAX_TYPES {
                draft
                    .add_record_type(RecordTypeDef {
                        name,
                        fields: Vec::new(),
                    })
                    .expect("a within-domain mint");
            }
        }
        Overflow::Enums => {
            let name = draft.intern_string("E").expect("a within-domain mint");
            for _ in 0..=MAX_ENUMS {
                draft
                    .add_enum_type(EnumTypeDef {
                        name,
                        variants: Vec::new(),
                    })
                    .expect("a within-domain mint");
            }
        }
        Overflow::Collections => {
            for _ in 0..=MAX_COLLECTIONS {
                draft
                    .add_collection_type(CollectionTypeDef::List {
                        elem: ImageType::scalar(Scalar::Int),
                    })
                    .expect("a within-domain mint");
            }
        }
        // The admitted plan permits exactly one occurrence past the root bound, so the
        // graph is complete and the encoder — not the intake — reports the cap.
        Overflow::Roots => {
            for index in 0..MAX_ROOTS {
                admit_root(
                    draft,
                    &admitted_plan(),
                    LedgerIdBytes::from_bytes(PRODUCT_ID),
                    &format!("extra{index}"),
                    seeded_id(tag::EXTRA_ROOT_PLACEMENT, index),
                    vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: seeded_id(tag::EXTRA_ROOT_KEY, index),
                    }],
                    Vec::new(),
                );
            }
        }
        Overflow::Sites => overflow_sites(draft),
        Overflow::Functions => {
            let name = draft.intern_string("f").expect("a within-domain mint");
            let src = draft
                .intern_string("src/extra.mw")
                .expect("a within-domain mint");
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
            let src = draft
                .intern_string("src/extra.mw")
                .expect("a within-domain mint");
            let zero = draft.intern_int(0).expect("a within-domain mint");
            for index in 0..MAX_EXPORTS {
                let name = draft
                    .intern_string(&format!("extra{index}"))
                    .expect("a within-domain mint");
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
            for index in 0..=MAX_TEST_ENTRIES {
                let label = format!("t{index}");
                let func = unit_function(draft, &label, vec![Instr::Return]);
                let name = draft.intern_string(&label).expect("a within-domain mint");
                draft.add_test_entry(name, func);
            }
        }
    }
}

/// A second Product as wide as the site table itself: demanding every field leaf fills
/// the table, and the root's whole-payload demand is the crossing.
fn overflow_sites(draft: &mut DraftTxn<'_>) {
    let value = draft
        .value_scalar(Scalar::Int)
        .expect("the test arena mints");
    let entry = empty_record(draft, "S");
    let members = (0..MAX_SITES)
        .map(|index| DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: seeded_id(tag::WIDE_FIELD, index),
                required: true,
                value,
            },
        })
        .collect();
    let fields = declare_product(
        draft,
        &admitted_plan(),
        seeded_id(tag::WIDE_PRODUCT, 0),
        entry,
        members,
    );
    let root = admit_root(
        draft,
        &admitted_plan(),
        seeded_id(tag::WIDE_PRODUCT, 0),
        "sites",
        seeded_id(tag::WIDE_ROOT_PLACEMENT, 0),
        vec![KeyColumn {
            scalar: Scalar::Int,
            id: seeded_id(tag::WIDE_ROOT_KEY, 0),
        }],
        Vec::new(),
    );
    for member in &fields {
        let handle = draft
            .bind_occurrence_site(root.occurrence(), member.path(), SemanticTarget::FieldLeaf)
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

/// Every pinned crossing: the faults one draft carries and the exact result the producer
/// reports for it. A row is the pin; the fault set is its name.
const CASES: &[(&[Fault], ImageBuildError)] = &[
    // Coherence outranks the byte-shaped results.
    (&[Fault::BodyPastCeiling], ImageBuildError::ImageTooLarge),
    (&[Fault::OverCodeBytes], ImageBuildError::CodeTooLong),
    (
        &[Fault::BodyPastCeiling, Fault::OverCodeBytes],
        ImageBuildError::CodeTooLong,
    ),
    (
        &[
            Fault::OverWideKey,
            Fault::BodyPastCeiling,
            Fault::OverCodeBytes,
        ],
        ImageBuildError::TooManyKeyColumns,
    ),
    (
        &[
            Fault::WithoutApplication,
            Fault::BodyPastCeiling,
            Fault::OverCodeBytes,
        ],
        ImageBuildError::InvalidReference(ReferenceKind::ApplicationIdentity),
    ),
    (
        &[Fault::WithoutApplication, Fault::OverWideKey],
        ImageBuildError::TooManyKeyColumns,
    ),
    // Each policy cap alone, and crossed with the invariant that outranks it.
    (
        &[Fault::Policy(Overflow::Strings)],
        ImageBuildError::TooManyStrings,
    ),
    (
        &[Fault::Policy(Overflow::StringBytes)],
        ImageBuildError::StringTooLong,
    ),
    (
        &[Fault::Policy(Overflow::LeafNameBytes)],
        ImageBuildError::StringTooLong,
    ),
    (
        &[Fault::Policy(Overflow::LeafNameBytes), Fault::OverWideKey],
        ImageBuildError::TooManyKeyColumns,
    ),
    (
        &[Fault::Policy(Overflow::Types)],
        ImageBuildError::TooManyTypes,
    ),
    (
        &[Fault::Policy(Overflow::Functions)],
        ImageBuildError::TooManyFunctions,
    ),
    (
        &[Fault::Policy(Overflow::Consts), Fault::OverWideKey],
        ImageBuildError::TooManyKeyColumns,
    ),
    (
        &[Fault::Policy(Overflow::Enums), Fault::OverDeepValue],
        ImageBuildError::DurableValueTooDeep,
    ),
    (
        &[Fault::Policy(Overflow::Collections), Fault::OverWideKey],
        ImageBuildError::TooManyKeyColumns,
    ),
    (
        &[Fault::Policy(Overflow::Roots), Fault::WithoutApplication],
        ImageBuildError::InvalidReference(ReferenceKind::ApplicationIdentity),
    ),
    (
        &[Fault::Policy(Overflow::Roots), Fault::OverWideKey],
        ImageBuildError::TooManyKeyColumns,
    ),
    (
        &[Fault::Policy(Overflow::Sites), Fault::OverLocals],
        ImageBuildError::TooManyLocals,
    ),
    (
        &[Fault::Policy(Overflow::Functions), Fault::OverLocals],
        ImageBuildError::TooManyLocals,
    ),
    (
        &[Fault::Policy(Overflow::Exports), Fault::LocalsBelowParams],
        ImageBuildError::LocalCountBelowParams,
    ),
    (
        &[Fault::Policy(Overflow::TestEntries), Fault::OverLocals],
        ImageBuildError::TooManyLocals,
    ),
    // A policy cap still outranks the measured whole-image ceiling.
    (
        &[Fault::BodyPastCeiling, Fault::Policy(Overflow::Strings)],
        ImageBuildError::TooManyStrings,
    ),
    // Invariant × invariant: the relative order inside the coherence sequence.
    (&[Fault::OverLocals], ImageBuildError::TooManyLocals),
    (
        &[Fault::OverWideRecord, Fault::OverWideKey],
        ImageBuildError::TooManyFields,
    ),
    (
        &[Fault::ConflictingProduct, Fault::OverWideKey],
        ImageBuildError::ProductGraphConflict,
    ),
    // Each checked conversion on a caller-supplied id, alone and crossed.
    (
        &[Fault::ForgedValueNode],
        ImageBuildError::InvalidReference(ReferenceKind::ValueShape),
    ),
    (
        &[Fault::ForgedValueNode, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::ValueShape),
    ),
    (
        &[Fault::BadConst],
        ImageBuildError::InvalidReference(ReferenceKind::Constant),
    ),
    (
        &[Fault::BadConst, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::Constant),
    ),
    (
        &[Fault::BadSpan],
        ImageBuildError::InvalidReference(ReferenceKind::SpanInstruction),
    ),
    (
        &[Fault::BadSpan, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::SpanInstruction),
    ),
    (
        &[Fault::BadJump],
        ImageBuildError::InvalidReference(ReferenceKind::JumpTarget),
    ),
    (
        &[Fault::BadJump, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::JumpTarget),
    ),
    // One error variant, two decision sites: the declaration branch decides before the
    // root-occurrence loop, and each site draws the variant alone.
    (&[Fault::BranchWideKey], ImageBuildError::TooManyKeyColumns),
    (&[Fault::OverWideKey], ImageBuildError::TooManyKeyColumns),
    (
        &[Fault::BranchWideKey, Fault::OverWideKey],
        ImageBuildError::TooManyKeyColumns,
    ),
    // Each hoisted reference family crossed with the first policy cap in candidate
    // order (Strings) and the last (TestEntries).
    (
        &[Fault::ForgedRecordName, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::RecordName),
    ),
    (
        &[Fault::BadConst, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::Constant),
    ),
    (
        &[Fault::BadJump, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::JumpTarget),
    ),
    (
        &[Fault::BadSpan, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::SpanInstruction),
    ),
    (
        &[
            Fault::ForgedRecordName,
            Fault::Policy(Overflow::TestEntries),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::RecordName),
    ),
    (
        &[Fault::BadConst, Fault::Policy(Overflow::TestEntries)],
        ImageBuildError::InvalidReference(ReferenceKind::Constant),
    ),
    (
        &[Fault::BadJump, Fault::Policy(Overflow::TestEntries)],
        ImageBuildError::InvalidReference(ReferenceKind::JumpTarget),
    ),
    (
        &[Fault::BadSpan, Fault::Policy(Overflow::TestEntries)],
        ImageBuildError::InvalidReference(ReferenceKind::SpanInstruction),
    ),
    // The same families crossed with the two byte-shaped results.
    (
        &[Fault::BadSpan, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::SpanInstruction),
    ),
    (
        &[Fault::ForgedFunctionName, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::FunctionName),
    ),
    (
        &[Fault::ForgedRecordName, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::RecordName),
    ),
    (
        &[Fault::ForgedEnumName, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::EnumName),
    ),
    // Two forged references in one draft, in different sections: the EARLIER site's
    // check decides, so the pair proves the coherence walk kept the emission-order
    // subsequence.
    //
    // One pair is not constructible and is recorded rather than forced: TYPES
    // record-name against a CONSTS text reference. A text constant's string id is
    // minted only by `ImageDraft::intern_text`, which interns the text itself, so no
    // public path binds a raw `StrId` to a constant.
    (
        &[Fault::ForgedFunctionName, Fault::ForgedRecordName],
        ImageBuildError::InvalidReference(ReferenceKind::FunctionName),
    ),
    (
        &[Fault::BadSpan, Fault::ForgedTestEntryName],
        ImageBuildError::InvalidReference(ReferenceKind::SpanInstruction),
    ),
    (
        &[Fault::ForgedBranchName, Fault::ForgedRecordName],
        ImageBuildError::InvalidReference(ReferenceKind::BranchName),
    ),
    // Once-unchecked table ordinals crossed with each policy boundary.
    (
        &[Fault::BadCallTarget, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::CallTarget),
    ),
    (
        &[Fault::ForgedTestEntryTarget, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::TestTarget),
    ),
    (
        &[Fault::BadCallTarget, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::CallTarget),
    ),
    (
        &[Fault::ForgedExportTarget, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::ExportTarget),
    ),
    (
        &[Fault::BadRecordNewOrdinal, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::BadRecordNewOrdinal, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::BadListNewOrdinal, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::CollectionType),
    ),
    (
        &[
            Fault::BadEnumConstructOrdinal,
            Fault::Policy(Overflow::Strings),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::EnumType),
    ),
    (
        &[Fault::BadVacantLoadType, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::BadMakeIdentityRoot, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::RootTable),
    ),
    (
        &[Fault::ForgedFieldType, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[
            Fault::ForgedEnumPayloadType,
            Fault::Policy(Overflow::Strings),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[
            Fault::ForgedCollectionElem,
            Fault::Policy(Overflow::Strings),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    // Across two functions, in both orders: all of coherence precedes all policy, so
    // the reference decides whichever function carries it.
    (
        &[Fault::BadConst, Fault::LaterFunctionOverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::Constant),
    ),
    (
        &[Fault::OverCodeBytes, Fault::LaterFunctionBadConst],
        ImageBuildError::InvalidReference(ReferenceKind::Constant),
    ),
    (
        &[Fault::BadJump, Fault::LaterFunctionOverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::JumpTarget),
    ),
    (
        &[Fault::BadCallTarget, Fault::LaterFunctionOverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::CallTarget),
    ),
    // The two DURABLE type-table ordinals: the root entry record and a branch's.
    (
        &[Fault::ForgedEntryRecord, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::ForgedBranchRecord, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::ForgedEntryRecord, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::ForgedBranchRecord, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[
            Fault::ForgedEntryRecord,
            Fault::Policy(Overflow::TestEntries),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::ForgedEntryRecord, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    // The remaining boundary × {call, export target, test target, type table} cells.
    (
        &[Fault::BadCallTarget, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::CallTarget),
    ),
    (
        &[Fault::ForgedExportTarget, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::ExportTarget),
    ),
    (
        &[
            Fault::ForgedTestEntryTarget,
            Fault::Policy(Overflow::Strings),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::TestTarget),
    ),
    (
        &[Fault::BadCallTarget, Fault::Policy(Overflow::TestEntries)],
        ImageBuildError::InvalidReference(ReferenceKind::CallTarget),
    ),
    (
        &[
            Fault::ForgedExportTarget,
            Fault::Policy(Overflow::TestEntries),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::ExportTarget),
    ),
    (
        &[
            Fault::ForgedTestEntryTarget,
            Fault::Policy(Overflow::TestEntries),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::TestTarget),
    ),
    (
        &[
            Fault::BadRecordNewOrdinal,
            Fault::Policy(Overflow::TestEntries),
        ],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::ForgedExportTarget, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::ExportTarget),
    ),
    (
        &[Fault::BadRecordNewOrdinal, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::ForgedTestEntryTarget, Fault::BodyPastCeiling],
        ImageBuildError::InvalidReference(ReferenceKind::TestTarget),
    ),
    (
        &[Fault::DanglingTraversal, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::CollectionType),
    ),
    (
        &[Fault::DanglingIndexScan, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::CollectionType),
    ),
    (
        &[Fault::ForgedParamType, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::ForgedParamType, Fault::OverCodeBytes],
        ImageBuildError::InvalidReference(ReferenceKind::TypeTable),
    ),
    (
        &[Fault::DuplicateExport, Fault::Policy(Overflow::Strings)],
        ImageBuildError::InvalidReference(ReferenceKind::ExportTable),
    ),
    // The measured ceiling's own cells: a draft every coherence item and every cap
    // admits, refused only by the measured whole-image total.
    (&[Fault::FinalOverage], ImageBuildError::ImageTooLarge),
    (
        &[Fault::FinalOverage, Fault::DuplicateExport],
        ImageBuildError::InvalidReference(ReferenceKind::ExportTable),
    ),
    (
        &[Fault::FinalOverage, Fault::BadSpan],
        ImageBuildError::InvalidReference(ReferenceKind::SpanInstruction),
    ),
];

#[test]
fn every_pinned_crossing_draws_its_verdict() {
    for (faults, expected) in CASES {
        assert_eq!(
            Fixture::clean().faults(faults).encode().as_ref().err(),
            Some(expected),
            "{faults:?}",
        );
    }
}

/// The bridge changes nothing about a clean draft.
#[test]
fn a_clean_draft_encodes() {
    assert_eq!(Fixture::clean().encode(), Ok(()));
}

/// An over-wide struct append is refused at the transaction surface, mutating nothing:
/// the over-wide arena state a crossing case would otherwise stage is unrepresentable
/// through the one mutation surface.
#[test]
fn an_over_wide_struct_append_is_refused_at_the_surface() {
    let mut owner = ImageDraft::new();
    let mut draft = owner.begin_transaction();
    let int = draft
        .value_scalar(Scalar::Int)
        .expect("the test arena mints");
    assert_eq!(
        draft.value_struct(vec![ValueShapeLeaf::new("v", int); MAX_STRUCT_LEAVES + 1]),
        Err(DraftStateError::CarrierDomain),
    );
    assert_eq!(draft.value_shapes().len(), 1, "the refusal mutated nothing");
}

/// The two `TooManyVariants` decision sites: the definition site draws the variant,
/// while the value-DAG site is refused at the append surface and leaves the rest of the
/// draft clean.
#[test]
fn the_enum_definition_site_draws_too_many_variants_and_a_refused_node_leaves_it() {
    assert_eq!(
        Fixture::clean()
            .fault(Fault::WideEnumDefinition)
            .fault(Fault::WideEnumValueNode)
            .encode(),
        Err(ImageBuildError::TooManyVariants),
    );
    assert_eq!(
        Fixture::clean().fault(Fault::WideEnumDefinition).encode(),
        Err(ImageBuildError::TooManyVariants),
        "the definition site draws the variant alone",
    );
    assert_eq!(
        Fixture::clean().fault(Fault::WideEnumValueNode).encode(),
        Ok(()),
        "the refused value node entered nothing, so the residual draft encodes",
    );
}

#[test]
fn vacant_functions_refuse_and_fill_order_does_not_change_image_order() {
    fn definitions(txn: &mut DraftTxn<'_>) -> Vec<FunctionDef> {
        let source = txn
            .intern_string("src/main.mw")
            .expect("valid fixture construction");
        ["first", "middle", "last"]
            .into_iter()
            .map(|name| FunctionDef {
                name: txn.intern_string(name).expect("valid fixture construction"),
                source,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                code: vec![Instr::Return],
                spans: Vec::new(),
            })
            .collect()
    }
    let mut control = ImageDraft::new();
    let mut txn = control.begin_transaction();
    for def in definitions(&mut txn) {
        txn.add_function(def).expect("valid fixture construction");
    }
    txn.commit();
    let expected = control
        .encode()
        .expect("the completed fixture encodes")
        .bytes;
    for missing in 0..3 {
        let mut owner = ImageDraft::new();
        let mut txn = owner.begin_transaction();
        let defs = definitions(&mut txn);
        let ids: Vec<_> = (0..3)
            .map(|_| {
                txn.reserve_function()
                    .expect("a within-carrier reservation")
            })
            .collect();
        for index in (0..3).rev().filter(|&index| index != missing) {
            txn.fill_function(ids[index], defs[index].clone())
                .expect("valid fixture construction");
        }
        txn.commit();
        assert_eq!(owner.function_count(), 3);
        assert!(owner.function_code(ids[missing]).is_none());
        assert_eq!(
            owner.encode().map(|_| ()),
            Err(ImageBuildError::InvalidReference(
                ReferenceKind::VacantFunction
            ))
        );
        let mut txn = owner.begin_transaction();
        txn.fill_function(ids[missing], defs[missing].clone())
            .expect("valid fixture construction");
        txn.commit();
        assert_eq!(
            owner.encode().expect("the completed fixture encodes").bytes,
            expected
        );
    }
}
