//! A repeated name inside one record or enum row is refused at the repeated name, before
//! the bytes that follow it are read; the same name in distinct rows is admitted.

use crate::reject::{Duplicate, RejectionKind, VerifyPhase};
use marrow_image::{
    DraftTxn, EnumTypeDef, ExportId, FieldDef, FunctionDef, ImageDraft, ImageType, Instr,
    RecordTypeDef, Scalar, SpanEntry, VariantDef,
};
use marrow_test_support::{admitted, rehash};
use std::ops::Range;

const RECORD_WIDTH: usize = 4_096;
const ENUM_WIDTH: usize = 256;

fn add_main(draft: &mut DraftTxn<'_>) {
    let name = draft.intern_string("main").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            spans: vec![SpanEntry {
                instr_index: 0,
                line: 1,
                column: 1,
            }],
            code: vec![Instr::Return],
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), function);
}

fn record_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let record_name = draft.intern_string("Wide").expect("a within-domain mint");
    let mut fields = Vec::with_capacity(RECORD_WIDTH);
    for index in 0..RECORD_WIDTH {
        fields.push(FieldDef {
            name: draft
                .intern_string(&format!("field{index:04}"))
                .expect("a within-domain mint"),
            ty: ImageType::scalar(Scalar::Int),
            required: index % 2 == 0,
        });
    }
    draft
        .add_record_type(RecordTypeDef {
            name: record_name,
            fields,
        })
        .expect("a within-domain mint");
    add_main(&mut draft);
    draft.encode().expect("below-cap record image").bytes
}

fn enum_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let enum_name = draft.intern_string("Choice").expect("a within-domain mint");
    let mut variants = Vec::with_capacity(ENUM_WIDTH);
    for index in 0..ENUM_WIDTH {
        variants.push(VariantDef {
            name: draft
                .intern_string(&format!("variant{index:03}"))
                .expect("a within-domain mint"),
            category: index % 2 == 1,
            payload: Vec::new(),
        });
    }
    draft
        .add_enum_type(EnumTypeDef {
            name: enum_name,
            variants,
        })
        .expect("a within-domain mint");
    add_main(&mut draft);
    draft.encode().expect("below-cap enum image").bytes
}

fn repeated_names_across_rows_image() -> Vec<u8> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let field_name = draft.intern_string("value").expect("a within-domain mint");
    for record in ["First", "Second"] {
        let name = draft.intern_string(record).expect("a within-domain mint");
        draft
            .add_record_type(RecordTypeDef {
                name,
                fields: vec![FieldDef {
                    name: field_name,
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                }],
            })
            .expect("a within-domain mint");
    }
    let variant_name = draft.intern_string("ready").expect("a within-domain mint");
    for item in ["Left", "Right"] {
        let name = draft.intern_string(item).expect("a within-domain mint");
        draft
            .add_enum_type(EnumTypeDef {
                name,
                variants: vec![VariantDef {
                    name: variant_name,
                    category: false,
                    payload: Vec::new(),
                }],
            })
            .expect("a within-domain mint");
    }
    add_main(&mut draft);
    draft
        .encode()
        .expect("cross-row repeated names are canonical")
        .bytes
}

fn section_range(bytes: &[u8], wanted: u8) -> Range<usize> {
    let mut cursor = 38;
    for _ in 0..bytes[37] {
        let id = bytes[cursor];
        let length = u32::from_be_bytes(
            bytes[cursor + 1..cursor + 5]
                .try_into()
                .expect("section length bytes"),
        ) as usize;
        let start = cursor + 5;
        let end = start + length;
        if id == wanted {
            return start..end;
        }
        cursor = end;
    }
    panic!("section {wanted:#04x} is present in a canonical image");
}

#[test]
fn the_same_name_in_distinct_rows_is_admitted() {
    let verified =
        crate::verify::verify(&repeated_names_across_rows_image()).expect("cross-row names verify");
    assert_eq!(verified.record_types().len(), 2);
    assert_eq!(verified.enums().len(), 2);
    for record in verified.record_types() {
        assert_eq!(record.fields()[0].name.as_ref(), "value");
    }
    for enum_type in verified.enums() {
        assert_eq!(enum_type.variants()[0].name.as_ref(), "ready");
    }
}

/// The last field takes the first field's name and a poisoned type byte: the duplicate is
/// refused before the type byte is read.
#[test]
fn a_duplicate_field_name_rejects_before_its_poisoned_type_byte() {
    let mut bytes = record_image();
    let range = section_range(&bytes, 0x02);
    let body = &mut bytes[range];
    let first_field = 6;
    let final_field = first_field + (RECORD_WIDTH - 1) * 4;
    let first_name = [body[first_field], body[first_field + 1]];
    body[final_field..final_field + 2].copy_from_slice(&first_name);
    body[final_field + 2] = 0xff;
    rehash(&mut bytes);

    let rejection = crate::verify::verify(&bytes).expect_err("a duplicate field name rejects");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.kind(),
        &RejectionKind::Duplicate(Duplicate::FieldName)
    );
}

/// The last variant takes the first variant's name and a poisoned category byte: the
/// duplicate is refused before the category byte is read.
#[test]
fn a_duplicate_variant_name_rejects_before_its_poisoned_category_byte() {
    let mut bytes = enum_image();
    let range = section_range(&bytes, 0x09);
    let body = &mut bytes[range];
    let first_variant = 6;
    let final_variant = first_variant + (ENUM_WIDTH - 1) * 4;
    let first_name = [body[first_variant], body[first_variant + 1]];
    body[final_variant..final_variant + 2].copy_from_slice(&first_name);
    body[final_variant + 2] = 0xff;
    rehash(&mut bytes);

    let rejection = crate::verify::verify(&bytes).expect_err("a duplicate variant name rejects");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.kind(),
        &RejectionKind::Duplicate(Duplicate::VariantName)
    );
}
