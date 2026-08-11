//! Boundary known-answer tests: the narrow width bounds that did NOT widen
//! with the record field width still bite at exactly their chosen value. The dense
//! inline-composite leaf count admits `MAX_STRUCT_LEAVES` and refuses one more; the
//! index projection width admits `MAX_INDEX_COMPONENTS` and refuses one more. These
//! exercise the encoder's `check_bounds` recheck (the same constants the independent
//! verifier rechecks), so a future re-coupling to the widened record width is
//! conspicuous, not silent.

use marrow_image::bounds::{MAX_INDEX_COMPONENTS, MAX_STRUCT_LEAVES};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, DurableIndexComponent, DurableIndexShape,
    ExportId, FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes,
    RecordTypeDef, RootOccurrenceDef, Scalar, SpanEntry,
};

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const KEY_ID: [u8; 16] = [0x0c; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
const FIELD_ID: [u8; 16] = [0x0e; 16];
const INDEX_ID: [u8; 16] = [0x3b; 16];

/// A distinct 16-byte ledger id seeded by `n` (its low byte), for the many-component
/// index projections below. Kept below the reserved fixed ids above.
fn component_id(n: usize) -> LedgerIdBytes {
    // Two seed bytes carry the distinctness this helper promises. Past them the ids
    // silently repeat and a bound test would pass over a smaller set than it named.
    assert!(n <= u16::MAX as usize, "component seed exceeds its two bytes");
    let mut bytes = [0x40u8; 16];
    bytes[0] = n as u8;
    bytes[1] = (n >> 8) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// A minimal encodable draft carrying a `main` returning `0`, one record type, and one
/// keyed root whose Product declares `members` and whose occurrence carries `indexes`.
/// The declared member graph and the index shapes are the only things the callers vary,
/// so the bound under test is the sole reason an encode fails.
fn encode_root(
    members: impl FnOnce(&mut ImageDraft) -> Vec<DeclarationMemberDef>,
    indexes: Vec<DurableIndexShape>,
) -> Result<(), ImageBuildError> {
    let mut draft = ImageDraft::new();
    let members = members(&mut draft);
    let type_name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields: Vec::new(),
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let root_name = draft.intern_string("r");
    draft
        .declare_product(LedgerIdBytes::from_bytes(PRODUCT_ID), record, members)
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes,
            },
        )
        .expect("the Product is declared");
    let src = draft.intern_string("src/main.mw");
    let main_name = draft.intern_string("main");
    let zero = draft.intern_int(0);
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let spans = (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect();
    let main = draft
        .add_function(FunctionDef {
            name: main_name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            code,
            spans,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.encode().map(|_| ())
}

/// A Product whose single field member carries a dense struct value of `leaves` scalar
/// leaves.
fn members_with_struct_field(draft: &mut ImageDraft, leaves: usize) -> Vec<DeclarationMemberDef> {
    let values = draft.value_shapes_mut();
    let int = values.scalar(Scalar::Int);
    let value = values.struct_shape(vec![int; leaves]);
    vec![DeclarationMemberDef {
        parent: None,
        shape: DeclarationMemberShape::Field {
            id: LedgerIdBytes::from_bytes(FIELD_ID),
            required: false,
            value,
        },
    }]
}

/// One nonunique index projecting `components` field components.
fn indexes_with_components(components: usize) -> Vec<DurableIndexShape> {
    vec![DurableIndexShape {
        id: LedgerIdBytes::from_bytes(INDEX_ID),
        unique: false,
        components: (0..components)
            .map(|n| DurableIndexComponent::Field(component_id(n)))
            .collect(),
    }]
}

#[test]
fn a_dense_struct_at_the_leaf_limit_encodes() {
    assert_eq!(
        encode_root(
            |draft| members_with_struct_field(draft, MAX_STRUCT_LEAVES),
            Vec::new(),
        ),
        Ok(()),
        "a struct value of exactly MAX_STRUCT_LEAVES leaves is admitted",
    );
}

#[test]
fn a_dense_struct_one_leaf_over_the_limit_is_refused() {
    assert_eq!(
        encode_root(
            |draft| members_with_struct_field(draft, MAX_STRUCT_LEAVES + 1),
            Vec::new(),
        ),
        Err(ImageBuildError::TooManyStructLeaves),
        "one leaf past the dense-composite limit is refused as TooManyStructLeaves",
    );
}

/// The value bounds are rechecked over the whole arena, not over the shapes a
/// declaration happens to reference.
///
/// The arena is the draft's own retained state. A node past a value bound is a producer
/// defect wherever it came from, and deciding it by reachability would make the same draft
/// encode or refuse depending on a traversal — while paying for a reachability walk to
/// learn something no correct producer can produce. This pins the declared precondition:
/// a draft whose declarations are all within bounds still refuses when its arena holds an
/// over-wide shape nothing references.
#[test]
fn an_over_wide_shape_no_declaration_references_still_refuses() {
    assert_eq!(
        encode_root(
            |draft| {
                let members = members_with_struct_field(draft, MAX_STRUCT_LEAVES);
                let values = draft.value_shapes_mut();
                let int = values.scalar(Scalar::Int);
                let _unreferenced = values.struct_shape(vec![int; MAX_STRUCT_LEAVES + 1]);
                members
            },
            Vec::new(),
        ),
        Err(ImageBuildError::TooManyStructLeaves),
        "an unreferenced over-wide shape in the draft's arena refuses the encode",
    );
}

#[test]
fn an_index_at_the_component_limit_encodes() {
    assert_eq!(
        encode_root(
            |_| Vec::new(),
            indexes_with_components(MAX_INDEX_COMPONENTS)
        ),
        Ok(()),
        "an index projecting exactly MAX_INDEX_COMPONENTS components is admitted",
    );
}

#[test]
fn an_index_one_component_over_the_limit_is_refused() {
    assert_eq!(
        encode_root(
            |_| Vec::new(),
            indexes_with_components(MAX_INDEX_COMPONENTS + 1)
        ),
        Err(ImageBuildError::TooManyIndexComponents),
        "one component past the projection limit is refused as TooManyIndexComponents",
    );
}
