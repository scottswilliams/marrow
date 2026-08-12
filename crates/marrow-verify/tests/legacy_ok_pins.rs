//! Structural Ok-pins: drafts the producer ENCODES today although the reference they
//! carry answers to no table row, leaving the independent verifier as the only owner
//! that refuses them.
//!
//! Each case pins both halves of that split — `encode()` returns `Ok` and
//! `verify(&bytes)` returns `Err` — so the coherence hoist has an executable baseline
//! for the producer-side conversion. Each clean twin is asserted to verify, proving the
//! rejection comes from the one defect and not from the fixture's shape.

use marrow_image::{
    EncodedImage, ExportId, FuncId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry,
};
use marrow_verify::verify;

/// A minimal exported `main`: one function, one constant, one export, no durable graph.
fn main_draft(params: Vec<ImageType>, code: Vec<Instr>) -> ImageDraft {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    draft.intern_int(0);
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            local_count: params.len() as u16,
            params,
            ret: ImageType::scalar(Scalar::Int),
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    draft
}

fn short_code() -> Vec<Instr> {
    vec![Instr::ConstLoad(0), Instr::Return]
}

fn clean_image() -> EncodedImage {
    main_draft(Vec::new(), short_code())
        .encode()
        .expect("the clean twin encodes")
}

/// A function index no row of a one-function draft answers, minted by a draft that
/// holds two: a `FuncId` is a table position, not a capability bound to its draft.
fn forged_func_id() -> FuncId {
    let mut other = ImageDraft::new();
    let src = other.intern_string("s");
    let name = other.intern_string("f");
    other.intern_int(0);
    let def = FunctionDef {
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
        code: vec![Instr::ConstLoad(0), Instr::Return],
    };
    other
        .add_function(def.clone())
        .expect("every site operand is live");
    other.add_function(def).expect("every site operand is live")
}

/// The clean twin the four pins below vary one reference from.
#[test]
fn the_clean_twin_verifies() {
    let outcome = verify(&clean_image().bytes);
    assert!(outcome.is_ok(), "{outcome:?}");
}

/// The coherence hoist will convert this Ok to `InvalidReference("call target")`; the
/// flip must cite this pin: today a `Call` naming no function row still encodes, and
/// only the verifier refuses the bytes.
#[test]
fn an_out_of_range_call_target_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(Vec::new(), vec![Instr::Call(u16::MAX), Instr::Return])
        .encode()
        .expect("the producer accepts the unanswered call target today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("export target")`; the
/// flip must cite this pin: today an export naming no function row still encodes, and
/// only the verifier refuses the bytes.
#[test]
fn an_out_of_range_export_target_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    draft.add_export(ExportId::of_local("", "ghost"), forged_func_id());
    let image = draft
        .encode()
        .expect("the producer accepts the unanswered export target today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("test target")`; the
/// flip must cite this pin: today a test entry naming no function row still encodes,
/// and only the verifier refuses the bytes.
#[test]
fn an_out_of_range_test_entry_target_encodes_today_and_only_the_verifier_rejects() {
    let mut draft = main_draft(Vec::new(), short_code());
    let entry_name = draft.intern_string("t");
    draft.add_test_entry(entry_name, forged_func_id());
    let image = draft
        .encode()
        .expect("the producer accepts the unanswered test target today");
    assert!(verify(&image.bytes).is_err());
}

/// The coherence hoist will convert this Ok to `InvalidReference("type table")`; the
/// flip must cite this pin: today a parameter type naming no TYPES row still encodes
/// (`ImageType::Record` is publicly constructible over any raw index), and only the
/// verifier refuses the bytes.
#[test]
fn an_out_of_range_param_type_encodes_today_and_only_the_verifier_rejects() {
    let image = main_draft(
        vec![ImageType::Record {
            idx: u16::MAX,
            optional: false,
        }],
        short_code(),
    )
    .encode()
    .expect("the producer accepts the unanswered type index today");
    assert!(verify(&image.bytes).is_err());
}
