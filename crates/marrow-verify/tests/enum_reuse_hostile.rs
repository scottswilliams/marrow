//! Forged-image hostiles for the durable enum-reuse identity law. An enum's sum and
//! member ledger ids are the identity of the enum *declaration*: two durable fields of
//! the same enum type reference that one identity, and the verifier must accept the
//! reuse. The guard narrows — it does not vanish: a genuinely duplicated declaration id
//! (a repeated field id, or a field id colliding with an enum's sum id) and an
//! inconsistent enum reference (the same sum id presenting different member ids) must
//! still reject at the table phase. Each artifact carries a valid encoder-computed
//! digest, so it exercises the verifier's own decode rather than the digest gate.

use marrow_image::{
    DurableMemberDef, DurableValueShape, EnumTypeDef, ExportId, FieldDef, FunctionDef, ImageDraft,
    ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity, Scalar,
    SemanticPath, SiteDef, SpanEntry, VariantDef,
};
use marrow_verify::{VerifyPhase, verify};

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT: [u8; 16] = [0x0b; 16];
const PRODUCT: [u8; 16] = [0x0d; 16];
const KEY: [u8; 16] = [0x0c; 16];
const FIELD_ONE: [u8; 16] = [0x0e; 16];
const FIELD_TWO: [u8; 16] = [0x0f; 16];
// The one enum declaration's durable identity: its sum and per-member ids.
const SUM: [u8; 16] = [0x50; 16];
const MEMBER_READER: [u8; 16] = [0x51; 16];
const MEMBER_WRITER: [u8; 16] = [0x52; 16];
const MEMBER_ADMIN: [u8; 16] = [0x53; 16];

fn id(bytes: [u8; 16]) -> LedgerIdBytes {
    LedgerIdBytes::from_bytes(bytes)
}

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

/// Add the enum type `Access { reader, writer, admin }` (all empty-payload) and return
/// its table index.
fn access_enum(draft: &mut ImageDraft) -> u16 {
    let name = draft.intern_string("Access");
    let reader = draft.intern_string("reader");
    let writer = draft.intern_string("writer");
    let admin = draft.intern_string("admin");
    draft
        .add_enum_type(EnumTypeDef {
            name,
            variants: vec![
                VariantDef {
                    name: reader,
                    category: false,
                    payload: vec![],
                },
                VariantDef {
                    name: writer,
                    category: false,
                    payload: vec![],
                },
                VariantDef {
                    name: admin,
                    category: false,
                    payload: vec![],
                },
            ],
        })
        .index()
}

/// The durable value shape of an `Access` field, parameterized on its member ids so a
/// hostile can present the same `sum` with an altered member set.
fn access_shape(sum: [u8; 16], members: [[u8; 16]; 3]) -> DurableValueShape {
    DurableValueShape::Enum {
        sum: id(sum),
        members: members
            .iter()
            .map(|m| marrow_image::DurableEnumMemberShape {
                id: id(*m),
                payload: vec![],
            })
            .collect(),
    }
}

/// A one-root image ("grants", int key) whose `Access` record carries two enum fields.
/// Each field's ledger id and durable enum value shape are supplied so a hostile can
/// forge a duplicate field id or an inconsistent enum reference.
fn build(
    field_one: [u8; 16],
    shape_one: DurableValueShape,
    field_two: [u8; 16],
    shape_two: DurableValueShape,
) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    draft.set_application_identity(id(APPLICATION_ID));
    let enum_idx = access_enum(&mut draft);
    let ty = ImageType::Enum {
        idx: enum_idx,
        optional: false,
    };
    let first = draft.intern_string("first");
    let second = draft.intern_string("second");
    let rec_name = draft.intern_string("Grant");
    let record = draft.add_record_type(RecordTypeDef {
        name: rec_name,
        fields: vec![
            FieldDef {
                name: first,
                ty,
                required: true,
            },
            FieldDef {
                name: second,
                ty,
                required: true,
            },
        ],
    });
    let root_name = draft.intern_string("grants");
    draft.add_root(RootDef {
        name: root_name,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: id(KEY),
        }],
        record,
        identity: RootIdentity {
            placement: id(PLACEMENT),
            product: id(PRODUCT),
            indexes: vec![],
            members: vec![
                DurableMemberDef::Field {
                    id: id(field_one),
                    required: true,
                    value: shape_one,
                },
                DurableMemberDef::Field {
                    id: id(field_two),
                    required: true,
                    value: shape_two,
                },
            ],
        },
    });
    draft.request_site(SiteDef::whole_payload(SemanticPath::root(
        id(APPLICATION_ID),
        id(PLACEMENT),
    )));
    let src = draft.intern_string("src/main.mw");
    let fname = draft.intern_string("f");
    let code = vec![Instr::LocalGet(0), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: fname,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::scalar(Scalar::Int),
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.encode().expect("encode").bytes
}

