//! Checked function selection, direct calls, cycle rejection and call depth.

use marrow_image::{ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry};
use marrow_verify::{FunctionIndex, VerifiedImage, verify};
use marrow_vm::{Value, run};

#[path = "common/admitted.rs"]
mod admitted_helper;
use admitted_helper::admitted;

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

fn direct_call_image(argument: i64, operation: Instr) -> VerifiedImage {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let helper_name = draft.intern_string("helper").expect("a within-domain mint");
    let helper_code = vec![
        Instr::LocalGet(0),
        Instr::LocalGet(0),
        operation,
        Instr::Return,
    ];
    let helper = draft
        .add_function(FunctionDef {
            name: helper_name,
            source: src,
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::scalar(Scalar::Int),
            local_count: 1,
            spans: spans(&helper_code),
            code: helper_code,
        })
        .expect("every site operand is live");
    let caller_name = draft.intern_string("caller").expect("a within-domain mint");
    let arg = draft.intern_int(argument).expect("a within-domain mint");
    let caller_code = vec![
        Instr::ConstLoad(arg),
        Instr::Call(helper.index()),
        Instr::Return,
    ];
    let caller = draft
        .add_function(FunctionDef {
            name: caller_name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            spans: spans(&caller_code),
            code: caller_code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "caller"), caller);
    let bytes = draft.encode().expect("encode").bytes;
    verify(&bytes).expect("verifies")
}

#[test]
fn a_direct_call_runs() {
    let image = direct_call_image(21, Instr::IntAdd);
    let index = image
        .export_by_id(ExportId::of_local("", "caller"))
        .expect("export")
        .function();
    let function = image.function(index).expect("verified export function");
    assert_eq!(run(function, Vec::new()), Ok(Some(Value::Int(42))));
}

#[test]
fn absent_function_ordinals_are_refused_without_panicking() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("answer").expect("a within-domain mint");
    let forty_two = draft.intern_int(42).expect("a within-domain mint");
    let code = vec![Instr::ConstLoad(forty_two), Instr::Return];
    let func = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "answer"), func);
    let bytes = draft.encode().expect("encode").bytes;
    let image = verify(&bytes).expect("verifies");

    let index: FunctionIndex = image
        .export_by_id(ExportId::of_local("", "answer"))
        .expect("export")
        .function();

    let selected = image.function(index).expect("verified export function");
    assert_eq!(selected.body().name(), "answer");
    assert_eq!(run(selected, Vec::new()), Ok(Some(Value::Int(42))));

    let foreign_image = direct_call_image(7, Instr::IntMul);
    let higher = foreign_image
        .export_by_id(ExportId::of_local("", "caller"))
        .expect("export")
        .function();
    assert_eq!(higher.index(), image.functions().len());
    for absent in [FunctionIndex::new(u16::MAX), higher] {
        assert!(image.function(absent).is_none(), "ordinal {}", absent.get());
    }
}

#[test]
fn selections_keep_their_image_through_constants_and_helper_calls() {
    let first = direct_call_image(21, Instr::IntAdd);
    let second = direct_call_image(7, Instr::IntMul);
    let ordinal = first
        .export_by_id(ExportId::of_local("", "caller"))
        .expect("first export")
        .function();
    assert_eq!(
        ordinal,
        second
            .export_by_id(ExportId::of_local("", "caller"))
            .expect("second export")
            .function()
    );

    // The same relative ordinal validly selects a different owner in each image.
    let first_function = first.function(ordinal).expect("first function");
    let second_function = second.function(ordinal).expect("second function");
    assert!(std::ptr::eq(first_function.image(), &first));
    assert!(std::ptr::eq(second_function.image(), &second));
    assert!(std::ptr::eq(
        first_function.body(),
        &first.functions()[ordinal.index()]
    ));
    assert!(std::ptr::eq(
        second_function.body(),
        &second.functions()[ordinal.index()]
    ));
    assert!(first_function.demand().is_empty());
    assert!(second_function.demand().is_empty());
    assert_eq!(run(second_function, Vec::new()), Ok(Some(Value::Int(49))));
    assert_eq!(run(first_function, Vec::new()), Ok(Some(Value::Int(42))));
    assert_eq!(run(second_function, Vec::new()), Ok(Some(Value::Int(49))));
}

#[test]
fn a_self_recursive_call_rejects_as_a_cycle() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("loops").expect("a within-domain mint");
    let code = vec![Instr::Call(0), Instr::Return];
    let func = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "loops"), func);
    let bytes = draft.encode().expect("encode").bytes;
    assert_eq!(
        verify(&bytes).err().map(|r| r.code().to_string()),
        Some("image.closure".to_string())
    );
}

#[test]
fn an_acyclic_call_chain_past_the_dynamic_depth_bound_refuses() {
    const FIRST_OVER_LIMIT_CALLS: usize = 65;

    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let zero = draft.intern_int(0).expect("a within-domain mint");

    let leaf_name = draft.intern_string("leaf").expect("a within-domain mint");
    let leaf_code = vec![Instr::ConstLoad(zero), Instr::Return];
    let mut callee = draft
        .add_function(FunctionDef {
            name: leaf_name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            spans: spans(&leaf_code),
            code: leaf_code,
        })
        .expect("every site operand is live");

    for depth in 1..=FIRST_OVER_LIMIT_CALLS {
        let name = draft
            .intern_string(&format!("depth_{depth}"))
            .expect("a within-domain mint");
        let code = vec![Instr::Call(callee.index()), Instr::Return];
        callee = draft
            .add_function(FunctionDef {
                name,
                source: src,
                params: Vec::new(),
                ret: ImageType::scalar(Scalar::Int),
                local_count: 0,
                spans: spans(&code),
                code,
            })
            .expect("every site operand is live");
    }

    draft.add_export(ExportId::of_local("", "deepest"), callee);
    let bytes = draft.encode().expect("encode").bytes;
    let image = verify(&bytes).expect("the acyclic chain verifies");
    let index = image
        .export_by_id(ExportId::of_local("", "deepest"))
        .expect("export")
        .function();

    // The image admits up to 4,096 functions, so an acyclic chain can cross the VM's
    // 64-call dynamic bound even though the verifier rejects recursive cycles.
    assert_eq!(
        run(
            image.function(index).expect("verified export function"),
            Vec::new()
        )
        .err()
        .map(|fault| fault.code().to_string()),
        Some("run.call_depth".to_string()),
    );
}
