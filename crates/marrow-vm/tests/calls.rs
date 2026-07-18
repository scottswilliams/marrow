//! Slice K.4 evidence: direct calls, the acyclic-call-graph rejection, and the
//! dynamic call-depth guard.

use marrow_image::{ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry};
use marrow_verify::{FunctionIndex, verify};
use marrow_vm::{Value, run};

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

#[test]
fn a_direct_call_runs() {
    // double(n) = n + n ; caller() = double(21) == 42
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let double_name = draft.intern_string("double");
    let double_code = vec![
        Instr::LocalGet(0),
        Instr::LocalGet(0),
        Instr::IntAdd,
        Instr::Return,
    ];
    let double = draft.add_function(FunctionDef {
        name: double_name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::scalar(Scalar::Int),
        local_count: 1,
        spans: spans(&double_code),
        code: double_code,
    });
    let caller_name = draft.intern_string("caller");
    let arg = draft.intern_int(21);
    let caller_code = vec![
        Instr::ConstLoad(arg.index()),
        Instr::Call(double.index()),
        Instr::Return,
    ];
    let caller = draft.add_function(FunctionDef {
        name: caller_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&caller_code),
        code: caller_code,
    });
    draft.add_export(ExportId::of_local("", "caller"), caller);
    let bytes = draft.encode().expect("encode").bytes;
    let image = verify(&bytes).expect("verifies");
    let index = image
        .export_by_id(ExportId::of_local("", "caller"))
        .expect("export")
        .function();
    assert_eq!(run(&image, index, Vec::new()), Ok(Some(Value::Int(42))));
}

/// The VM's run entry addresses a function by a typed [`FunctionIndex`], not a bare
/// `u16`: the only image-blessed source is [`marrow_verify::SealedExport::function`],
/// and the newtype round-trips through `get`/`new` while staying distinct from the
/// many other `u16` handles a sealed image carries. A raw integer (a local slot, a
/// const index) cannot be presented to `run` in its place — that is a compile error,
/// which is the boundary this test documents.
#[test]
fn the_vm_run_entry_takes_a_typed_function_index() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("answer");
    let forty_two = draft.intern_int(42);
    let code = vec![Instr::ConstLoad(forty_two.index()), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "answer"), func);
    let bytes = draft.encode().expect("encode").bytes;
    let image = verify(&bytes).expect("verifies");

    let index: FunctionIndex = image
        .export_by_id(ExportId::of_local("", "answer"))
        .expect("export")
        .function();

    // The typed index addresses the right function.
    assert_eq!(image.function(index).name(), "answer");

    // The newtype round-trips, and a value reconstructed through `new` addresses the
    // same function the export named — the only sanctioned way to build one by hand.
    let rebuilt = FunctionIndex::new(index.get());
    assert_eq!(rebuilt, index);
    assert_eq!(rebuilt.index(), index.get() as usize);
    assert_eq!(run(&image, rebuilt, Vec::new()), Ok(Some(Value::Int(42))));
}

#[test]
fn a_self_recursive_call_rejects_as_a_cycle() {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("loops");
    let code = vec![Instr::Call(0), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "loops"), func);
    let bytes = draft.encode().expect("encode").bytes;
    assert_eq!(
        verify(&bytes).err().map(|r| r.code().to_string()),
        Some("image.closure".to_string())
    );
}

// The dynamic call-depth guard (64) is defensive: with at most 64 functions and no
// recursion (rejected as a cycle), an acyclic call chain is at most 63 deep, so the
// guard is unreachable at this subset. It is retained in the VM to match the design
// and to bound a future subset with more functions; there is no reachable test for
// it here.
