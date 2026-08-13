//! Verifier value-shape scaling: what a decoded image's value shapes cost the
//! independent verifier as the corpus that references them grows.
//!
//! A durable field's value shape is a reference into the decoded image's one
//! value-shape arena. Two corpora scale the thing the arena is meant to be linear in —
//! unique value nodes plus declared edges — at 1x, 2x, and 4x, and pin two facts about
//! what that costs the independent verifier:
//!
//! 1. **The wire is linear.** A shared shape is spelled once per occurrence and the
//!    occurrences are what the corpus scales, so quadrupling the corpus quadruples the
//!    encoded image. This is deterministic and is the real subject: a representation
//!    that rebuilt a shape per member, or that carried an expanded tree, would show it
//!    here as growth the declared graph does not explain.
//! 2. **Verification stays inside a flat budget.** The 4x corpus verifies well inside a
//!    budget two orders of magnitude above its linear cost, so a rebuild-per-member or
//!    an expansion-per-occurrence — either of which is quadratic in these corpora —
//!    cannot pass.
//!
//! The deleted half of the same red — the per-member value-shape clone the verifier
//! used to take while rebuilding the descriptor — is enforced structurally by the
//! absence gate `the_verifier_holds_no_raw_durable_value_tree`, not measured here.

use std::time::{Duration, Instant};

use marrow_image::{
    CanonicalValueShapeDag, DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId,
    FieldDef, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef,
    RootOccurrenceDef, Scalar, SpanEntry, TypeId, ValueShapeNodeId,
};
use marrow_verify::verify;

#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const ROOT_KEY_ID: [u8; 16] = [0x0c; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];

/// A distinct field ledger id per member ordinal.
fn field_id(ordinal: usize) -> LedgerIdBytes {
    let mut bytes = [0x20u8; 16];
    bytes[0] = ordinal as u8;
    bytes[1] = (ordinal >> 8) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// The value shape and matching record type of one dense struct level.
#[derive(Clone, Copy)]
struct Level {
    shape: ValueShapeNodeId,
    ty: ImageType,
}

/// The base struct level `{ v: int, w: string }`: one value-shape node over two
/// scalars, and the materialized record the durable field's declared type must be.
fn base_level(draft: &mut DraftTxn<'_>) -> Level {
    let v = draft.intern_string("v");
    let w = draft.intern_string("w");
    let name = draft.intern_string("S0");
    let record = draft.add_record_type(RecordTypeDef {
        name,
        fields: vec![
            FieldDef {
                name: v,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: w,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
        ],
    });
    let int = draft.value_scalar(Scalar::Int);
    let text = draft.value_scalar(Scalar::Text);
    Level {
        shape: draft.value_struct(vec![int, text]).expect("a within-bounds shape appends"),
        ty: record_type(record),
    }
}

/// One more enclosing level over `inner`: a single-leaf struct, so the chain's depth
/// grows by one per level while its expansion grows by one node.
fn enclosing_level(draft: &mut DraftTxn<'_>, ordinal: usize, inner: Level) -> Level {
    let field = draft.intern_string("inner");
    let name = draft.intern_string(&format!("S{ordinal}"));
    let record = draft.add_record_type(RecordTypeDef {
        name,
        fields: vec![FieldDef {
            name: field,
            ty: inner.ty,
            required: true,
        }],
    });
    Level {
        shape: draft.value_struct(vec![inner.shape]).expect("a within-bounds shape appends"),
        ty: record_type(record),
    }
}

fn record_type(record: TypeId) -> ImageType {
    ImageType::Record {
        idx: record,
        optional: false,
    }
}

/// Encode one root over a Product whose direct members are `levels[i % levels.len()]`
/// for each of `fields` ordinals, plus the materialized entry record those members tie
/// to. The value graph is exactly `levels`; the declared edges are `fields`.
fn encode_corpus(fields: usize, levels: &dyn Fn(&mut DraftTxn<'_>) -> Vec<Level>) -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = draft_owner
        .begin_transaction(draft_owner.savepoint())
        .expect("a fresh savepoint admits");
    let levels = levels(&mut draft);
    let entry_name = draft.intern_string("R");
    let entry_fields: Vec<FieldDef> = (0..fields)
        .map(|ordinal| FieldDef {
            name: draft.intern_string(&format!("f{ordinal}")),
            ty: levels[ordinal % levels.len()].ty,
            required: true,
        })
        .collect();
    let entry = draft.add_record_type(RecordTypeDef {
        name: entry_name,
        fields: entry_fields,
    });
    let members: Vec<DeclarationMemberDef> = (0..fields)
        .map(|ordinal| DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: field_id(ordinal),
                required: true,
                value: levels[ordinal % levels.len()].shape,
            },
        })
        .collect();
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            entry,
            members,
        )
        .expect("a well-formed declaration");
    let root_name = draft.intern_string("counters");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");
    add_main(&mut draft);
    draft.encode().expect("the corpus fits every bound").bytes
}

