//! Ceiling N/N+1 reds for the measure core: for each selector whose policy-clean
//! reachable domain straddles the whole-image ceiling, a fitting exact-N corpus that
//! emits exact bytes and an over corpus the measured plan refuses decisively —
//! before any sort scratch, section body, tail, output, or contract hash exists.
//!
//! The no-emission-artifact claim is carried structurally, not by a probe: the four
//! affine steps make emission unreachable from a measurement refusal (`measure()`
//! must return a plan before `emit_image` exists to call), the reachable
//! post-assembly ImageTooLarge is deleted, and the absence gates pin the closed
//! access set — so `Err(ImageTooLarge)` from `encode()` *is* the observation that
//! measurement, not emission, decided. The wall-clock guards below are the
//! linearity tripwires the phase requires: decisive capped work only, never the
//! mathematical full total.

use std::time::{Duration, Instant};

use marrow_image::bounds::{MAX_FUNCTIONS, MAX_IMAGE_BYTES, MAX_STRING_BYTES, MAX_TEST_ENTRIES};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, ExportId, FunctionDef, ImageBuildError,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootOccurrenceDef,
    Scalar, SpanEntry,
};

#[path = "common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

#[path = "common/admitted.rs"]
mod admitted_helper;
use admitted_helper::admitted;

/// A minimal clean storeless draft: one exported `main`, one constant.
fn storeless_base() -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let zero = draft.intern_int(0).expect("a within-domain mint");
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
            code: vec![Instr::ConstLoad(zero), Instr::Return],
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.commit();
    draft_owner
}

/// The base draft plus filler strings whose STRINGS rows land the assembled image at
/// exactly `target` bytes. Each interned string contributes exactly `2 + len` body
/// bytes wherever the sort places it, so the arithmetic is exact rather than a
/// search.
fn string_corpus(target: usize) -> ImageDraft {
    let mut owner = storeless_base();
    let base = owner.encode().expect("the base draft encodes").bytes.len();
    assert!(target > base, "the target sits above the base image");
    let mut delta = target - base;
    const FULL: usize = 4_000;
    const _: () = assert!(
        FULL < MAX_STRING_BYTES,
        "filler rows stay inside the string cap"
    );
    let mut full_rows = delta / (2 + FULL);
    delta -= full_rows * (2 + FULL);
    // The remainder row needs at least a 6-byte text so its content stays distinct.
    if delta < 8 {
        full_rows -= 1;
        delta += 2 + FULL;
    }
    let mut draft = admitted(&mut owner);
    for index in 0..full_rows {
        draft
            .intern_string(&format!("{index:04}{}", "x".repeat(FULL - 4)))
            .expect("a within-domain mint");
    }
    draft
        .intern_string(&format!("rem-{}", "y".repeat(delta - 2 - 4)))
        .expect("a within-domain mint");
    draft.commit();
    owner
}

/// The STRINGS selector's fitting exact-N corpus: the assembled image lands on the
/// ceiling byte exactly, emits, and carries its digest.
#[test]
fn the_string_selector_emits_at_exactly_the_ceiling() {
    let image = string_corpus(MAX_IMAGE_BYTES)
        .encode()
        .expect("an image of exactly the ceiling fits");
    assert_eq!(image.bytes.len(), MAX_IMAGE_BYTES);
    assert_eq!(
        &image.bytes[5..37],
        image.image_id.0.as_slice(),
        "the embedded digest slot carries the computed ImageId",
    );
}

/// The STRINGS selector's over corpus, one byte past the ceiling: the measured plan
/// refuses it decisively, and the affine step order means no emission artifact —
/// sort scratch, section body, tail, output, or hash — can exist behind the verdict.
#[test]
fn the_string_selector_is_refused_one_byte_past_the_ceiling() {
    assert_eq!(
        string_corpus(MAX_IMAGE_BYTES + 1).encode().map(|_| ()),
        Err(ImageBuildError::ImageTooLarge),
    );
}