/// The reference shape: one enum declaration reused by two fields. Both fields present
/// the identical sum and member ids — the honest reuse the verifier must accept.
fn shared() -> (DurableValueShape, DurableValueShape) {
    let members = [MEMBER_READER, MEMBER_WRITER, MEMBER_ADMIN];
    (access_shape(SUM, members), access_shape(SUM, members))
}

#[test]
fn two_fields_of_one_enum_type_verify() {
    let (one, two) = shared();
    let bytes = build(FIELD_ONE, one, FIELD_TWO, two);
    assert!(
        verify(&bytes).is_ok(),
        "two durable fields of one enum type must verify"
    );
}

#[test]
fn a_repeated_field_id_still_rejects() {
    // Genuine duplicate: the second field claims the first field's ledger id.
    let (one, two) = shared();
    let rejection = verify(&build(FIELD_ONE, one, FIELD_ONE, two))
        .expect_err("a repeated field id must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert!(
        rejection.detail().contains("duplicate durable ledger id"),
        "expected a duplicate-ledger-id rejection, got {rejection:?}"
    );
}

#[test]
fn a_field_id_colliding_with_the_enum_sum_still_rejects() {
    // Genuine duplicate across identity kinds: the second field's ledger id equals the
    // enum's sum id, which the first field's value shape already claimed.
    let (one, two) = shared();
    let rejection = verify(&build(FIELD_ONE, one, SUM, two))
        .expect_err("a field id colliding with an enum sum id must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert!(
        rejection.detail().contains("duplicate durable ledger id"),
        "expected a duplicate-ledger-id rejection, got {rejection:?}"
    );
}

#[test]
fn an_inconsistent_enum_reference_rejects() {
    // The second field reuses the sum id but presents a different member id: a forged
    // identity that claims to be the same enum while carrying a different member set.
    let one = access_shape(SUM, [MEMBER_READER, MEMBER_WRITER, MEMBER_ADMIN]);
    let two = access_shape(SUM, [MEMBER_READER, MEMBER_WRITER, [0x99; 16]]);
    let rejection = verify(&build(FIELD_ONE, one, FIELD_TWO, two))
        .expect_err("an inconsistent enum reference must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert!(
        rejection.detail().contains("different member set"),
        "expected an inconsistent-enum-reuse rejection, got {rejection:?}"
    );
}

// ---------------------------------------------------------------------------
// The pairwise declaration-identity matrix.
//
// One global declaration-id owner governs the durable table: a fresh application,
// outer-root placement or key, Product, Product field, group, branch placement,
// branch key, managed index, enum sum, or enum member claims its ledger bytes
// globally. Exactly two reuses are references rather than claims — a later root
// carrying an already-declared Product, and a later field carrying an already-
// declared enum sum — and each admits only when it repeats that declaration
// exactly. Every other reuse, in either direction and across every pair of the
// eleven declaration kinds, rejects.
//
// The matrix generates all 11 x 11 ordered pairs from one two-root base graph whose
// every declaration id is distinct: for the pair `(first, second)` the second root's
// `second` slot is forced to the first root's `first` id, and the artifact must
// reject. The two reference diagonals instead repeat the whole referenced
// declaration — the exact form the law admits — and must verify.
// ---------------------------------------------------------------------------

