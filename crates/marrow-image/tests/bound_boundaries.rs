//! Boundary known-answer tests: the narrow width bounds that did NOT widen
//! with the record field width still bite at exactly their chosen value. The dense
//! inline-composite leaf count admits `MAX_STRUCT_LEAVES` and refuses one more; the
//! index projection width admits `MAX_INDEX_COMPONENTS` and refuses one more. These
//! exercise the encoder's `check_bounds` recheck (the same constants the independent
//! verifier rechecks), so a future re-coupling to the widened record width is
//! conspicuous, not silent.

use marrow_image::bounds::{MAX_INDEX_COMPONENTS, MAX_STRUCT_LEAVES};
use marrow_image::{
    DurableIndexComponent, DurableIndexShape, DurableMemberDef, DurableValueShape, ExportId,
    FunctionDef, ImageBuildError, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes,
    RecordTypeDef, RootDef, RootIdentity, Scalar, SpanEntry,
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
    let mut bytes = [0x40u8; 16];
    bytes[0] = n as u8;
    bytes[1] = (n >> 8) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// A minimal encodable draft carrying a `main` returning `0`, one record type, and
/// one keyed root whose identity is `identity`. The root's shape is the only thing
/// the callers vary, so the bound under test is the sole reason an encode fails.
fn encode_root(identity: RootIdentity) -> Result<(), ImageBuildError> {
    let mut draft = ImageDraft::new();
    let type_name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields: Vec::new(),
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let root_name = draft.intern_string("r");
    draft.add_root(RootDef {
        name: root_name,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes(KEY_ID),
        }],
        record,
        identity,
    });
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
    let main = draft.add_function(FunctionDef {
        name: main_name,
        source: src,
        params: Vec::new(),
        ret: ImageType::scalar(Scalar::Int),
        local_count: 0,
        code,
        spans,
    });
    draft.add_export(ExportId::of_local("", "main"), main);
    draft.encode().map(|_| ())
}

/// A root whose single field member carries a dense struct value of `leaves` scalar
/// leaves.
fn identity_with_struct_field(leaves: usize) -> RootIdentity {
    RootIdentity {
        placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
        product: LedgerIdBytes::from_bytes(PRODUCT_ID),
        members: vec![DurableMemberDef::Field {
            id: LedgerIdBytes::from_bytes(FIELD_ID),
            required: false,
            value: DurableValueShape::Struct(
                (0..leaves)
                    .map(|_| DurableValueShape::Scalar(Scalar::Int))
                    .collect(),
            ),
        }],
        indexes: Vec::new(),
    }
}

/// A root with one nonunique index projecting `components` field components.
fn identity_with_index(components: usize) -> RootIdentity {
    RootIdentity {
        placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
        product: LedgerIdBytes::from_bytes(PRODUCT_ID),
        members: Vec::new(),
        indexes: vec![DurableIndexShape {
            id: LedgerIdBytes::from_bytes(INDEX_ID),
            unique: false,
            components: (0..components)
                .map(|n| DurableIndexComponent::Field(component_id(n)))
                .collect(),
        }],
    }
}

#[test]
fn a_dense_struct_at_the_leaf_limit_encodes() {
    assert_eq!(
        encode_root(identity_with_struct_field(MAX_STRUCT_LEAVES)),
        Ok(()),
        "a struct value of exactly MAX_STRUCT_LEAVES leaves is admitted",
    );
}

#[test]
fn a_dense_struct_one_leaf_over_the_limit_is_refused() {
    assert_eq!(
        encode_root(identity_with_struct_field(MAX_STRUCT_LEAVES + 1)),
        Err(ImageBuildError::TooManyStructLeaves),
        "one leaf past the dense-composite limit is refused as TooManyStructLeaves",
    );
}

#[test]
fn an_index_at_the_component_limit_encodes() {
    assert_eq!(
        encode_root(identity_with_index(MAX_INDEX_COMPONENTS)),
        Ok(()),
        "an index projecting exactly MAX_INDEX_COMPONENTS components is admitted",
    );
}

#[test]
fn an_index_one_component_over_the_limit_is_refused() {
    assert_eq!(
        encode_root(identity_with_index(MAX_INDEX_COMPONENTS + 1)),
        Err(ImageBuildError::TooManyIndexComponents),
        "one component past the projection limit is refused as TooManyIndexComponents",
    );
}
