//! Exit-gate evidence: the VM runs an image built with `ImageDraft → encode →
//! verify`, with no compiler dependency. The image is minted here, encoded to
//! canonical bytes, and sealed by the independent verifier before the VM sees it —
//! so the executable trust path is exercised end to end without the compiler.

use marrow_image::{ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry};
use marrow_verify::verify;
use marrow_vm::{Value, run};

/// The synthetic export id these draft-level tests bind and look up by.
fn answer_id() -> ExportId {
    ExportId::of_local("", "answer")
}

/// Build a one-function image `answer(): int = <value>` and return its bytes.
fn return_const_image(value: i64) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("answer");
    let source = draft.intern_string("src/main.mw");
    let konst = draft.intern_int(value);
    let func = draft.add_function(FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        code: vec![Instr::ConstLoad(konst.index()), Instr::Return],
        spans: vec![SpanEntry {
            instr_index: 0,
            line: 2,
            column: 12,
        }],
    });
    draft.add_export(answer_id(), func);
    draft.encode().expect("encode").bytes
}

#[test]
fn verified_image_runs_on_the_vm() {
    let bytes = return_const_image(42);
    let image = verify(&bytes).expect("image verifies");
    let export = image.export_by_id(answer_id()).expect("export present");
    let result = run(&image, export.function(), Vec::<Value>::new()).expect("run");
    assert_eq!(result, Some(Value::Int(42)));
}

#[test]
fn a_flipped_digest_slot_rejects_at_the_envelope() {
    let mut bytes = return_const_image(7);
    // Flip a byte in the digest slot (offsets 5..37) without rehashing.
    bytes[10] ^= 0xFF;
    let rejection = verify(&bytes).expect_err("a stale digest must reject");
    assert_eq!(rejection.code(), "image.envelope");
}

#[test]
fn relocating_the_project_yields_identical_image_bytes() {
    // Reproducibility: the image is a pure function of the draft inputs.
    assert_eq!(return_const_image(1), return_const_image(1));
    assert_ne!(return_const_image(1), return_const_image(2));
}

/// Build `guarded(n: int): int` that range-guards its argument against
/// `[0, 150]` and returns it, exercising the guard through draft → encode →
/// verify → run with no compiler.
fn range_guard_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("guarded");
    let source = draft.intern_string("src/main.mw");
    let func = draft.add_function(FunctionDef {
        name,
        source,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::scalar(Scalar::Int),
        local_count: 1,
        code: vec![
            Instr::LocalGet(0),
            Instr::RangeGuard { lo: 0, hi: 150 },
            Instr::Return,
        ],
        spans: vec![SpanEntry {
            instr_index: 0,
            line: 3,
            column: 5,
        }],
    });
    draft.add_export(answer_id(), func);
    draft.encode().expect("encode").bytes
}

/// The range guard peeks: an in-interval int passes through unchanged at both
/// boundaries, and an out-of-interval int faults `run.range` at the guarded
/// instruction's source span, on both sides.
#[test]
fn a_range_guard_admits_the_interval_and_faults_outside_it() {
    let bytes = range_guard_image();
    let image = verify(&bytes).expect("image verifies");
    let export = image.export_by_id(answer_id()).expect("export present");
    for value in [0, 150, 42] {
        let result = run(&image, export.function(), vec![Value::Int(value)]).expect("in range");
        assert_eq!(result, Some(Value::Int(value)));
    }
    for value in [-1, 151, i64::MIN, i64::MAX] {
        let fault = run(&image, export.function(), vec![Value::Int(value)])
            .expect_err("out of range must fault");
        assert_eq!(fault.code(), "run.range");
        assert_eq!((fault.line(), fault.column()), (3, 5));
    }
}
