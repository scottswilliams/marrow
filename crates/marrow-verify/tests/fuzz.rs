//! Slice K.4 image-bytes fuzz driver (design §E, B02 pattern).
//!
//! A bounded, seeded, deterministic driver over the verifier's decoder: the reusable
//! oracle asserts that `verify` never panics and never allocates unboundedly on
//! arbitrary or mutated bytes — it always returns a typed rejection (or, rarely, a
//! valid image). No external fuzz dependency; a fixed iteration budget keeps it in
//! the default suite. A minimized counterexample becomes a permanent fixture.

use marrow_image::{
    DurableEnumMemberShape, DurableIndexComponent, DurableIndexShape, DurableMemberDef,
    DurableValueShape, EnumTypeDef, ExportId, FieldDef, FunctionDef, ImageDraft, ImageType, Instr,
    KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity, Scalar, SemanticPath,
    SemanticStep, SemanticStepKind, SiteDef, SpanEntry, VariantDef, image_id,
};
use marrow_verify::verify;

/// The reusable bounded oracle: `verify` must return without panicking, and any
/// success must be internally consistent (its digest recomputes over the payload).
fn oracle(bytes: &[u8]) {
    if let Ok(image) = verify(bytes) {
        // A verified image's stored digest must equal the recomputed payload digest —
        // a decode that accepts a mismatched digest would be unsound.
        let recomputed = image_id(&bytes[37..]);
        assert_eq!(image.image_id().0, recomputed.0, "verified digest mismatch");
    }
}

/// A tiny deterministic xorshift RNG (no external dependency).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