/// One coherent `G + M + T = 4,096` function-table partition — instance-shaped rows,
/// monomorphic rows, and unique tests — every body far inside CodeBytes, whose
/// aggregate section crosses the ceiling. Measurement refuses it without any
/// encoder or hash allocation; the corpus's mathematical full total (~840 KiB) is
/// never computed as a retained value.
#[test]
fn the_full_function_partition_is_refused_by_measurement() {
    let started = Instant::now();
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let generic_src = draft
        .intern_string("src/generic.mw")
        .expect("a within-domain mint");
    let mono_src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let test_src = draft
        .intern_string("src/tests.mw")
        .expect("a within-domain mint");
    let g_name = draft
        .intern_string("instance")
        .expect("a within-domain mint");
    let m_name = draft.intern_string("mono").expect("a within-domain mint");
    let zero = draft.intern_int(0).expect("a within-domain mint");
    let body: Vec<Instr> = std::iter::repeat_n(Instr::ConstLoad(zero), 64)
        .chain([Instr::Return])
        .collect();
    let tests = MAX_TEST_ENTRIES;
    let generics = (MAX_FUNCTIONS - tests) / 2;
    let monos = MAX_FUNCTIONS - tests - generics;
    for (count, name, source) in [(generics, g_name, generic_src), (monos, m_name, mono_src)] {
        for _ in 0..count {
            draft
                .add_function(FunctionDef {
                    name,
                    source,
                    params: Vec::new(),
                    ret: ImageType::scalar(Scalar::Int),
                    local_count: 0,
                    spans: Vec::new(),
                    code: body.clone(),
                })
                .expect("every site operand is live");
        }
    }
    for index in 0..tests {
        let name = draft
            .intern_string(&format!("t{index}"))
            .expect("a within-domain mint");
        let func = draft
            .add_function(FunctionDef {
                name,
                source: test_src,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                spans: Vec::new(),
                code: vec![Instr::Return],
            })
            .expect("every site operand is live");
        draft.add_test_entry(name, func);
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::ImageTooLarge),
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the refusal is decisive capped work, never the full total",
    );
}

/// The span-heavy twin: few functions, span tables carrying the bulk, the same
/// decisive measurement refusal.
#[test]
fn the_span_heavy_draft_is_refused_by_measurement() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let zero = draft.intern_int(0).expect("a within-domain mint");
    for index in 0..2 {
        let name = draft
            .intern_string(&format!("f{index}"))
            .expect("a within-domain mint");
        draft
            .add_function(FunctionDef {
                name,
                source: src,
                params: Vec::new(),
                ret: ImageType::scalar(Scalar::Int),
                local_count: 0,
                spans: vec![
                    SpanEntry {
                        instr_index: 0,
                        line: 1,
                        column: 1,
                    };
                    22_000
                ],
                code: vec![Instr::ConstLoad(zero), Instr::Return],
            })
            .expect("every site operand is live");
    }
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::ImageTooLarge),
    );
}

/// A coherent draft whose one function carries `count` spans over a valid
/// instruction.
fn span_count_draft(count: usize) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let zero = draft.intern_int(0).expect("a within-domain mint");
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            spans: vec![
                SpanEntry {
                    instr_index: 0,
                    line: 1,
                    column: 1,
                };
                count
            ],
            code: vec![Instr::ConstLoad(zero), Instr::Return],
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.commit();
    draft_owner
}

/// `u16::MAX` and `u16::MAX + 1` span counts both cross the ceiling and both select
/// the aggregate ImageBytes verdict from measurement — before any wire proof, so no
/// count-prefix narrowing can mask or replace the result.
#[test]
fn u16_boundary_span_counts_select_the_ceiling_before_any_wire_proof() {
    for count in [u16::MAX as usize, u16::MAX as usize + 1] {
        assert_eq!(
            span_count_draft(count).encode().map(|_| ()),
            Err(ImageBuildError::ImageTooLarge),
            "a {count}-span table crosses the ceiling and draws exactly ImageBytes",
        );
    }
}

