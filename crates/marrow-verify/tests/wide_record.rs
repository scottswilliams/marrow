//! WR01 verifier re-check: the independent verifier admits a record type at the
//! widened top-level field width ([`MAX_RECORD_FIELDS`]), and the encoder still
//! refuses one field beyond it. The verifier and encoder share one bounds owner, so
//! this pins that the widened law-9 width holds on both sides of the trust boundary.

use marrow_image::bounds::MAX_RECORD_FIELDS;
use marrow_image::{
    FieldDef, FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr, RecordTypeDef, Scalar,
    SpanEntry,
};
use marrow_verify::verify;

fn int() -> ImageType {
    ImageType::Scalar {
        scalar: Scalar::Int,
        optional: false,
    }
}

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

/// A draft carrying a `main` returning `0` and one record type of `field_count`
/// scalar fields.
fn draft_with_record(field_count: usize) -> ImageDraft {
    let mut draft = ImageDraft::new();
    let src = draft.intern_string("src/main.mw");
    let type_name = draft.intern_string("Wide");
    let fields = (0..field_count)
        .map(|i| FieldDef {
            name: draft.intern_string(&format!("f{i}")),
            ty: int(),
            required: false,
        })
        .collect();
    draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields,
    });
    let name = draft.intern_string("main");
    let zero = draft.intern_int(0);
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let spans = spans(&code);
    let main = draft.add_function(FunctionDef {
        name,
        source: src,
        params: Vec::new(),
        ret: int(),
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(marrow_image::ExportId::of_local("", "main"), main);
    draft
}

/// The independent verifier admits a record type at the full widened field width.
#[test]
fn the_verifier_admits_a_record_at_the_widened_field_width() {
    let image = draft_with_record(MAX_RECORD_FIELDS)
        .encode()
        .expect("a record at the widened width encodes");
    verify(&image.bytes).expect("the widened-width record verifies");
}

/// One field beyond the widened width is refused by the encoder's own bound recheck,
/// so the widened bound still bites — it did not become unbounded.
#[test]
fn one_field_beyond_the_width_is_refused() {
    let error = draft_with_record(MAX_RECORD_FIELDS + 1)
        .encode()
        .expect_err("a record one field past the width must be refused");
    assert_eq!(error, ImageBuildError::TooManyFields);
}
