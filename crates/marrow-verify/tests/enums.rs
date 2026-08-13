//! Enum-image verification: a well-formed enum-bearing image seals, and each
//! single enum-table or enum-opcode defect rejects at the phase that owns it.
//! Images are minted with `ImageDraft`; since the coherence hoist the producer
//! refuses out-of-range enum references itself (the pinned flips live in
//! `legacy_ok_pins.rs`), so those cases assert the producer refusal here while
//! every defect the producer still emits stays the verifier's own rejection.

use marrow_image::{
    CollectionTypeDef, DraftTxn, EnumId, EnumTypeDef, ExportId, FunctionDef, ImageBuildError,
    ImageDraft, ImageType, Instr, Scalar, SpanEntry, VariantDef,
};
use marrow_verify::verify;

/// The armed transaction a fresh savepoint admits over `owner`.
fn admitted(owner: &mut ImageDraft) -> DraftTxn<'_> {
    owner
        .begin_transaction(owner.savepoint())
        .expect("a fresh savepoint admits")
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

/// Add a `Shape { dot, circle(int), rect(int, int) }` enum to `draft`.
fn shape(draft: &mut DraftTxn<'_>) -> EnumId {
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

/// Encode `draft` (adding `f` as a storeless export over `code` returning `ret`)
/// and verify, returning the rejection code or `"VERIFIED"`.
fn verify_fn(
    mut owner: ImageDraft,
    params: Vec<ImageType>,
    ret: ImageType,
    code: Vec<Instr>,
) -> String {
    let mut draft = admitted(&mut owner);
    let name = draft.intern_string("f").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let local_count = params.len() as u16 + 4;
    let func = draft
        .add_function(FunctionDef {
            name,
            source,
            params,
            ret,
            local_count,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
    let bytes = draft.encode().expect("encode").bytes;
    verify(&bytes)
        .err()
        .map(|r| r.code().to_string())
        .unwrap_or_else(|| "VERIFIED".to_string())
}

/// Add the same storeless export shape as [`verify_fn`] and return the producer's
/// own verdict, for the defects the coherence hoist refuses before any byte exists.
fn encode_fn(
    mut owner: ImageDraft,
    params: Vec<ImageType>,
    ret: ImageType,
    code: Vec<Instr>,
) -> Result<(), ImageBuildError> {
    let mut draft = admitted(&mut owner);
    let name = draft.intern_string("f").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let local_count = params.len() as u16 + 4;
    let func = draft
        .add_function(FunctionDef {
            name,
            source,
            params,
            ret,
            local_count,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.encode().map(|_| ())
}

#[test]
fn a_well_formed_enum_image_verifies() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let enum_idx = shape(&mut draft);
    let two = draft.intern_int(2).expect("a within-domain mint");
    // f(): int = Shape::circle(2) then read its payload leaf.
    let code = vec![
        Instr::ConstLoad(two),
        Instr::EnumConstruct {
            enum_idx,
            variant: 1,
        },
        Instr::EnumPayloadGet {
            variant: 1,
            field: 0,
        },
        Instr::Return,
    ];
    assert_eq!(
        verify_fn(
            {
                draft.commit();
                draft_owner
            },
            vec![],
            ImageType::scalar(Scalar::Int),
            code
        ),
        "VERIFIED"
    );
}

/// Flipped by the coherence hoist (the pinned flip lives in `legacy_ok_pins.rs`):
/// an out-of-range enum parameter reference is refused by the producer.
#[test]
fn an_enum_param_index_out_of_range_is_refused_by_the_producer() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let _ = shape(&mut draft); // one enum exists (index 0)
    // A parameter references enum index 7, which is out of range.
    let code = vec![Instr::Return];
    assert_eq!(
        encode_fn(
            {
                draft.commit();
                draft_owner
            },
            vec![ImageType::Enum {
                idx: EnumId::from_index(7),
                optional: false,
            }],
            ImageType::Unit,
            code,
        ),
        Err(ImageBuildError::InvalidReference("enum type")),
    );
}

/// Flipped by the coherence hoist (the pinned flip lives in `legacy_ok_pins.rs`):
/// an out-of-range enum return reference is refused by the producer.
#[test]
fn an_enum_return_index_out_of_range_is_refused_by_the_producer() {
    let draft = ImageDraft::new(); // no enums at all
    let code = vec![Instr::Return];
    assert_eq!(
        encode_fn(
            draft,
            vec![],
            ImageType::Enum {
                idx: marrow_image::EnumId::from_index(0),
                optional: false,
            },
            code,
        ),
        Err(ImageBuildError::InvalidReference("enum type")),
    );
}

#[test]
fn a_duplicate_variant_name_rejects_at_table() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let name = draft.intern_string("E").expect("a within-domain mint");
    let a = draft.intern_string("a").expect("a within-domain mint");
    draft
        .add_enum_type(EnumTypeDef {
            name,
            variants: vec![
                VariantDef {
                    name: a,
                    category: false,
                    payload: vec![],
                },
                VariantDef {
                    name: a, // same name string index
                    category: false,
                    payload: vec![],
                },
            ],
        })
        .expect("a within-domain mint");
    let code = vec![Instr::Return];
    assert_eq!(
        verify_fn(
            {
                draft.commit();
                draft_owner
            },
            vec![],
            ImageType::Unit,
            code
        ),
        "image.table"
    );
}

/// Flipped by the coherence hoist (the pinned flip lives in `legacy_ok_pins.rs`):
/// a construct variant outside the resolved enum is refused by the producer.
#[test]
fn an_out_of_range_construct_variant_is_refused_by_the_producer() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let enum_idx = shape(&mut draft);
    // Shape has 3 variants; constructing variant 9 is out of range.
    let code = vec![
        Instr::EnumConstruct {
            enum_idx,
            variant: 9,
        },
        Instr::Return,
    ];
    assert_eq!(
        encode_fn(
            {
                draft.commit();
                draft_owner
            },
            vec![],
            ImageType::Enum {
                idx: marrow_image::EnumId::from_index(0),
                optional: false,
            },
            code,
        ),
        Err(ImageBuildError::InvalidReference("enum type")),
    );
}

#[test]
fn an_out_of_range_payload_field_rejects_at_function() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let enum_idx = shape(&mut draft);
    let two = draft.intern_int(2).expect("a within-domain mint");
    // circle has one payload field (index 0); reading field 5 is out of range.
    let code = vec![
        Instr::ConstLoad(two),
        Instr::EnumConstruct {
            enum_idx,
            variant: 1,
        },
        Instr::EnumPayloadGet {
            variant: 1,
            field: 5,
        },
        Instr::Return,
    ];
    assert_eq!(
        verify_fn(
            {
                draft.commit();
                draft_owner
            },
            vec![],
            ImageType::scalar(Scalar::Int),
            code
        ),
        "image.function"
    );
}