/// The 31-level compact-expansion regression: one root, one field whose value is a
/// scalar under exactly 31 enclosing record levels, each declaring 64 edges to the
/// one next node — 32 unique nodes, 1,984 declared edges, and a `64^31`-leaf
/// expansion no machine could traverse. Coherence visits the arena's nodes and the
/// declared edges once; measurement expands direct-to-sink only until the decisive
/// ceiling byte. The wall-clock guard is the linearity tripwire: a full-total
/// traversal would not return in this epoch.
#[test]
fn the_compact_expansion_regression_is_refused_decisively() {
    let started = Instant::now();
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    draft.set_application_identity(LedgerIdBytes::from_bytes([0x01; 16]));
    let value = {
        let mut level = draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints");
        for _ in 0..31 {
            level = draft
                .value_struct(vec![level; 64])
                .expect("a within-bounds shape appends");
        }
        level
    };
    let type_name = draft.intern_string("R").expect("a within-domain mint");
    let record = draft
        .add_record_type(RecordTypeDef {
            name: type_name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint");
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes([0x0d; 16]),
            record,
            vec![DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Field {
                    id: LedgerIdBytes::from_bytes([0x0e; 16]),
                    required: true,
                    value,
                },
            }],
        )
        .expect("a well-formed declaration");
    let root_name = draft.intern_string("r").expect("a within-domain mint");
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
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("main").expect("a within-domain mint");
    let zero = draft.intern_int(0).expect("a within-domain mint");
    let main = draft
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
    draft.add_export(ExportId::of_local("", "main"), main);
    assert_eq!(
        draft.encode().map(|_| ()),
        Err(ImageBuildError::ImageTooLarge),
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "coherence visits nodes and edges once and measurement stops at the ceiling",
    );
}

/// A coherent fitting draft at `scale`: functions, record types, and pool strings
/// all scale together, and the whole image stays inside the ceiling at 4x.
fn linear_draft(scale: usize) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let zero = draft.intern_int(0).expect("a within-domain mint");
    let body: Vec<Instr> = std::iter::repeat_n(Instr::ConstLoad(zero), 32)
        .chain([Instr::Return])
        .collect();
    for index in 0..64 * scale {
        let name = draft
            .intern_string(&format!("ty{index}"))
            .expect("a within-domain mint");
        let field = draft
            .intern_string(&format!("fy{index}"))
            .expect("a within-domain mint");
        draft
            .add_record_type(RecordTypeDef {
                name,
                fields: vec![marrow_image::FieldDef {
                    name: field,
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                }],
            })
            .expect("a within-domain mint");
    }
    let mut main = None;
    for index in 0..96 * scale {
        let name = draft
            .intern_string(&format!("f{index}"))
            .expect("a within-domain mint");
        let func = draft
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
                code: body.clone(),
            })
            .expect("every site operand is live");
        main.get_or_insert(func);
    }
    draft.add_export(ExportId::of_local("", "main"), main.expect("one function"));
    draft.commit();
    draft_owner
}

/// The 1x/2x/4x linearity tripwire: coherence work is linear in retained rows plus
/// unique nodes and declared edges, and fitting measurement plus emission is linear
/// in emitted bytes, so quadrupling the input must not cost more than a generous
/// constant times four. The margin (12x, plus an absolute noise floor) makes the
/// guard a superlinearity tripwire rather than a benchmark.
#[test]
fn encode_work_stays_linear_across_1x_2x_4x() {
    let timed = |scale: usize| {
        let draft = linear_draft(scale);
        draft.encode().expect("the linear corpus fits");
        let started = Instant::now();
        for _ in 0..10 {
            draft.encode().expect("the linear corpus fits");
        }
        started.elapsed()
    };
    let base = timed(1);
    let quad = timed(4);
    assert!(
        quad < base * 12 + Duration::from_millis(250),
        "4x input must stay near 4x the 1x work: 1x={base:?}, 4x={quad:?}",
    );
    let double = timed(2);
    assert!(
        double < base * 6 + Duration::from_millis(250),
        "2x input must stay near 2x the 1x work: 1x={base:?}, 2x={double:?}",
    );
}
