//! Slice K.2 evidence: scalars and procedural control through the sealed tape.
//!
//! Images are minted with `ImageDraft`, sealed by the independent verifier, and run
//! on the VM — the executable trust path without the compiler. The checked-
//! arithmetic known-answer tests (`i64::MIN % -1` and `IntNeg(i64::MIN)`) and the
//! structural rejections (unreachable code, fall-off-end, return-type mismatch) live
//! here beside the machine that owns them.

use marrow_image::{ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry};
use marrow_verify::verify;
use marrow_vm::{Value, run};

/// Encode a one-function image `f(): ret` built by `build`, returning its bytes.
fn encode(build: impl FnOnce(&mut ImageDraft) -> (ImageType, Vec<Instr>)) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let name = draft.intern_string("f");
    let source = draft.intern_string("src/main.mw");
    let (ret, code) = build(&mut draft);
    let spans = (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect();
    let func = draft.add_function(FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret,
        local_count: 2,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.encode().expect("encode").bytes
}

/// Seal `f(): ret`, returning the phase code on a verifier rejection.
fn seal(build: impl FnOnce(&mut ImageDraft) -> (ImageType, Vec<Instr>)) -> Result<(), String> {
    verify(&encode(build))
        .map(|_| ())
        .map_err(|rejection| rejection.code().to_string())
}

/// Build, verify, and run `f(): ret`, returning either its value or the typed code
/// of the verifier rejection / runtime fault.
fn build_and_run(
    build: impl FnOnce(&mut ImageDraft) -> (ImageType, Vec<Instr>),
) -> Result<Option<Value>, String> {
    let bytes = encode(build);
    let image = verify(&bytes).map_err(|rejection| rejection.code().to_string())?;
    let index = image
        .export_by_id(ExportId::of_local("", "f"))
        .expect("export present")
        .function();
    run(&image, index, Vec::new()).map_err(|fault| fault.code().to_string())
}

#[test]
fn locals_and_arithmetic_compute_a_value() {
    // let a = 6; let b = 7; return a * b  == 42
    let result = build_and_run(|draft| {
        let six = draft.intern_int(6);
        let seven = draft.intern_int(7);
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::ConstLoad(six.index()),
                Instr::LocalSet(0),
                Instr::ConstLoad(seven.index()),
                Instr::LocalSet(1),
                Instr::LocalGet(0),
                Instr::LocalGet(1),
                Instr::IntMul,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Ok(Some(Value::Int(42))));
}

#[test]
fn int_min_rem_negative_one_faults_overflow() {
    let result = build_and_run(|draft| {
        let min = draft.intern_int(i64::MIN);
        let neg_one = draft.intern_int(-1);
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::ConstLoad(min.index()),
                Instr::ConstLoad(neg_one.index()),
                Instr::IntRem,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Err("run.overflow".to_string()));
}

#[test]
fn neg_int_min_faults_overflow() {
    let result = build_and_run(|draft| {
        let min = draft.intern_int(i64::MIN);
        (
            ImageType::scalar(Scalar::Int),
            vec![Instr::ConstLoad(min.index()), Instr::IntNeg, Instr::Return],
        )
    });
    assert_eq!(result, Err("run.overflow".to_string()));
}

#[test]
fn rem_by_zero_faults_divide_by_zero() {
    let result = build_and_run(|draft| {
        let five = draft.intern_int(5);
        let zero = draft.intern_int(0);
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::ConstLoad(five.index()),
                Instr::ConstLoad(zero.index()),
                Instr::IntRem,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Err("run.divide_by_zero".to_string()));
}

#[test]
fn text_concat_over_the_limit_faults() {
    // A single text constant caps at 4 KiB, so reach the 64 KiB result bound by
    // doubling an accumulator: 4K → 8K → 16K → 32K → 64K → (128K faults).
    let seed = "x".repeat(4 * 1024);
    let result = build_and_run(move |draft| {
        let a = draft.intern_text(&seed);
        let mut code = vec![Instr::ConstLoad(a.index()), Instr::LocalSet(0)];
        for _ in 0..5 {
            code.push(Instr::LocalGet(0));
            code.push(Instr::LocalGet(0));
            code.push(Instr::TextConcat);
            code.push(Instr::LocalSet(0));
        }
        code.push(Instr::LocalGet(0));
        code.push(Instr::Return);
        (ImageType::scalar(Scalar::Text), code)
    });
    assert_eq!(result, Err("run.text_limit".to_string()));
}

#[test]
fn unreachable_instruction_rejects() {
    // A second Return after the first is never reached.
    let rejection = seal(|draft| {
        let one = draft.intern_int(1);
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::ConstLoad(one.index()),
                Instr::Return,
                Instr::ConstLoad(one.index()),
                Instr::Return,
            ],
        )
    });
    assert_eq!(rejection.err(), Some("image.function".to_string()));
}

#[test]
fn falling_off_the_end_rejects() {
    // No Return: control falls off the end.
    let rejection = seal(|draft| {
        let one = draft.intern_int(1);
        (
            ImageType::scalar(Scalar::Int),
            vec![Instr::ConstLoad(one.index())],
        )
    });
    assert_eq!(rejection.err(), Some("image.function".to_string()));
}

#[test]
fn return_type_mismatch_rejects() {
    // Returns a bool where the return type is int.
    let rejection = seal(|draft| {
        let flag = draft.intern_bool(true);
        (
            ImageType::scalar(Scalar::Int),
            vec![Instr::ConstLoad(flag.index()), Instr::Return],
        )
    });
    assert_eq!(rejection.err(), Some("image.function".to_string()));
}
