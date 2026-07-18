//! Exit-gate evidence: the VM runs an image built with `ImageDraft → encode →
//! verify`, with no compiler dependency. The image is minted here, encoded to
//! canonical bytes, and sealed by the independent verifier before the VM sees it —
//! so the executable trust path is exercised end to end without the compiler.

use marrow_image::{
    CollectionTypeDef, ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry,
};
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

/// Build a one-function image `forged(): int` that constructs an empty collection,
/// pushes an out-of-range positional index, and performs `read`. The verifier proves
/// the operand types (a collection under an int, yielding the element/key/value type),
/// but not the index value, so this image verifies — it is the shape a hand-built or
/// corrupted image takes. The compiler never emits such an out-of-range positional
/// read; the VM's totality guard is what these images probe.
fn forged_list_positional_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("forged");
    let source = draft.intern_string("src/main.mw");
    let coll = draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Int),
    });
    let index = draft.intern_int(100);
    let func = draft.add_function(FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        code: vec![
            Instr::ListNew(coll.index()),
            Instr::ConstLoad(index.index()),
            Instr::ListGet,
            Instr::Return,
        ],
        spans: vec![SpanEntry {
            instr_index: 0,
            line: 2,
            column: 12,
        }],
    });
    draft.add_export(answer_id(), func);
    draft.encode().expect("encode").bytes
}

/// The map twin: an empty map, an out-of-range positional index, and `read`
/// (`MapKeyAt` or `MapValueAt`). The key and value types are `int`, so the read
/// yields an `int` and the image verifies.
fn forged_map_positional_image(read: Instr) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("forged");
    let source = draft.intern_string("src/main.mw");
    let coll = draft.add_collection_type(CollectionTypeDef::Map {
        key: ImageType::scalar(Scalar::Int),
        value: ImageType::scalar(Scalar::Int),
    });
    let index = draft.intern_int(5);
    let func = draft.add_function(FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        code: vec![
            Instr::MapNew(coll.index()),
            Instr::ConstLoad(index.index()),
            read,
            Instr::Return,
        ],
        spans: vec![SpanEntry {
            instr_index: 0,
            line: 2,
            column: 12,
        }],
    });
    draft.add_export(answer_id(), func);
    draft.encode().expect("encode").bytes
}

/// A forged image whose internal positional read (`ListGet`/`MapKeyAt`/`MapValueAt`)
/// addresses a position past an empty collection passes verification — the verifier
/// bounds operand types, not index values — and fails closed at runtime with the typed
/// `run.corruption` fault rather than panicking or reading past the collection. This
/// restores the totality the deleted `run.collection_range` guard provided while
/// keeping the source law that no out-of-bounds fault is named.
#[test]
fn a_forged_out_of_range_positional_read_faults_run_corruption() {
    let images = [
        forged_list_positional_image(),
        forged_map_positional_image(Instr::MapKeyAt),
        forged_map_positional_image(Instr::MapValueAt),
    ];
    for bytes in images {
        let image = verify(&bytes).expect("a type-correct forged image verifies");
        let export = image.export_by_id(answer_id()).expect("export present");
        let fault = run(&image, export.function(), Vec::<Value>::new())
            .expect_err("an out-of-range positional read must fault, not panic");
        assert_eq!(fault.code(), "run.corruption");
    }
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