fn seed() -> u64 {
    std::env::var("MARROW_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}

fn a_good_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let answer = draft.intern_int(42);
    let code = vec![Instr::ConstLoad(answer.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
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
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

/// A good image whose value-type tables exercise the nested-value decode and
/// acyclicity paths: an `Outer` record with a scalar field, a nested `Inner` record
/// field, and an `E` enum field, plus the referenced `Inner` record and `E` enum.
/// Mutating this reaches the record-field record/enum index decode and the value-
/// graph cycle pass that plain scalar images never touch.
fn a_nested_value_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let outer = draft.intern_string("Outer");
    let inner = draft.intern_string("Inner");
    let ename = draft.intern_string("E");
    let f_inner = draft.intern_string("inner");
    let f_tag = draft.intern_string("tag");
    let f_n = draft.intern_string("n");
    let v_only = draft.intern_string("only");
    // Inner is record 0, Outer is record 1 (Outer references Inner and E).
    draft.add_record_type(RecordTypeDef {
        name: inner,
        fields: vec![FieldDef {
            name: f_n,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    draft.add_record_type(RecordTypeDef {
        name: outer,
        fields: vec![
            FieldDef {
                name: f_inner,
                ty: ImageType::Record {
                    idx: 0,
                    optional: false,
                },
                required: true,
            },
            FieldDef {
                name: f_tag,
                ty: ImageType::Enum {
                    idx: 0,
                    optional: false,
                },
                required: true,
            },
        ],
    });
    draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: v_only,
            category: false,
            payload: Vec::new(),
        }],
    });
    let answer = draft.intern_int(42);
    let name = draft.intern_string("main");
    let code = vec![Instr::ConstLoad(answer.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
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
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_nested_value_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0x2545_F491_4F6C_DD1D);
    let base = a_nested_value_image();
    // The base image itself must verify, so the decode path is reached.
    assert!(verify(&base).is_ok(), "nested value base image must verify");
    for _ in 0..4096 {
        let mut bytes = base.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

#[test]
fn random_bytes_never_panic_the_verifier() {
    let mut rng = Rng(seed());
    for _ in 0..4096 {
        let len = rng.below(512);
        let bytes: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        oracle(&bytes);
    }
}

#[test]
fn mutated_good_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0xD1B5_4A32_D192_ED03);
    let base = a_good_image();
    for _ in 0..4096 {
        let mut bytes = base.clone();
        // Flip one to three random bytes.
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

#[test]
fn structured_prefix_of_a_good_image_never_panics() {
    let base = a_good_image();
    // Every truncation of a good image must decode-reject cleanly, never panic.
    for len in 0..base.len() {
        oracle(&base[..len]);
    }
}

/// A good durable image: the tracer schema (`^counters(name:string): Counter`) plus
/// one verifying mutating export. Mutating it reaches the DURABLE-table decode and
/// the durable-contract-id recomputation that scalar/value images never touch.
fn a_durable_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let label = draft.intern_string("label");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    });
    let root = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Int),
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0f; 16]),
                    required: false,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
            ],
        },
    });
    let app = SemanticStep::new(
        SemanticStepKind::Application,
        LedgerIdBytes::from_bytes([0x0a; 16]),
    );
    let placement = SemanticStep::new(
        SemanticStepKind::Placement,
        LedgerIdBytes::from_bytes([0x0b; 16]),
    );
    draft.add_site(SiteDef::whole_payload(SemanticPath::from_steps(vec![
        app, placement,
    ])));
    let value_site = draft.add_site(SiteDef::field_leaf(SemanticPath::from_steps(vec![
        app,
        placement,
        SemanticStep::new(
            SemanticStepKind::Field,
            LedgerIdBytes::from_bytes([0x0e; 16]),
        ),
    ])));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("put");
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::LocalGet(1),
        Instr::DurSetRequired(value_site.index()),
        Instr::TxnCommit,
        Instr::Return,
    ];
    let spans = (0..code.len() as u32)
        .map(|instr_index| SpanEntry {
            instr_index,
            line: 1,
            column: 1,
        })
        .collect();
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![
            ImageType::scalar(Scalar::Text),
            ImageType::scalar(Scalar::Int),
        ],
        ret: ImageType::Unit,
        local_count: 2,
        spans,
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_durable_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0x6C62_272E_07BB_0142);
    let base = a_durable_image();
    // The base image itself must verify, so the durable decode path is reached.
    assert!(verify(&base).is_ok(), "durable base image must verify");
    for _ in 0..4096 {
        let mut bytes = base.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

/// A good durable image whose keyed root carries two managed indexes — a nonunique
/// `byLabel(label, k)` and a unique `byValue(value)` — plus their parked index read
/// sites. Mutating it reaches the index-block decoder (index count, index ids, unique
/// flags, component kinds and leaf ids, the component-resolves-to-a-real-leaf check)
/// and the index-site path/target resolver (the unique-flag agreement), which a
/// root without indexes never exercises.
fn an_indexed_durable_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let label = draft.intern_string("label");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    });
    let root = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Int),
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0f; 16]),
                    required: false,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
            ],
            indexes: vec![
                DurableIndexShape {
                    id: LedgerIdBytes::from_bytes([0x70; 16]),
                    unique: false,
                    components: vec![
                        DurableIndexComponent::Field(LedgerIdBytes::from_bytes([0x0f; 16])),
                        DurableIndexComponent::Key(LedgerIdBytes::from_bytes([0x0c; 16])),
                    ],
                },
                DurableIndexShape {
                    id: LedgerIdBytes::from_bytes([0x71; 16]),
                    unique: true,
                    components: vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
                        [0x0e; 16],
                    ))],
                },
            ],
        },
    });
    let app = SemanticStep::new(
        SemanticStepKind::Application,
        LedgerIdBytes::from_bytes([0x0a; 16]),
    );
    let placement = SemanticStep::new(
        SemanticStepKind::Placement,
        LedgerIdBytes::from_bytes([0x0b; 16]),
    );
    draft.add_site(SiteDef::index_scan(SemanticPath::from_steps(vec![
        app,
        placement,
        SemanticStep::new(
            SemanticStepKind::Index,
            LedgerIdBytes::from_bytes([0x70; 16]),
        ),
    ])));
    draft.add_site(SiteDef::index_lookup(SemanticPath::from_steps(vec![
        app,
        placement,
        SemanticStep::new(
            SemanticStepKind::Index,
            LedgerIdBytes::from_bytes([0x71; 16]),
        ),
    ])));
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_indexed_durable_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0x1B56_3C1A_9F0E_4477);
    let base = an_indexed_durable_image();
    // The base image itself must verify, so the index-block decode path is reached.
    assert!(verify(&base).is_ok(), "indexed base image must verify");
    for _ in 0..4096 {
        let mut bytes = base.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

/// A good durable image whose mutating export carries a strict present-entry sparse
/// set guarded by `if exists(p)`. Mutating it reaches the DurSetSparsePresent decode
/// (two `u16` operands) and the place-slot presence lattice, which a bare-set image
/// never exercises.
fn a_strict_durable_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let counter = draft.intern_string("Counter");
    let value = draft.intern_string("value");
    let label = draft.intern_string("label");
    let record = draft.add_record_type(RecordTypeDef {
        name: counter,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    });
    let root = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Text,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Int),
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0f; 16]),
                    required: false,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
            ],
        },
    });
    let app = SemanticStep::new(
        SemanticStepKind::Application,
        LedgerIdBytes::from_bytes([0x0a; 16]),
    );
    let placement = SemanticStep::new(
        SemanticStepKind::Placement,
        LedgerIdBytes::from_bytes([0x0b; 16]),
    );
    draft.add_site(SiteDef::whole_payload(SemanticPath::from_steps(vec![
        app, placement,
    ])));
    draft.add_site(SiteDef::field_leaf(SemanticPath::from_steps(vec![
        app,
        placement,
        SemanticStep::new(
            SemanticStepKind::Field,
            LedgerIdBytes::from_bytes([0x0e; 16]),
        ),
    ])));
    let label_site = draft.add_site(SiteDef::field_leaf(SemanticPath::from_steps(vec![
        app,
        placement,
        SemanticStep::new(
            SemanticStepKind::Field,
            LedgerIdBytes::from_bytes([0x0f; 16]),
        ),
    ])));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("tag");
    let text = draft.intern_text("x");
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurExists(0),
        Instr::JumpIfFalse(7),
        Instr::ConstLoad(text.index()),
        Instr::SomeWrap,
        Instr::DurSetSparsePresent {
            site: label_site.index(),
            key_slot: 0,
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let spans = (0..code.len() as u32)
        .map(|instr_index| SpanEntry {
            instr_index,
            line: 1,
            column: 1,
        })
        .collect();
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Text)],
        ret: ImageType::Unit,
        local_count: 1,
        spans,
        code,
    });
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_strict_durable_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0x8A5C_D789_0AB0_1C3F);
    let base = a_strict_durable_image();
    // The base image itself must verify, so the strict-set decode and the presence
    // lattice are reached before mutation.
    assert!(
        verify(&base).is_ok(),
        "strict durable base image must verify"
    );
    for _ in 0..4096 {
        let mut bytes = base.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

/// A good durable image whose resource declares a static `group` (holding a field)
/// and a keyed `branch` (holding a field). Mutating it reaches the recursive
/// durable member-tree decoder — its group/branch tags, the `Group` id, the branch
/// placement and key tuple, and the nesting-depth and member-count bounds — that a
/// flat root never exercises.
fn a_group_branch_durable_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let book = draft.intern_string("Book");
    let title = draft.intern_string("title");
    let record = draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![FieldDef {
            name: title,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let root = draft.intern_string("books");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
                DurableMemberDef::Group {
                    id: LedgerIdBytes::from_bytes([0x20; 16]),
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes([0x21; 16]),
                        required: false,
                        value: DurableValueShape::Scalar(Scalar::Int),
                    }],
                },
                DurableMemberDef::Branch {
                    placement: LedgerIdBytes::from_bytes([0x30; 16]),
                    keys: vec![KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes([0x31; 16]),
                    }],
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes([0x32; 16]),
                        required: true,
                        value: DurableValueShape::Scalar(Scalar::Text),
                    }],
                },
            ],
        },
    });
    // The whole-graph operation sites the compiler emits for a nested graph: a
    // whole-payload site per keyed placement (root and `notes` branch) and a
    // field-leaf site per stored field (`title`, `details.pages`, `notes.text`).
    // Every site seals as parked; mutating the image reaches the nested-site path
    // decode and node resolution a flat root never exercises.
    let app = SemanticStep::new(
        SemanticStepKind::Application,
        LedgerIdBytes::from_bytes([0x0a; 16]),
    );
    let root_place = SemanticStep::new(
        SemanticStepKind::Placement,
        LedgerIdBytes::from_bytes([0x0b; 16]),
    );
    let group = SemanticStep::new(
        SemanticStepKind::Group,
        LedgerIdBytes::from_bytes([0x20; 16]),
    );
    let branch = SemanticStep::new(
        SemanticStepKind::Placement,
        LedgerIdBytes::from_bytes([0x30; 16]),
    );
    let field =
        |id: [u8; 16]| SemanticStep::new(SemanticStepKind::Field, LedgerIdBytes::from_bytes(id));
    draft.add_site(SiteDef::whole_payload(SemanticPath::from_steps(vec![
        app, root_place,
    ])));
    draft.add_site(SiteDef::field_leaf(SemanticPath::from_steps(vec![
        app,
        root_place,
        field([0x0e; 16]),
    ])));
    draft.add_site(SiteDef::field_leaf(SemanticPath::from_steps(vec![
        app,
        root_place,
        group,
        field([0x21; 16]),
    ])));
    draft.add_site(SiteDef::whole_payload(SemanticPath::from_steps(vec![
        app, root_place, branch,
    ])));
    draft.add_site(SiteDef::field_leaf(SemanticPath::from_steps(vec![
        app,
        root_place,
        branch,
        field([0x32; 16]),
    ])));
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("label");
    let zero = draft.intern_int(0);
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let spans = (0..code.len() as u32)
        .map(|instr_index| SpanEntry {
            instr_index,
            line: 1,
            column: 1,
        })
        .collect();
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans,
        code,
    });
    draft.add_export(ExportId::of_local("", "label"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_group_branch_durable_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0x2545_F491_4F6C_DD1D);
    let base = a_group_branch_durable_image();
    // The base image itself must verify, so the member-tree decode path is reached.
    assert!(verify(&base).is_ok(), "group/branch base image must verify");
    for _ in 0..4096 {
        let mut bytes = base.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

/// A good durable image whose resource stores widened value shapes: a plain scalar
/// (`id`), a closed enum with a payload-carrying member (`kind: Access`), and a dense
/// struct (`owner: Pair`). Mutating it reaches the recursive value-shape decoder —
/// its value tags, the enum sum/member ids and payload leaves, the struct leaf
/// count, and the value-nesting-depth bound — plus the value-shape/record cross-check
/// that a flat scalar root never exercises.
fn a_widened_durable_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    // Enum `Access { a, b(int) }` — `b` carries an int payload leaf.
    let access = draft.intern_string("Access");
    let a = draft.intern_string("a");
    let b = draft.intern_string("b");
    draft.add_enum_type(EnumTypeDef {
        name: access,
        variants: vec![
            VariantDef {
                name: a,
                category: false,
                payload: Vec::new(),
            },
            VariantDef {
                name: b,
                category: false,
                payload: vec![ImageType::scalar(Scalar::Int)],
            },
        ],
    });
    // Struct `Pair { x:int, y:string }` at record index 0.
    let pair = draft.intern_string("Pair");
    let x = draft.intern_string("x");
    let y = draft.intern_string("y");
    let pair_ty = draft.add_record_type(RecordTypeDef {
        name: pair,
        fields: vec![
            FieldDef {
                name: x,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: y,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
        ],
    });
    // Resource `W { id:int, kind:Access, owner:Pair }` at record index 1.
    let w = draft.intern_string("W");
    let idn = draft.intern_string("id");
    let kindn = draft.intern_string("kind");
    let ownern = draft.intern_string("owner");
    let record = draft.add_record_type(RecordTypeDef {
        name: w,
        fields: vec![
            FieldDef {
                name: idn,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: kindn,
                ty: ImageType::Enum {
                    idx: 0,
                    optional: false,
                },
                required: true,
            },
            FieldDef {
                name: ownern,
                ty: ImageType::Record {
                    idx: pair_ty.index(),
                    optional: false,
                },
                required: false,
            },
        ],
    });
    let root = draft.intern_string("ws");
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x0b; 16]),
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Int),
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x0f; 16]),
                    required: true,
                    value: DurableValueShape::Enum {
                        sum: LedgerIdBytes::from_bytes([0x50; 16]),
                        members: vec![
                            DurableEnumMemberShape {
                                id: LedgerIdBytes::from_bytes([0x51; 16]),
                                payload: Vec::new(),
                            },
                            DurableEnumMemberShape {
                                id: LedgerIdBytes::from_bytes([0x52; 16]),
                                payload: vec![DurableValueShape::Scalar(Scalar::Int)],
                            },
                        ],
                    },
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes([0x10; 16]),
                    required: false,
                    value: DurableValueShape::Struct(vec![
                        DurableValueShape::Scalar(Scalar::Int),
                        DurableValueShape::Scalar(Scalar::Text),
                    ]),
                },
            ],
        },
    });
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("label");
    let zero = draft.intern_int(0);
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let spans = (0..code.len() as u32)
        .map(|instr_index| SpanEntry {
            instr_index,
            line: 1,
            column: 1,
        })
        .collect();
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans,
        code,
    });
    draft.add_export(ExportId::of_local("", "label"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_widened_durable_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0x94D0_49BB_1331_11EB);
    let base = a_widened_durable_image();
    // The base image itself must verify, so the value-shape decode/cross-check path
    // is reached.
    assert!(verify(&base).is_ok(), "widened base image must verify");
    for _ in 0..4096 {
        let mut bytes = base.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

/// A good durable image over a flat root with several scalar fields, so its site
/// table carries the whole-payload site plus one field-leaf site per field. Mutating
/// it concentrates on the site-path decoder — the per-site step count, per-step
/// ledger-kind byte and 16-byte id, the target-kind byte, and the resolution of each
/// path against the reconstructed node set — that a one- or two-site image barely
/// exercises.
fn a_multi_site_durable_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let rec = draft.intern_string("Row");
    let mut field_defs = Vec::new();
    for name in ["a", "b", "c", "d"] {
        let field = draft.intern_string(name);
        field_defs.push(FieldDef {
            name: field,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        });
    }
    let record = draft.add_record_type(RecordTypeDef {
        name: rec,
        fields: field_defs,
    });
    let root = draft.intern_string("rows");
    let application = LedgerIdBytes::from_bytes([0x0a; 16]);
    let placement = LedgerIdBytes::from_bytes([0x0b; 16]);
    draft.set_application_identity(application);
    let field_ids = [
        LedgerIdBytes::from_bytes([0x0e; 16]),
        LedgerIdBytes::from_bytes([0x0f; 16]),
        LedgerIdBytes::from_bytes([0x1e; 16]),
        LedgerIdBytes::from_bytes([0x1f; 16]),
    ];
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x0c; 16]),
        }],
        record,
        identity: RootIdentity {
            placement,
            product: LedgerIdBytes::from_bytes([0x0d; 16]),
            indexes: Vec::new(),
            members: field_ids
                .iter()
                .map(|id| DurableMemberDef::Field {
                    id: *id,
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Int),
                })
                .collect(),
        },
    });
    let app_step = SemanticStep::new(SemanticStepKind::Application, application);
    let placement_step = SemanticStep::new(SemanticStepKind::Placement, placement);
    draft.add_site(SiteDef::whole_payload(SemanticPath::from_steps(vec![
        app_step,
        placement_step,
    ])));
    for id in field_ids {
        draft.add_site(SiteDef::field_leaf(SemanticPath::from_steps(vec![
            app_step,
            placement_step,
            SemanticStep::new(SemanticStepKind::Field, id),
        ])));
    }
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("label");
    let zero = draft.intern_int(0);
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
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
    });
    draft.add_export(ExportId::of_local("", "label"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_multi_site_durable_images_never_panic_the_verifier() {
    let mut rng = Rng(seed() ^ 0x1D87_2B41_09CC_5E2F);
    let base = a_multi_site_durable_image();
    // The base image itself must verify, so every site-path decode/resolution is
    // reached before mutation.
    assert!(verify(&base).is_ok(), "multi-site base image must verify");
    for _ in 0..4096 {
        let mut bytes = base.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}
