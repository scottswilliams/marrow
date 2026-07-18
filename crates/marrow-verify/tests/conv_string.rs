//! `ConvString` (the `value → string` canonical renderer behind `$"{}"` and
//! `string(...)`) verifies only for a bare scalar, enum, or identity operand — the
//! values the runtime renderer is total over as interpolable leaves. A bare aggregate
//! (list/map/record) or any optional operand is refused at verification, so a forged
//! image can never drive the renderer with a value it would have to reject at runtime.
//! This guards the interpolation-canon soundness rule directly on the trust boundary.

use marrow_image::{
    CollectionTypeDef, EnumTypeDef, ExportId, FieldDef, FunctionDef, ImageDraft, ImageType, Instr,
    RecordTypeDef, Scalar, SpanEntry, VariantDef,
};
use marrow_verify::verify;

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

const TEXT: ImageType = ImageType::Scalar {
    scalar: Scalar::Text,
    optional: false,
};

/// Build a one-export image whose `main` runs `setup` to push one operand, then applies
/// `ConvString` and returns the resulting `string`. Returns the verifier's result (the
/// rejection code on failure).
fn verify_conv(setup: impl FnOnce(&mut ImageDraft) -> Vec<Instr>) -> Result<(), String> {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let mut code = setup(&mut draft);
    code.push(Instr::ConvString);
    code.push(Instr::Return);
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: TEXT,
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    let bytes = draft.encode().expect("encode").bytes;
    verify(&bytes).map(|_| ()).map_err(|r| r.code().to_string())
}

/// Add a bare two-member `Color` enum, returning its ENUMS index.
fn color_enum(draft: &mut ImageDraft) -> u16 {
    let name = draft.intern_string("Color");
    let red = draft.intern_string("red");
    let green = draft.intern_string("green");
    draft
        .add_enum_type(EnumTypeDef {
            name,
            variants: vec![
                VariantDef {
                    name: red,
                    category: false,
                    payload: vec![],
                },
                VariantDef {
                    name: green,
                    category: false,
                    payload: vec![],
                },
            ],
        })
        .index()
}

#[test]
fn conv_string_accepts_a_bare_scalar() {
    let result = verify_conv(|draft| {
        let n = draft.intern_int(7);
        vec![Instr::ConstLoad(n.index())]
    });
    assert!(result.is_ok(), "a bare scalar renders: {result:?}");
}

#[test]
fn conv_string_accepts_a_bare_enum() {
    let result = verify_conv(|draft| {
        let idx = color_enum(draft);
        vec![Instr::EnumConstruct {
            enum_idx: idx,
            variant: 0,
        }]
    });
    assert!(result.is_ok(), "a bare enum renders: {result:?}");
}

#[test]
fn conv_string_rejects_a_bare_list() {
    let result = verify_conv(|draft| {
        let idx = draft
            .add_collection_type(CollectionTypeDef::List {
                elem: ImageType::scalar(Scalar::Int),
            })
            .index();
        vec![Instr::ListNew(idx)]
    });
    assert!(
        result.is_err(),
        "a bare list is not a renderable ConvString operand: {result:?}"
    );
}

#[test]
fn conv_string_rejects_a_bare_record() {
    let result = verify_conv(|draft| {
        let name = draft.intern_string("Point");
        let field = draft.intern_string("x");
        draft.add_record_type(RecordTypeDef {
            name,
            fields: vec![FieldDef {
                name: field,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        });
        let n = draft.intern_int(1);
        vec![Instr::ConstLoad(n.index()), Instr::RecordNew(0)]
    });
    assert!(
        result.is_err(),
        "a bare record is not a renderable ConvString operand: {result:?}"
    );
}

#[test]
fn conv_string_rejects_a_bare_map() {
    let result = verify_conv(|draft| {
        let idx = draft
            .add_collection_type(CollectionTypeDef::Map {
                key: ImageType::scalar(Scalar::Text),
                value: ImageType::scalar(Scalar::Int),
            })
            .index();
        vec![Instr::MapNew(idx)]
    });
    assert!(
        result.is_err(),
        "a bare map is not a renderable ConvString operand: {result:?}"
    );
}

#[test]
fn conv_string_rejects_an_optional_scalar() {
    // `int` wrapped to `int?` is an optional operand — not renderable through ConvString.
    let result = verify_conv(|draft| {
        let n = draft.intern_int(7);
        vec![Instr::ConstLoad(n.index()), Instr::SomeWrap]
    });
    assert!(
        result.is_err(),
        "an optional scalar is not a renderable ConvString operand: {result:?}"
    );
}

#[test]
fn conv_string_rejects_an_optional_enum() {
    let result = verify_conv(|draft| {
        let idx = color_enum(draft);
        vec![
            Instr::EnumConstruct {
                enum_idx: idx,
                variant: 0,
            },
            Instr::SomeWrap,
        ]
    });
    assert!(
        result.is_err(),
        "an optional enum is not a renderable ConvString operand: {result:?}"
    );
}