#[test]
fn a_collection_enum_payload_leaf_rejects_at_table() {
    // The payload-shape contract admits a bare scalar, record, or enum enum-payload
    // leaf; a collection is not one. A tampered image whose variant payload names a
    // `List` collection type is refused at the phase that owns the ENUMS table, so the
    // compiler's check-time refusal of the same shape is defense in depth, not the
    // trust boundary.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let list_int = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        })
        .expect("a within-domain mint");
    let name = draft.intern_string("Holder").expect("a within-domain mint");
    let wrap = draft.intern_string("wrap").expect("a within-domain mint");
    draft
        .add_enum_type(EnumTypeDef {
            name,
            variants: vec![VariantDef {
                name: wrap,
                category: false,
                payload: vec![ImageType::Collection {
                    idx: list_int,
                    optional: false,
                }],
            }],
        })
        .expect("a within-domain mint");
    let code = vec![Instr::Return];
    assert_eq!(
        verify_fn(
            {
                draft.commit();
                draft_owner
            },
            vec![],
            ImageType::Unit,
            code
        ),
        "image.table"
    );
}

#[test]
fn a_truncated_enum_table_rejects_at_envelope() {
    // A valid enum image with its final byte flipped but not rehashed rejects at
    // the envelope; truncating the trailing ENUMS section corrupts the digest.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let _ = shape(&mut draft);
    let name = draft.intern_string("f").expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let code = vec![Instr::Return];
    let func = draft
        .add_function(FunctionDef {
            name,
            source,
            params: vec![],
            ret: ImageType::Unit,
            local_count: 0,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
    let mut bytes = draft.encode().expect("encode").bytes;
    bytes.truncate(bytes.len() - 2);
    assert_eq!(
        verify(&bytes)
            .err()
            .map(|r| r.code().to_string())
            .unwrap_or_default(),
        "image.envelope"
    );
}