use marrow_image::{DurableIndexComponent, DurableIndexShape, SemanticStep, SemanticStepKind};

/// The eleven durable declaration kinds a ledger id may be claimed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclarationKind {
    Application,
    RootPlacement,
    RootKey,
    Product,
    Field,
    Group,
    BranchPlacement,
    BranchKey,
    Index,
    Sum,
    Member,
}

use DeclarationKind::*;

const KINDS: [DeclarationKind; 11] = [
    Application,
    RootPlacement,
    RootKey,
    Product,
    Field,
    Group,
    BranchPlacement,
    BranchKey,
    Index,
    Sum,
    Member,
];

/// One root occurrence's declaration ids. The Product-scoped half — `product` through
/// `members` — is what a second root referencing this Product repeats exactly; the
/// occurrence half (`placement`, `key`, `index`) is its own whatever the Product.
#[derive(Clone, PartialEq, Eq)]
struct RootSpec {
    placement: [u8; 16],
    key: [u8; 16],
    index: [u8; 16],
    product: [u8; 16],
    field: [u8; 16],
    enum_field: [u8; 16],
    group: [u8; 16],
    group_field: [u8; 16],
    branch: [u8; 16],
    branch_key: [u8; 16],
    branch_field: [u8; 16],
    sum: [u8; 16],
    members: [[u8; 16]; 3],
}

/// A two-root durable graph, by declaration id.
#[derive(Clone)]
struct GraphSpec {
    application: [u8; 16],
    roots: [RootSpec; 2],
}

impl GraphSpec {
    /// The base graph: two roots over structurally identical but separately declared
    /// Products, so every declaration id in the table is distinct and any collision the
    /// matrix introduces is the only one.
    fn base() -> Self {
        let root = |b: u8| RootSpec {
            placement: [b; 16],
            key: [b + 1; 16],
            index: [b + 2; 16],
            product: [b + 3; 16],
            field: [b + 4; 16],
            enum_field: [b + 5; 16],
            group: [b + 6; 16],
            group_field: [b + 7; 16],
            branch: [b + 8; 16],
            branch_key: [b + 9; 16],
            branch_field: [b + 10; 16],
            sum: [b + 11; 16],
            members: [[b + 12; 16], [b + 13; 16], [b + 14; 16]],
        };
        Self {
            application: [0x01; 16],
            roots: [root(0x10), root(0x40)],
        }
    }

    /// The id claimed as `kind` by the root occurrence at `root`, or `None` when that
    /// kind has no declaration there. The application is declared once for the whole
    /// table, so it is reachable only through the first root's block.
    fn claimed(&self, kind: DeclarationKind, root: usize) -> Option<[u8; 16]> {
        let spec = &self.roots[root];
        Some(match kind {
            Application if root == 0 => self.application,
            Application => return None,
            RootPlacement => spec.placement,
            RootKey => spec.key,
            Product => spec.product,
            Field => spec.field,
            Group => spec.group,
            BranchPlacement => spec.branch,
            BranchKey => spec.branch_key,
            Index => spec.index,
            Sum => spec.sum,
            Member => spec.members[0],
        })
    }

    /// Force the declaration of `kind` at `root` to carry `id`.
    fn force(&mut self, kind: DeclarationKind, root: usize, id: [u8; 16]) {
        if kind == Application {
            self.application = id;
            return;
        }
        let spec = &mut self.roots[root];
        match kind {
            Application => unreachable!("handled above"),
            RootPlacement => spec.placement = id,
            RootKey => spec.key = id,
            Product => spec.product = id,
            Field => spec.field = id,
            Group => spec.group = id,
            BranchPlacement => spec.branch = id,
            BranchKey => spec.branch_key = id,
            Index => spec.index = id,
            Sum => spec.sum = id,
            Member => spec.members[0] = id,
        }
    }
}

/// The durable value shape of a `Access` field for one root's enum declaration.
fn spec_enum_shape(spec: &RootSpec) -> DurableValueShape {
    access_shape(spec.sum, spec.members)
}

