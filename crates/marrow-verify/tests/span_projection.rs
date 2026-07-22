//! Public-path coverage for bounded verifier span projection.

use marrow_image::{
    ExportId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry, image_id,
};
use marrow_verify::{VerifyPhase, verify};

const INSTRUCTION_COUNT: usize = 4_096;
const FUNCTION_SECTION_ID: u8 = 0x05;
const SPAN_SECTION_ID: u8 = 0x07;

fn linear_span_image(with_spans: bool) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    let source = draft.intern_string("src/main.mw");
    let name = draft.intern_string("linearSpans");
    let mut code = Vec::with_capacity(INSTRUCTION_COUNT);
    for _ in 0..2_047 {
        code.push(Instr::VacantLoad(ImageType::opt_scalar(Scalar::Int)));
        code.push(Instr::Pop);
    }
    code.push(Instr::Jump((INSTRUCTION_COUNT - 1) as u32));
    code.push(Instr::Return);
    assert_eq!(code.len(), INSTRUCTION_COUNT);

    let spans = if with_spans {
        (0..INSTRUCTION_COUNT)
            .map(|instr_index| SpanEntry {
                instr_index: instr_index as u32,
                line: 1,
                column: 1,
            })
            .collect()
    } else {
        Vec::new()
    };
    let function = draft.add_function(FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "linearSpans"), function);
    draft.encode().expect("encode linear span image").bytes
}

fn section(bytes: &[u8], wanted: u8) -> (usize, usize) {
    let mut offset = 38;
    for _ in 0..10 {
        let section_id = bytes[offset];
        let length = u32::from_be_bytes(
            bytes[offset + 1..offset + 5]
                .try_into()
                .expect("four-byte section length"),
        ) as usize;
        let body = offset + 5;
        if section_id == wanted {
            return (body, length);
        }
        offset = body + length;
    }
    panic!("section {wanted:#04x} is present");
}

fn span_offset_at(span_body: usize, row: usize) -> usize {
    span_body + 2 + row * 12
}

fn only_function_code_len(bytes: &[u8]) -> u32 {
    let (body, _) = section(bytes, FUNCTION_SECTION_ID);
    assert_eq!(u16::from_be_bytes([bytes[body], bytes[body + 1]]), 1);
    assert_eq!(bytes[body + 6], 0, "fixture function has no parameters");
    read_u32(bytes, body + 10)
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("four-byte integer"),
    )
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn rehash(bytes: &mut [u8]) {
    let id = image_id(&bytes[37..]);
    bytes[5..37].copy_from_slice(&id.0);
}

fn assert_rejection(bytes: &[u8], phase: VerifyPhase, detail: &'static str) {
    let rejection = verify(bytes).expect_err("hostile span image must reject");
    assert_eq!(rejection.phase(), phase);
    assert_eq!(rejection.detail(), detail);
}

#[test]
fn full_span_projection_is_bounded_and_preserves_rejections() {
    let bytes = linear_span_image(true);
    println!("D=S=4096 encoded image bytes: {}", bytes.len());
    assert!(bytes.len() < marrow_image::bounds::MAX_IMAGE_BYTES);
    let verified = verify(&bytes).expect("valid full span mapping verifies");
    assert_eq!(verified.functions().len(), 1);
    assert_eq!(verified.functions()[0].instrs().len(), INSTRUCTION_COUNT);
    assert_eq!(
        verified.functions()[0].span_at(INSTRUCTION_COUNT - 1),
        Some((1, 1))
    );

    assert_rejection(
        &linear_span_image(false),
        VerifyPhase::Function,
        "code has no span mappings",
    );

    let (span_body, span_length) = section(&bytes, SPAN_SECTION_ID);
    assert_eq!(span_length, 2 + INSTRUCTION_COUNT * 12);
    assert_eq!(
        u16::from_be_bytes(
            bytes[span_body..span_body + 2]
                .try_into()
                .expect("two-byte span count")
        ) as usize,
        INSTRUCTION_COUNT
    );

    let mut first_offset = bytes.clone();
    write_u32(&mut first_offset, span_offset_at(span_body, 0), 1);
    rehash(&mut first_offset);
    assert_rejection(
        &first_offset,
        VerifyPhase::Function,
        "first span must map instruction offset 0",
    );

    let mut interior_offset = bytes.clone();
    let interior_at = span_offset_at(span_body, 2);
    assert_eq!(read_u32(&interior_offset, interior_at), 3);
    assert_eq!(read_u32(&interior_offset, span_offset_at(span_body, 3)), 5);
    write_u32(&mut interior_offset, interior_at, 4);
    rehash(&mut interior_offset);
    assert_rejection(
        &interior_offset,
        VerifyPhase::Function,
        "span offset is not an instruction boundary",
    );

    let mut past_end_offset = bytes.clone();
    let last_at = span_offset_at(span_body, INSTRUCTION_COUNT - 1);
    let last_offset = read_u32(&past_end_offset, last_at);
    let code_len = only_function_code_len(&past_end_offset);
    assert_eq!(last_offset + 1, code_len);
    write_u32(&mut past_end_offset, last_at, code_len);
    rehash(&mut past_end_offset);
    assert_rejection(
        &past_end_offset,
        VerifyPhase::Function,
        "span offset is not an instruction boundary",
    );

    let mut nonascending_offsets = bytes;
    let previous = read_u32(&nonascending_offsets, span_offset_at(span_body, 1));
    write_u32(
        &mut nonascending_offsets,
        span_offset_at(span_body, 2),
        previous,
    );
    rehash(&mut nonascending_offsets);
    assert_rejection(
        &nonascending_offsets,
        VerifyPhase::Table,
        "span offsets must strictly ascend",
    );
}