fn add_main(draft: &mut DraftTxn<'_>) {
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let zero = draft.intern_int(0);
    let code = vec![Instr::ConstLoad(zero), Instr::Return];
    let spans = (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect();
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            spans,
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
}

/// The repeated-subshape corpus: every durable field references the one base level, so
/// the value graph is three nodes whatever the field count.
fn repeated_subshape(fields: usize) -> Vec<u8> {
    encode_corpus(fields, &|draft| vec![base_level(draft)])
}

/// The deep-diamond corpus: a 24-level chain over the base level, with the fields
/// cycling through every level of it. Each level is reached from several fields at
/// several depths, and the deepest field's value is 26 levels deep — inside
/// `MAX_DURABLE_VALUE_DEPTH` — while the whole graph is 26 nodes.
fn deep_diamond(fields: usize) -> Vec<u8> {
    encode_corpus(fields, &|draft| {
        let mut levels = vec![base_level(draft)];
        for ordinal in 1..=24 {
            let inner = levels[ordinal - 1];
            levels.push(enclosing_level(draft, ordinal, inner));
        }
        levels
    })
}

/// The wall-clock budget for verifying the 4x corpus. Its linear cost is under a
/// millisecond; the budget is far above that and far below the cost of any accounting
/// quadratic in these corpora.
const VERIFY_BUDGET: Duration = Duration::from_secs(5);

/// Quadrupling the declared edges quadruples the image and leaves verification inside
/// a flat budget. The tolerance is on the ratio, not the byte count: the corpora carry
/// a fixed header, a fixed value graph, and per-field string names, so the growth is
/// linear with a constant, never the corpus size squared.
#[test]
fn value_shape_work_scales_with_the_declared_graph() {
    for (name, corpus) in [
        (
            "repeated subshape",
            &repeated_subshape as &dyn Fn(usize) -> Vec<u8>,
        ),
        ("deep diamond", &deep_diamond),
    ] {
        let mut sizes = Vec::new();
        for fields in [256usize, 512, 1024] {
            let image = corpus(fields);
            let started = Instant::now();
            verify(&image).unwrap_or_else(|rejection| {
                panic!("{name} at {fields} fields must verify, got {rejection:?}")
            });
            let elapsed = started.elapsed();
            assert!(
                elapsed < VERIFY_BUDGET,
                "{name} at {fields} fields verified in {elapsed:?}, over the \
                 {VERIFY_BUDGET:?} budget",
            );
            sizes.push(image.len());
        }
        let growth = sizes[2] as f64 / sizes[0] as f64;
        assert!(
            (3.5..=4.5).contains(&growth),
            "{name}: 4x the declared edges encoded {growth:.2}x the bytes {sizes:?} — \
             the wire is not linear in the declared graph",
        );
    }
}

/// A shared shape costs one arena node however many fields reference it, and reaching
/// it at several depths does not multiply it. This is the retained-size claim the byte
/// growth above is evidence about, stated directly against the arena.
#[test]
fn a_shared_value_shape_is_one_node_however_many_fields_reference_it() {
    let mut values = CanonicalValueShapeDag::new();
    let int = values.scalar(Scalar::Int);
    let text = values.scalar(Scalar::Text);
    let base = values.struct_shape(vec![int, text]);
    let mut level = base;
    for _ in 0..24 {
        level = values.struct_shape(vec![level]);
    }
    let before = values.len();
    for _ in 0..1024 {
        let repeat = values.struct_shape(vec![int, text]);
        assert_eq!(repeat, base);
    }
    assert_eq!(
        values.len(),
        before,
        "a repeated shape mints no second node"
    );
    assert_eq!(before, 27, "two scalars, the base struct, and 24 levels");
}
