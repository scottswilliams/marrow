//! Image-bytes fuzz driver: `verify` must return — never panic — for every byte string,
//! and any image it accepts must be internally consistent.
//!
//! A finite, seeded, deterministic driver calls `verify` on generated and mutated bytes.
//! Each corpus is one good image whose shape reaches a decode path plainer images never
//! touch. No external fuzz dependency; a fixed iteration budget keeps this in the default
//! suite. A minimized counterexample becomes a permanent fixture.

use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, EnumId, EnumTypeDef, ExportId, FieldDef,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootOccurrenceDef,
    Scalar, SemanticTarget, TypeId, VariantDef, image_id,
};
use marrow_test_support::{admitted_plan, site};
use marrow_verify::verify;

use marrow_test_support::tracer_schema;
use tracer_schema::*;

fn ledger(bytes: [u8; 16]) -> LedgerIdBytes {
    LedgerIdBytes::from_bytes(bytes)
}

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

/// Flip one to three random bytes of `base` for a fixed budget of rounds, and put every
/// result past the oracle. `salt` decorrelates one corpus's byte choices from another's.
fn mutation_fuzz(base: Vec<u8>, salt: u64, what: &str) {
    // The base must verify, or the mutations never reach the decode path the corpus exists
    // to cover.
    assert!(verify(&base).is_ok(), "{what} base image must verify");
    let mut rng = Rng(seed() ^ salt);
    for _ in 0..4096 {
        let mut bytes = base.clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= rng.byte();
        }
        oracle(&bytes);
    }
}