/// Encode the two-root artifact `spec` describes. Every root carries the same
/// structural graph — an int field, an enum field, a one-field group, a keyed branch,
/// and one unique managed index — so the artifacts differ only in which ledger ids
/// they claim.
fn forge(spec: &GraphSpec) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    draft.set_application_identity(id(spec.application));
    let enum_idx = access_enum(&mut draft);
    let access = ImageType::Enum {
        idx: enum_idx,
        optional: false,
    };
    let int = ImageType::scalar(Scalar::Int);

    let group_rec_name = draft.intern_string("Marks");
    let mark = draft.intern_string("mark");
    let group_record = draft.add_record_type(RecordTypeDef {
        name: group_rec_name,
        fields: vec![FieldDef {
            name: mark,
            ty: int,
            required: true,
        }],
    });
    let branch_rec_name = draft.intern_string("Note");
    let body = draft.intern_string("body");
    let branch_record = draft.add_record_type(RecordTypeDef {
        name: branch_rec_name,
        fields: vec![FieldDef {
            name: body,
            ty: int,
            required: true,
        }],
    });
    let root_rec_name = draft.intern_string("Grant");
    let count = draft.intern_string("count");
    let access_name = draft.intern_string("access");
    let marks = draft.intern_string("marks");
    let root_record = draft.add_record_type(RecordTypeDef {
        name: root_rec_name,
        fields: vec![
            FieldDef {
                name: count,
                ty: int,
                required: true,
            },
            FieldDef {
                name: access_name,
                ty: access,
                required: true,
            },
            FieldDef {
                name: marks,
                ty: ImageType::Record {
                    idx: group_record.index(),
                    optional: false,
                },
                required: true,
            },
        ],
    });
    let notes = draft.intern_string("notes");

    for (position, root) in spec.roots.iter().enumerate() {
        let name = draft.intern_string(if position == 0 { "a" } else { "b" });
        draft.add_root(RootDef {
            name,
            keys: vec![KeyColumn {
                scalar: Scalar::Int,
                id: id(root.key),
            }],
            record: root_record,
            identity: RootIdentity {
                placement: id(root.placement),
                product: id(root.product),
                members: vec![
                    DurableMemberDef::Field {
                        id: id(root.field),
                        required: true,
                        value: DurableValueShape::Scalar(Scalar::Int),
                    },
                    DurableMemberDef::Field {
                        id: id(root.enum_field),
                        required: true,
                        value: spec_enum_shape(root),
                    },
                    DurableMemberDef::Group {
                        id: id(root.group),
                        members: vec![DurableMemberDef::Field {
                            id: id(root.group_field),
                            required: true,
                            value: DurableValueShape::Scalar(Scalar::Int),
                        }],
                    },
                    DurableMemberDef::Branch {
                        placement: id(root.branch),
                        name: notes,
                        record: branch_record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Int,
                            id: id(root.branch_key),
                        }],
                        members: vec![DurableMemberDef::Field {
                            id: id(root.branch_field),
                            required: true,
                            value: DurableValueShape::Scalar(Scalar::Int),
                        }],
                    },
                ],
                indexes: vec![DurableIndexShape {
                    id: id(root.index),
                    unique: true,
                    components: vec![
                        DurableIndexComponent::Field(id(root.field)),
                        DurableIndexComponent::Key(id(root.key)),
                    ],
                }],
            },
        });
        let root_path = SemanticPath::root(id(spec.application), id(root.placement));
        draft.request_site(SiteDef::whole_payload(root_path.clone()));
        draft.request_site(SiteDef::index_lookup(
            root_path.child(SemanticStep::new(SemanticStepKind::Index, id(root.index))),
        ));
    }

    let src = draft.intern_string("src/main.mw");
    let fname = draft.intern_string("f");
    let code = vec![Instr::LocalGet(0), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: fname,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::scalar(Scalar::Int),
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.encode().expect("encode").bytes
}

/// What the law says of one ordered pair: a second declaration of that kind carrying an
/// id the first kind already claimed either references the first declaration or is
/// refused. `Singleton` marks the one pair that names no second declaration at all.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Reference,
    Refuse,
    Singleton,
}

