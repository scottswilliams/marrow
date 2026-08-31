//! Closed flat enums through the sealed tape: construction, the match primitive
//! (`EnumTag`), positional payload reads (`EnumPayloadGet`), and exact equality
//! (`EqEnum`). Images are minted with `ImageDraft`, sealed by the independent
//! verifier, and run on the VM. The payload-read variant guard is the soundness
//! core: an image extracting the wrong variant's payload faults rather than
//! reading a differently-typed leaf.

use marrow_image::{
    DraftTxn, EnumTypeDef, ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry,
    VariantDef,
};
use marrow_verify::verify;
use marrow_vm::{Value, run};

/// An enum `Shape { dot, circle(int), rect(int, int) }` interned into `draft`,
/// returning its enum index.
fn shape_enum(draft: &mut DraftTxn<'_>) -> marrow_image::EnumId {
    let name = draft.intern_string("Shape").expect("a within-domain mint");
    let dot = draft.intern_string("dot").expect("a within-domain mint");
    let circle = draft.intern_string("circle").expect("a within-domain mint");
    let rect = draft.intern_string("rect").expect("a within-domain mint");
    draft
        .add_enum_type(EnumTypeDef {
            name,
            variants: vec![
                VariantDef {
                    name: dot,
                    category: false,
                    payload: vec![],
                },
                VariantDef {
                    name: circle,
                    category: false,
                    payload: vec![ImageType::scalar(Scalar::Int)],
                },
                VariantDef {
                    name: rect,
                    category: false,
                    payload: vec![
                        ImageType::scalar(Scalar::Int),
                        ImageType::scalar(Scalar::Int),
                    ],
                },
            ],
        })
        .expect("a within-domain mint")
}

fn build_and_run(
    build: impl FnOnce(&mut DraftTxn<'_>) -> (ImageType, Vec<Instr>),
) -> Result<Option<Value>, String> {
    let mut draft_owner = ImageDraft::new();
    let savepoint = draft_owner.savepoint();
    let mut draft = draft_owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits");
    let name = draft.intern_string("f").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let (ret, code) = build(&mut draft);
    let spans = (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect();
    let func = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret,
            local_count: 2,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
    let bytes = draft.encode().expect("encode").bytes;
    let image = verify(&bytes).map_err(|rejection| rejection.code().to_string())?;
    let index = image
        .export_by_id(ExportId::of_local("", "f"))
        .expect("export present")
        .function();
    run(&image, index, Vec::new()).map_err(|fault| fault.code().to_string())
}

#[test]
fn construct_then_read_the_variant_tag() {
    // Shape::circle(2) then EnumTag == 1 (circle is variant index 1).
    let result = build_and_run(|draft| {
        let shape = shape_enum(draft);
        let two = draft.intern_int(2).expect("a within-domain mint");
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::ConstLoad(two),
                Instr::EnumConstruct {
                    enum_idx: shape,
                    variant: 1,
                },
                Instr::EnumTag,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Ok(Some(Value::Int(1))));
}

#[test]
fn read_a_payload_leaf() {
    // Shape::rect(3, 5) then EnumPayloadGet(rect, field 1) == 5.
    let result = build_and_run(|draft| {
        let shape = shape_enum(draft);
        let three = draft.intern_int(3).expect("a within-domain mint");
        let five = draft.intern_int(5).expect("a within-domain mint");
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::ConstLoad(three),
                Instr::ConstLoad(five),
                Instr::EnumConstruct {
                    enum_idx: shape,
                    variant: 2,
                },
                Instr::EnumPayloadGet {
                    variant: 2,
                    field: 1,
                },
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Ok(Some(Value::Int(5))));
}

#[test]
fn equality_compares_variant_and_payload() {
    // Shape::circle(2) == Shape::circle(2) is true.
    let result = build_and_run(|draft| {
        let shape = shape_enum(draft);
        let two = draft.intern_int(2).expect("a within-domain mint");
        (
            ImageType::scalar(Scalar::Bool),
            vec![
                Instr::ConstLoad(two),
                Instr::EnumConstruct {
                    enum_idx: shape,
                    variant: 1,
                },
                Instr::ConstLoad(two),
                Instr::EnumConstruct {
                    enum_idx: shape,
                    variant: 1,
                },
                Instr::EqEnum,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Ok(Some(Value::Bool(true))));

    // Different payloads compare unequal.
    let result = build_and_run(|draft| {
        let shape = shape_enum(draft);
        let two = draft.intern_int(2).expect("a within-domain mint");
        let three = draft.intern_int(3).expect("a within-domain mint");
        (
            ImageType::scalar(Scalar::Bool),
            vec![
                Instr::ConstLoad(two),
                Instr::EnumConstruct {
                    enum_idx: shape,
                    variant: 1,
                },
                Instr::ConstLoad(three),
                Instr::EnumConstruct {
                    enum_idx: shape,
                    variant: 1,
                },
                Instr::EqEnum,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Ok(Some(Value::Bool(false))));
}

#[test]
fn a_wrong_variant_payload_read_faults() {
    // A hand-built image that constructs `dot` (variant 0) but reads it as the
    // `circle` payload (variant 1) verifies (the leaf type-checks against variant
    // 1) but faults at run time — the defense-in-depth guard, never reachable from
    // compiled code, which always dispatches on the tag first.
    let result = build_and_run(|draft| {
        let shape = shape_enum(draft);
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::EnumConstruct {
                    enum_idx: shape,
                    variant: 0,
                },
                Instr::EnumPayloadGet {
                    variant: 1,
                    field: 0,
                },
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Err("run.enum_variant".to_string()));
}
