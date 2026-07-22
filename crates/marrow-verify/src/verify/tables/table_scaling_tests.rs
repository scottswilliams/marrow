use super::{
    decode_enums, decode_enums_with_work, decode_strings, decode_types, decode_types_with_work,
};
use crate::reject::VerifyPhase;
use marrow_image::{
    EnumTypeDef, ExportId, FieldDef, FunctionDef, ImageDraft, ImageType, Instr, RecordTypeDef,
    Scalar, SpanEntry, VariantDef, image_id,
};
use std::ops::Range;

const RECORD_WIDTH: usize = 4_096;
const ENUM_WIDTH: usize = 256;

fn add_main(draft: &mut ImageDraft) {
    let name = draft.intern_string("main");
    let source = draft.intern_string("src/main.mw");
    let code = vec![Instr::Return];
    let function = draft.add_function(FunctionDef {
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
        code,
    });
    draft.add_export(ExportId::of_local("", "main"), function);
}

fn record_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let record_name = draft.intern_string("Wide");
    let mut fields = Vec::with_capacity(RECORD_WIDTH);
    for index in 0..RECORD_WIDTH {
        fields.push(FieldDef {
            name: draft.intern_string(&format!("field{index:04}")),
            ty: ImageType::scalar(Scalar::Int),
            required: index % 2 == 0,
        });
    }
    draft.add_record_type(RecordTypeDef {
        name: record_name,
        fields,
    });
    add_main(&mut draft);
    draft.encode().expect("below-cap record image").bytes
}

fn enum_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let enum_name = draft.intern_string("Choice");
    let mut variants = Vec::with_capacity(ENUM_WIDTH);
    for index in 0..ENUM_WIDTH {
        variants.push(VariantDef {
            name: draft.intern_string(&format!("variant{index:03}")),
            category: index % 2 == 1,
            payload: Vec::new(),
        });
    }
    draft.add_enum_type(EnumTypeDef {
        name: enum_name,
        variants,
    });
    add_main(&mut draft);
    draft.encode().expect("below-cap enum image").bytes
}