/// The law's verdict for one ordered pair, stated independently of the artifacts.
fn verdict(first: DeclarationKind, second: DeclarationKind) -> Verdict {
    match (first, second) {
        // The application is declared exactly once per durable table, so there is no
        // second application declaration to admit or refuse.
        (Application, Application) => Verdict::Singleton,
        // The two reference exceptions: a later root carrying an already-declared
        // Product, and a later field carrying an already-declared enum sum.
        (Product, Product) | (Sum, Sum) => Verdict::Reference,
        _ => Verdict::Refuse,
    }
}

/// The artifact for one ordered pair. A refusal pair aliases exactly one id: the second
/// root's `second` declaration claims the first root's `first` id, and nothing else
/// changes. A reference pair repeats the whole referenced declaration instead, because
/// that — not a bare id alias — is the form the law admits.
fn pair_artifact(first: DeclarationKind, second: DeclarationKind) -> Option<Vec<u8>> {
    let mut spec = GraphSpec::base();
    match verdict(first, second) {
        Verdict::Singleton => return None,
        Verdict::Reference => {
            let declared = spec.roots[0].clone();
            let second = &mut spec.roots[1];
            if first == Product {
                // The second root references the first root's Product: the same Product
                // id and the identical member/value graph, while its own placement, key
                // tuple, and managed index — every occurrence fact — stay its own. Its
                // index projects the referenced declaration's field, which is the only
                // field it now has.
                second.product = declared.product;
                second.field = declared.field;
                second.enum_field = declared.enum_field;
                second.group = declared.group;
                second.group_field = declared.group_field;
                second.branch = declared.branch;
                second.branch_key = declared.branch_key;
                second.branch_field = declared.branch_field;
                second.sum = declared.sum;
                second.members = declared.members;
            } else {
                // The second root's enum field references the first root's enum
                // declaration: the same sum id and the identical ordered member ids.
                second.sum = declared.sum;
                second.members = declared.members;
            }
        }
        Verdict::Refuse => {
            let claimed = spec
                .claimed(first, 0)
                .expect("the first root declares every kind");
            // The application is one per table, so a second application declaration is
            // the table's own application slot rather than a per-root one.
            let root = usize::from(second != Application);
            spec.force(second, root, claimed);
        }
    }
    Some(forge(&spec))
}

#[test]
fn the_base_two_root_graph_verifies() {
    // The matrix's premise: with every declaration id distinct, the artifact verifies,
    // so each cell's outcome is caused by the one collision it introduces.
    let bytes = forge(&GraphSpec::base());
    assert!(
        verify(&bytes).is_ok(),
        "the collision-free base graph must verify"
    );
}

#[test]
fn only_the_two_reference_exceptions_admit_a_reused_declaration_id() {
    let mut cells = 0usize;
    let mut references = 0usize;
    let mut refusals = 0usize;
    let mut singletons = 0usize;
    for first in KINDS {
        for second in KINDS {
            cells += 1;
            let expected = verdict(first, second);
            let Some(bytes) = pair_artifact(first, second) else {
                assert_eq!(expected, Verdict::Singleton);
                singletons += 1;
                continue;
            };
            match expected {
                Verdict::Reference => {
                    references += 1;
                    assert!(
                        verify(&bytes).is_ok(),
                        "{first:?} -> {second:?}: an exact reference to an already \
                         declared {first:?} must verify"
                    );
                }
                Verdict::Refuse => {
                    refusals += 1;
                    let Err(rejection) = verify(&bytes) else {
                        panic!(
                            "{first:?} -> {second:?}: a {second:?} declaration must not \
                             claim an id already declared as {first:?}"
                        );
                    };
                    assert_eq!(
                        rejection.phase(),
                        VerifyPhase::Table,
                        "{first:?} -> {second:?} must reject in the table phase"
                    );
                }
                Verdict::Singleton => unreachable!("a singleton pair builds no artifact"),
            }
        }
    }
    assert_eq!(cells, 121, "the matrix covers every ordered pair");
    assert_eq!(references, 2, "exactly two reuses are references");
    assert_eq!(singletons, 1, "only the application is declared once");
    assert_eq!(refusals, 118);
}