fn seed() -> u64 {
    std::env::var("MARROW_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}

fn a_good_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let answer = ok(draft.intern_int(42));
    let code = vec![Instr::ConstLoad(answer), Instr::Return];
    let func = add_int_fn(&mut draft, "main", code);
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

/// A good image whose value-type tables exercise the nested-value decode and
/// acyclicity paths: an `Outer` record with a scalar field, a nested `Inner` record
/// field, and an `E` enum field, plus the referenced `Inner` record and `E` enum.
/// Mutating this reaches the record-field record/enum index decode and the value-
/// graph cycle pass that plain scalar images never touch.
fn a_nested_value_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let inner = ok(draft.intern_string("Inner"));
    let outer = ok(draft.intern_string("Outer"));
    let ename = ok(draft.intern_string("E"));
    let f_inner = ok(draft.intern_string("inner"));
    let f_tag = ok(draft.intern_string("tag"));
    let f_n = ok(draft.intern_string("n"));
    let v_only = ok(draft.intern_string("only"));
    // Inner is record 0, Outer is record 1 (Outer references Inner and E).
    ok(draft.add_record_type(RecordTypeDef {
        name: inner,
        fields: vec![FieldDef {
            name: f_n,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    }));
    ok(draft.add_record_type(RecordTypeDef {
        name: outer,
        fields: vec![
            FieldDef {
                name: f_inner,
                ty: ImageType::Record {
                    idx: TypeId::from_index(0),
                    optional: false,
                },
                required: true,
            },
            FieldDef {
                name: f_tag,
                ty: ImageType::Enum {
                    idx: EnumId::from_index(0),
                    optional: false,
                },
                required: true,
            },
        ],
    }));
    ok(draft.add_enum_type(EnumTypeDef {
        name: ename,
        variants: vec![VariantDef {
            name: v_only,
            category: false,
            payload: Vec::new(),
        }],
    }));
    let answer = ok(draft.intern_int(42));
    let code = vec![Instr::ConstLoad(answer), Instr::Return];
    let func = add_int_fn(&mut draft, "main", code);
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_nested_value_images_never_panic_the_verifier() {
    mutation_fuzz(
        a_nested_value_image(),
        0x2545_F491_4F6C_DD1D,
        "nested value",
    );
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
    mutation_fuzz(a_good_image(), 0xD1B5_4A32_D192_ED03, "scalar");
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
    put_export(|sites| {
        vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurEraseEntry(sites.entry.clone()),
            Instr::TxnCommit,
            Instr::Return,
        ]
    })
    .encode()
    .expect("encode")
    .bytes
}

#[test]
fn mutated_durable_images_never_panic_the_verifier() {
    mutation_fuzz(a_durable_image(), 0x6C62_272E_07BB_0142, "durable");
}

/// A good durable image whose keyed root carries two managed indexes — a nonunique
/// `byLabel(label, k)` and a unique `byValue(value)` — plus their parked index read
/// sites. Mutating it reaches the index-block decoder (index count, index ids, unique
/// flags, component kinds and leaf ids, the component-resolves-to-a-real-leaf check)
/// and the index-site path/target resolver (the unique-flag agreement), which a
/// root without indexes never exercises.
fn an_indexed_durable_image() -> Vec<u8> {
    let (mut draft_owner, root) = indexed_draft(by_label_projection());
    let mut draft = draft_owner.begin_transaction();
    site(
        &mut draft,
        root.occurrence(),
        &root.index_paths()[0],
        SemanticTarget::IndexScan,
    );
    site(
        &mut draft,
        root.occurrence(),
        &root.index_paths()[1],
        SemanticTarget::IndexLookup,
    );
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_indexed_durable_images_never_panic_the_verifier() {
    mutation_fuzz(an_indexed_durable_image(), 0x1B56_3C1A_9F0E_4477, "indexed");
}

/// A good durable image whose mutating export carries a strict present-entry sparse
/// set guarded by `if exists(p)`. Mutating it reaches the DurSetField decode
/// (a `u16` site, a `u16` key-path length, then one `u16` per key-path slot) and the
/// address-slot presence lattice, which a bare-set image never exercises.
fn a_strict_durable_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let sites = durable_schema(&mut draft);
    let text = ok(draft.intern_text("x"));
    let code = vec![
        Instr::TxnBegin,
        Instr::LocalGet(0),
        Instr::DurExists(sites.entry),
        Instr::JumpIfFalse(6),
        Instr::ConstLoad(text),
        Instr::DurSetField {
            site: sites.label,
            key_slots: vec![0],
        },
        Instr::TxnCommit,
        Instr::Return,
    ];
    let func = add_fn(
        &mut draft,
        "tag",
        vec![ImageType::scalar(Scalar::Text)],
        ImageType::Unit,
        1,
        code,
    );
    draft.add_export(ExportId::of_local("", "e"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_strict_durable_images_never_panic_the_verifier() {
    mutation_fuzz(
        a_strict_durable_image(),
        0x8A5C_D789_0AB0_1C3F,
        "strict durable",
    );
}

/// A good durable image whose resource declares a static `group` (holding a field)
/// and a keyed `branch` (holding a field). Mutating it reaches the recursive
/// durable member-tree decoder — its group/branch tags, the `Group` id, the branch
/// placement and key tuple, and the nesting-depth and member-count bounds — that a
/// flat root never exercises.
fn a_group_branch_durable_image() -> Vec<u8> {
    let (mut draft_owner, root) = group_branch_draft(false);
    let mut draft = draft_owner.begin_transaction();
    // The whole-graph operation sites a compiler emits for a nested graph: a whole-payload
    // site per keyed placement (the root and the `notes` branch) and a field-leaf site per
    // stored field (`title`, `details.pages`, `notes.text`).
    site(
        &mut draft,
        root.occurrence(),
        root.placement_path(),
        SemanticTarget::WholePayload,
    );
    book_title_site(&mut draft, &root);
    book_group_field_site(&mut draft, &root);
    book_branch_entry_site(&mut draft, &root);
    book_branch_field_site(&mut draft, &root);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_group_branch_durable_images_never_panic_the_verifier() {
    mutation_fuzz(
        a_group_branch_durable_image(),
        0x2545_F491_4F6C_DD1D,
        "group/branch",
    );
}

/// A good durable image whose resource stores widened value shapes: a plain scalar
/// (`id`), a closed enum with a payload-carrying member (`kind: Access`), and a dense
/// struct (`owner: Pair`). Mutating it reaches the recursive value-shape decoder —
/// its value tags, the enum sum/member ids and payload leaves, the struct leaf
/// count, and the value-nesting-depth bound — plus the value-shape/record cross-check
/// that a flat scalar root never exercises.
fn a_widened_durable_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    // Enum `Access { a, b(int) }` — `b` carries an int payload leaf.
    let access = ok(draft.intern_string("Access"));
    let a = ok(draft.intern_string("a"));
    let b = ok(draft.intern_string("b"));
    ok(draft.add_enum_type(EnumTypeDef {
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
    }));
    // Struct `Pair { x:int, y:string }` at record index 0.
    let pair = ok(draft.intern_string("Pair"));
    let x = ok(draft.intern_string("x"));
    let y = ok(draft.intern_string("y"));
    let pair_ty = ok(draft.add_record_type(RecordTypeDef {
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
    }));
    // Resource `W { id:int, kind:Access, owner:Pair }` at record index 1.
    let w = ok(draft.intern_string("W"));
    let idn = ok(draft.intern_string("id"));
    let kindn = ok(draft.intern_string("kind"));
    let ownern = ok(draft.intern_string("owner"));
    let record = ok(draft.add_record_type(RecordTypeDef {
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
                    idx: EnumId::from_index(0),
                    optional: false,
                },
                required: true,
            },
            FieldDef {
                name: ownern,
                ty: ImageType::Record {
                    idx: pair_ty,
                    optional: false,
                },
                required: false,
            },
        ],
    }));
    let root = ok(draft.intern_string("ws"));
    draft.set_application_identity(ledger([0x0a; 16]));
    let product = ledger([0x0d; 16]);
    let int_value = ok(draft.value_scalar(Scalar::Int));
    let text_value = ok(draft.value_scalar(Scalar::Text));
    // An `Option[int]`-shaped enum and a dense `struct { int, text }`.
    let enum_value = draft
        .value_enum(
            ledger([0x50; 16]),
            vec![
                (ledger([0x51; 16]), Vec::new()),
                (ledger([0x52; 16]), vec![("value".into(), int_value)]),
            ],
        )
        .expect("a within-bounds shape appends");
    let struct_value = draft
        .value_struct(vec![("x".into(), int_value), ("y".into(), text_value)])
        .expect("a within-bounds shape appends");
    draft
        .declare_product(
            &admitted_plan(),
            product,
            record,
            vec![
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: ledger([0x0e; 16]),
                        required: true,
                        value: int_value,
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: ledger([0x0f; 16]),
                        required: true,
                        value: enum_value,
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: ledger([0x10; 16]),
                        required: false,
                        value: struct_value,
                    },
                },
            ],
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            product,
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: ledger([0x0c; 16]),
                }],
                placement: ledger([0x0b; 16]),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let zero = ok(draft.intern_int(0));
    let code = vec![Instr::ConstLoad(zero), Instr::Return];
    let func = add_int_fn(&mut draft, "label", code);
    draft.add_export(ExportId::of_local("", "label"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_widened_durable_images_never_panic_the_verifier() {
    mutation_fuzz(a_widened_durable_image(), 0x94D0_49BB_1331_11EB, "widened");
}

/// A good durable image over a flat root with several scalar fields, so its site
/// table carries the whole-payload site plus one field-leaf site per field. Mutating
/// it concentrates on the site-path decoder — the per-site step count, per-step
/// ledger-kind byte and 16-byte id, the target-kind byte, and the resolution of each
/// path against the reconstructed node set — that a one- or two-site image barely
/// exercises.
fn a_multi_site_durable_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner.begin_transaction();
    let rec = ok(draft.intern_string("Row"));
    let mut field_defs = Vec::new();
    for name in ["a", "b", "c", "d"] {
        let field = draft.intern_string(name).expect("a within-domain mint");
        field_defs.push(FieldDef {
            name: field,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        });
    }
    let record = ok(draft.add_record_type(RecordTypeDef {
        name: rec,
        fields: field_defs,
    }));
    let root = ok(draft.intern_string("rows"));
    let application = ledger([0x0a; 16]);
    let placement = ledger([0x0b; 16]);
    draft.set_application_identity(application);
    let field_ids = [
        ledger([0x0e; 16]),
        ledger([0x0f; 16]),
        ledger([0x1e; 16]),
        ledger([0x1f; 16]),
    ];
    let product = ledger([0x0d; 16]);
    let int_value = ok(draft.value_scalar(Scalar::Int));
    draft
        .declare_product(
            &admitted_plan(),
            product,
            record,
            field_ids
                .iter()
                .map(|id| DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: *id,
                        required: true,
                        value: int_value,
                    },
                })
                .collect(),
        )
        .expect("a well-formed declaration");
    let occurrence = draft
        .add_root_occurrence(
            &admitted_plan(),
            product,
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: ledger([0x0c; 16]),
                }],
                placement,
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    let members = draft.product_members(product).expect("declared");
    site(
        &mut draft,
        occurrence.occurrence(),
        occurrence.placement_path(),
        SemanticTarget::WholePayload,
    );
    for member in &members {
        site(
            &mut draft,
            occurrence.occurrence(),
            member.path(),
            SemanticTarget::FieldLeaf,
        );
    }
    let zero = ok(draft.intern_int(0));
    let code = vec![Instr::ConstLoad(zero), Instr::Return];
    let func = add_int_fn(&mut draft, "label", code);
    draft.add_export(ExportId::of_local("", "label"), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn mutated_multi_site_durable_images_never_panic_the_verifier() {
    mutation_fuzz(
        a_multi_site_durable_image(),
        0x1D87_2B41_09CC_5E2F,
        "multi-site",
    );
}
