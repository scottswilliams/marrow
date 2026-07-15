//! Slice K.3 evidence: records and optionals through the sealed tape.
//!
//! Images are minted with `ImageDraft`, sealed by the independent verifier, and run
//! on the VM. The presence-typing rejection (a `T?` reaching a bare consumer) is the
//! soundness core: the only way to obtain a bare value from an optional is
//! `BranchPresent`, so an image that feeds `T?` into arithmetic rejects at verify.

use marrow_image::{
    ExportId, FieldDef, FunctionDef, ImageDraft, ImageType, Instr, RecordTypeDef, Scalar, SpanEntry,
};
use marrow_verify::verify;
use marrow_vm::{Value, run};

/// A record `Note { required value: int, label: string }` interned into `draft`,
/// returning its type index.
fn note_type(draft: &mut ImageDraft) -> u16 {
    let name = draft.intern_string("Note");
    let value = draft.intern_string("value");
    let label = draft.intern_string("label");
    let type_id = draft.add_record_type(RecordTypeDef {
        name,
        fields: vec![
            FieldDef {
                name: value,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            },
            FieldDef {
                name: label,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
    });
    type_id.index()
}

fn build_and_run(
    build: impl FnOnce(&mut ImageDraft) -> (ImageType, Vec<Instr>),
) -> Result<Option<Value>, String> {
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
    let bytes = draft.encode().expect("encode").bytes;
    let image = verify(&bytes).map_err(|rejection| rejection.code().to_string())?;
    let index = image
        .export_by_id(ExportId::of_local("", "f"))
        .expect("export present")
        .function();
    run(&image, index, Vec::new()).map_err(|fault| fault.code().to_string())
}

#[test]
fn construct_then_read_required_field() {
    // Note(value: 5, label: absent).value == 5
    let result = build_and_run(|draft| {
        let ty = note_type(draft);
        let five = draft.intern_int(5);
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::ConstLoad(five.index()),
                Instr::VacantLoad(ImageType::opt_scalar(Scalar::Text)),
                Instr::RecordNew(ty),
                Instr::FieldGet(0),
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Ok(Some(Value::Int(5))));
}

#[test]
fn reading_a_vacant_sparse_field_yields_an_empty_optional() {
    // Note(value: 5, label: absent).label == absent
    let result = build_and_run(|draft| {
        let ty = note_type(draft);
        let five = draft.intern_int(5);
        (
            ImageType::opt_scalar(Scalar::Text),
            vec![
                Instr::ConstLoad(five.index()),
                Instr::VacantLoad(ImageType::opt_scalar(Scalar::Text)),
                Instr::RecordNew(ty),
                Instr::FieldGet(1),
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Ok(Some(Value::Optional(None))));
}

#[test]
fn branch_present_unwraps_a_present_sparse_field() {
    // let n = Note(value: 5, label: "hi"); if present(n.label) -> label else "x"
    let result = build_and_run(|draft| {
        let ty = note_type(draft);
        let five = draft.intern_int(5);
        let hi = draft.intern_text("hi");
        let fallback = draft.intern_text("x");
        // Construct with a present label (SomeWrap coerces the bare text).
        let mut code = vec![
            Instr::ConstLoad(five.index()),
            Instr::ConstLoad(hi.index()),
            Instr::SomeWrap,
            Instr::RecordNew(ty),
            Instr::FieldGet(1), // label: text?
        ];
        // BranchPresent: present -> bare text on stack, jump past the fallback.
        code.push(Instr::BranchPresent(0)); // target patched below by index
        let bp_index = code.len() - 1;
        // present arm: value already on stack, jump to Return.
        code.push(Instr::Jump(0));
        let jump_index = code.len() - 1;
        // absent arm:
        let absent_index = code.len();
        code.push(Instr::ConstLoad(fallback.index()));
        let end_index = code.len();
        code.push(Instr::Return);
        // patch draft-form instruction-index targets.
        if let Instr::BranchPresent(t) = &mut code[bp_index] {
            *t = absent_index as u32;
        }
        if let Instr::Jump(t) = &mut code[jump_index] {
            *t = end_index as u32;
        }
        (ImageType::scalar(Scalar::Text), code)
    });
    assert_eq!(result, Ok(Some(Value::Text("hi".into()))));
}

#[test]
fn optional_into_a_bare_consumer_rejects() {
    // A vacant int? fed into IntAdd (a bare consumer) must reject at verify: the only
    // way to obtain a bare value from an optional is BranchPresent.
    let result = build_and_run(|draft| {
        let one = draft.intern_int(1);
        (
            ImageType::scalar(Scalar::Int),
            vec![
                Instr::VacantLoad(ImageType::opt_scalar(Scalar::Int)),
                Instr::ConstLoad(one.index()),
                Instr::IntAdd,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Err("image.function".to_string()));
}

// --- Local product mutation: FieldSet / FieldUnset (C02 V5). ---

#[test]
fn field_set_stores_a_value_present() {
    // let n = Note(value: 5, label: absent); n.label = "hi"; n.label == "hi"
    let result = build_and_run(|draft| {
        let ty = note_type(draft);
        let five = draft.intern_int(5);
        let hi = draft.intern_text("hi");
        (
            ImageType::opt_scalar(Scalar::Text),
            vec![
                Instr::ConstLoad(five.index()),
                Instr::VacantLoad(ImageType::opt_scalar(Scalar::Text)),
                Instr::RecordNew(ty),
                Instr::ConstLoad(hi.index()),
                Instr::FieldSet(1),
                Instr::FieldGet(1),
                Instr::Return,
            ],
        )
    });
    assert_eq!(
        result,
        Ok(Some(Value::Optional(Some(Box::new(Value::Text(
            "hi".into()
        ))))))
    );
}

#[test]
fn field_unset_clears_a_present_sparse_field() {
    // let n = Note(value: 5, label: "hi"); unset n.label; n.label == absent
    let result = build_and_run(|draft| {
        let ty = note_type(draft);
        let five = draft.intern_int(5);
        let hi = draft.intern_text("hi");
        (
            ImageType::opt_scalar(Scalar::Text),
            vec![
                Instr::ConstLoad(five.index()),
                Instr::ConstLoad(hi.index()),
                Instr::SomeWrap,
                Instr::RecordNew(ty),
                Instr::FieldUnset(1),
                Instr::FieldGet(1),
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Ok(Some(Value::Optional(None))));
}

#[test]
fn field_unset_on_a_required_field_rejects() {
    // A hostile image unsetting a required field (index 0) rejects at verify: a
    // required field is never vacant.
    let result = build_and_run(|draft| {
        let ty = note_type(draft);
        let five = draft.intern_int(5);
        (
            ImageType::Unit,
            vec![
                Instr::ConstLoad(five.index()),
                Instr::VacantLoad(ImageType::opt_scalar(Scalar::Text)),
                Instr::RecordNew(ty),
                Instr::FieldUnset(0),
                Instr::Pop,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Err("image.function".to_string()));
}

#[test]
fn field_set_with_a_wrong_typed_operand_rejects() {
    // Setting the text field (index 1) with an int operand is a verify type error.
    let result = build_and_run(|draft| {
        let ty = note_type(draft);
        let five = draft.intern_int(5);
        (
            ImageType::Unit,
            vec![
                Instr::ConstLoad(five.index()),
                Instr::VacantLoad(ImageType::opt_scalar(Scalar::Text)),
                Instr::RecordNew(ty),
                Instr::ConstLoad(five.index()),
                Instr::FieldSet(1),
                Instr::Pop,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Err("image.function".to_string()));
}

#[test]
fn field_set_with_an_out_of_range_field_index_rejects() {
    // A field index past the record's field list is a verify error.
    let result = build_and_run(|draft| {
        let ty = note_type(draft);
        let five = draft.intern_int(5);
        (
            ImageType::Unit,
            vec![
                Instr::ConstLoad(five.index()),
                Instr::VacantLoad(ImageType::opt_scalar(Scalar::Text)),
                Instr::RecordNew(ty),
                Instr::ConstLoad(five.index()),
                Instr::FieldSet(9),
                Instr::Pop,
                Instr::Return,
            ],
        )
    });
    assert_eq!(result, Err("image.function".to_string()));
}