fn repeated_names_across_rows_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let field_name = draft.intern_string("value");
    for record in ["First", "Second"] {
        let name = draft.intern_string(record);
        draft.add_record_type(RecordTypeDef {
            name,
            fields: vec![FieldDef {
                name: field_name,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        });
    }
    let variant_name = draft.intern_string("ready");
    for item in ["Left", "Right"] {
        let name = draft.intern_string(item);
        draft.add_enum_type(EnumTypeDef {
            name,
            variants: vec![VariantDef {
                name: variant_name,
                category: false,
                payload: Vec::new(),
            }],
        });
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

fn section_body(bytes: &[u8], id: u8) -> &[u8] {
    &bytes[section_range(bytes, id)]
}

fn rehash(bytes: &mut [u8]) {
    let digest = image_id(&bytes[37..]);
    bytes[5..37].copy_from_slice(&digest.0);
}

#[test]
fn full_width_record_projection_is_linear_and_preserves_sealed_order() {
    let bytes = record_image();
    let string_count = decode_strings(section_body(&bytes, 0x01))
        .expect("canonical strings")
        .len();
    let decoded = decode_types_with_work(section_body(&bytes, 0x02), string_count)
        .expect("canonical record table");
    assert_eq!(decoded.name_count, RECORD_WIDTH);
    assert_eq!(decoded.name_checks, RECORD_WIDTH);

    let verified = crate::verify::verify(&bytes).expect("full record image verifies");
    let fields = verified.record_types()[0].fields();
    assert_eq!(fields.len(), RECORD_WIDTH);
    for (index, field) in fields.iter().enumerate() {
        assert_eq!(field.name.as_ref(), format!("field{index:04}"));
        assert_eq!(field.ty, ImageType::scalar(Scalar::Int));
        assert_eq!(field.required, index % 2 == 0);
    }
}

#[test]
fn enum_projection_is_linear_and_preserves_sealed_order() {
    let bytes = enum_image();
    let string_count = decode_strings(section_body(&bytes, 0x01))
        .expect("canonical strings")
        .len();
    let decoded = decode_enums_with_work(section_body(&bytes, 0x09), string_count, 0)
        .expect("canonical enum table");
    assert_eq!(decoded.name_count, ENUM_WIDTH);
    assert_eq!(decoded.name_checks, ENUM_WIDTH);

    let verified = crate::verify::verify(&bytes).expect("full enum image verifies");
    let variants = verified.enums()[0].variants();
    assert_eq!(variants.len(), ENUM_WIDTH);
    for (index, variant) in variants.iter().enumerate() {
        assert_eq!(variant.name.as_ref(), format!("variant{index:03}"));
        assert_eq!(variant.category, index % 2 == 1);
        assert!(variant.payload.is_empty());
    }
}

#[test]
fn generation_marks_allow_the_same_name_in_distinct_rows() {
    let bytes = repeated_names_across_rows_image();
    let string_count = decode_strings(section_body(&bytes, 0x01))
        .expect("canonical strings")
        .len();
    let records = decode_types_with_work(section_body(&bytes, 0x02), string_count)
        .expect("record rows may reuse a field name");
    assert_eq!(records.name_count, 2);
    assert_eq!(records.name_checks, 2);
    let enums =
        decode_enums_with_work(section_body(&bytes, 0x09), string_count, records.rows.len())
            .expect("enum rows may reuse a variant name");
    assert_eq!(enums.name_count, 2);
    assert_eq!(enums.name_checks, 2);

    let verified = crate::verify::verify(&bytes).expect("cross-row repeated names verify");
    assert_eq!(verified.record_types().len(), 2);
    assert_eq!(verified.enums().len(), 2);
    assert_eq!(
        verified.record_types()[0].fields()[0].name.as_ref(),
        "value"
    );
    assert_eq!(
        verified.record_types()[1].fields()[0].name.as_ref(),
        "value"
    );
    assert_eq!(verified.enums()[0].variants()[0].name.as_ref(), "ready");
    assert_eq!(verified.enums()[1].variants()[0].name.as_ref(), "ready");
}

#[test]
fn duplicate_record_name_rejects_before_its_poisoned_type_byte() {
    let mut bytes = record_image();
    let strings = decode_strings(section_body(&bytes, 0x01)).expect("canonical strings");
    let range = section_range(&bytes, 0x02);
    let body = &mut bytes[range];
    let first_field = 6;
    let final_field = first_field + (RECORD_WIDTH - 1) * 4;
    let first_name = [body[first_field], body[first_field + 1]];
    body[final_field..final_field + 2].copy_from_slice(&first_name);
    body[final_field + 2] = 0xff;
    rehash(&mut bytes);

    let direct = match decode_types(section_body(&bytes, 0x02), strings.len()) {
        Ok(_) => panic!("duplicate field name must reject"),
        Err(rejection) => rejection,
    };
    assert_eq!(direct.phase(), VerifyPhase::Table);
    assert_eq!(direct.detail(), "duplicate field name in record");

    let public = crate::verify::verify(&bytes).expect_err("public verifier rejects duplicate");
    assert_eq!(public.phase(), VerifyPhase::Table);
    assert_eq!(public.detail(), "duplicate field name in record");
}

#[test]
fn duplicate_variant_name_rejects_before_its_poisoned_category_byte() {
    let mut bytes = enum_image();
    let strings = decode_strings(section_body(&bytes, 0x01)).expect("canonical strings");
    let range = section_range(&bytes, 0x09);
    let body = &mut bytes[range];
    let first_variant = 6;
    let final_variant = first_variant + (ENUM_WIDTH - 1) * 4;
    let first_name = [body[first_variant], body[first_variant + 1]];
    body[final_variant..final_variant + 2].copy_from_slice(&first_name);
    body[final_variant + 2] = 0xff;
    rehash(&mut bytes);

    let direct = match decode_enums(section_body(&bytes, 0x09), strings.len(), 0) {
        Ok(_) => panic!("duplicate variant name must reject"),
        Err(rejection) => rejection,
    };
    assert_eq!(direct.phase(), VerifyPhase::Table);
    assert_eq!(direct.detail(), "duplicate variant name in enum");

    let public = crate::verify::verify(&bytes).expect_err("public verifier rejects duplicate");
    assert_eq!(public.phase(), VerifyPhase::Table);
    assert_eq!(public.detail(), "duplicate variant name in enum");
}
