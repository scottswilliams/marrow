//! Forged-image hostiles for the durable enum-reuse identity law. An enum's sum and
//! member ledger ids are the identity of the enum *declaration*: two durable fields of
//! the same enum type reference that one identity, and the verifier must accept the
//! reuse. The guard narrows — it does not vanish: a genuinely duplicated declaration id
//! (a repeated field id, or a field id colliding with an enum's sum id) and an
//! inconsistent enum reference (the same sum id presenting different member ids) must
//! still reject at the table phase. Each artifact carries a valid encoder-computed
//! digest, so it exercises the verifier's own decode rather than the digest gate.

use marrow_image::{
    DurableMemberDef, DurableValueShape, EnumTypeDef, ExportId, FieldDef, FunctionDef, ImageDraft,
    ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity, Scalar,
    SemanticPath, SemanticStep, SemanticStepKind, SiteDef, SpanEntry, VariantDef,
};
use marrow_verify::{VerifyPhase, verify};

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT: [u8; 16] = [0x0b; 16];
const PRODUCT: [u8; 16] = [0x0d; 16];
const KEY: [u8; 16] = [0x0c; 16];
const FIELD_ONE: [u8; 16] = [0x0e; 16];
const FIELD_TWO: [u8; 16] = [0x0f; 16];
// The one enum declaration's durable identity: its sum and per-member ids.
const SUM: [u8; 16] = [0x50; 16];
const MEMBER_READER: [u8; 16] = [0x51; 16];
const MEMBER_WRITER: [u8; 16] = [0x52; 16];
const MEMBER_ADMIN: [u8; 16] = [0x53; 16];

fn id(bytes: [u8; 16]) -> LedgerIdBytes {
    LedgerIdBytes::from_bytes(bytes)
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

/// Add the enum type `Access { reader, writer, admin }` (all empty-payload) and return
/// its table index.
fn access_enum(draft: &mut ImageDraft) -> u16 {
    let name = draft.intern_string("Access");
    let reader = draft.intern_string("reader");
    let writer = draft.intern_string("writer");
    let admin = draft.intern_string("admin");
    draft
        .add_enum_type(EnumTypeDef {
            name,
            variants: vec![
                VariantDef {
                    name: reader,
                    category: false,
                    payload: vec![],
                },
                VariantDef {
                    name: writer,
                    category: false,
                    payload: vec![],
                },
                VariantDef {
                    name: admin,
                    category: false,
                    payload: vec![],
                },
            ],
        })
        .index()
}

/// The durable value shape of an `Access` field, parameterized on its member ids so a
/// hostile can present the same `sum` with an altered member set.
fn access_shape(sum: [u8; 16], members: [[u8; 16]; 3]) -> DurableValueShape {
    DurableValueShape::Enum {
        sum: id(sum),
        members: members
            .iter()
            .map(|m| marrow_image::DurableEnumMemberShape {
                id: id(*m),
                payload: vec![],
            })
            .collect(),
    }
}

/// A one-root image ("grants", int key) whose `Access` record carries two enum fields.
/// Each field's ledger id and durable enum value shape are supplied so a hostile can
/// forge a duplicate field id or an inconsistent enum reference.
fn build(field_one: [u8; 16], shape_one: DurableValueShape, field_two: [u8; 16], shape_two: DurableValueShape) -> Vec<u8> {
    let mut draft = ImageDraft::new();
    draft.set_application_identity(id(APPLICATION_ID));
    let enum_idx = access_enum(&mut draft);
    let ty = ImageType::Enum {
        idx: enum_idx,
        optional: false,
    };
    let first = draft.intern_string("first");
    let second = draft.intern_string("second");
    let rec_name = draft.intern_string("Grant");
    let record = draft.add_record_type(RecordTypeDef {
        name: rec_name,
        fields: vec![
            FieldDef {
                name: first,
                ty,
                required: true,
            },
            FieldDef {
                name: second,
                ty,
                required: true,
            },
        ],
    });
    let root_name = draft.intern_string("grants");
    draft.add_root(RootDef {
        name: root_name,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: id(KEY),
        }],
        record,
        identity: RootIdentity {
            placement: id(PLACEMENT),
            product: id(PRODUCT),
            indexes: vec![],
            members: vec![
                DurableMemberDef::Field {
                    id: id(field_one),
                    required: true,
                    value: shape_one,
                },
                DurableMemberDef::Field {
                    id: id(field_two),
                    required: true,
                    value: shape_two,
                },
            ],
        },
    });
    draft.add_site(SiteDef::whole_payload(SemanticPath::from_steps(vec![
        SemanticStep::new(SemanticStepKind::Application, id(APPLICATION_ID)),
        SemanticStep::new(SemanticStepKind::Placement, id(PLACEMENT)),
    ])));
    let src = draft.intern_string("src/main.mw");
    let fname = draft.intern_string("f");
    let code = vec![Instr::LocalGet(0), Instr::Return];
    let func = draft.add_function(FunctionDef {
        name: fname,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::scalar(Scalar::Int),
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    draft.encode().expect("encode").bytes
}

/// The reference shape: one enum declaration reused by two fields. Both fields present
/// the identical sum and member ids — the honest reuse the verifier must accept.
fn shared() -> (DurableValueShape, DurableValueShape) {
    let members = [MEMBER_READER, MEMBER_WRITER, MEMBER_ADMIN];
    (access_shape(SUM, members), access_shape(SUM, members))
}

#[test]
fn two_fields_of_one_enum_type_verify() {
    let (one, two) = shared();
    let bytes = build(FIELD_ONE, one, FIELD_TWO, two);
    assert!(
        verify(&bytes).is_ok(),
        "two durable fields of one enum type must verify"
    );
}

#[test]
fn a_repeated_field_id_still_rejects() {
    // Genuine duplicate: the second field claims the first field's ledger id.
    let (one, two) = shared();
    let rejection =
        verify(&build(FIELD_ONE, one, FIELD_ONE, two)).expect_err("a repeated field id must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert!(
        rejection.detail().contains("duplicate durable ledger id"),
        "expected a duplicate-ledger-id rejection, got {rejection:?}"
    );
}

#[test]
fn a_field_id_colliding_with_the_enum_sum_still_rejects() {
    // Genuine duplicate across identity kinds: the second field's ledger id equals the
    // enum's sum id, which the first field's value shape already claimed.
    let (one, two) = shared();
    let rejection = verify(&build(FIELD_ONE, one, SUM, two))
        .expect_err("a field id colliding with an enum sum id must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert!(
        rejection.detail().contains("duplicate durable ledger id"),
        "expected a duplicate-ledger-id rejection, got {rejection:?}"
    );
}

#[test]
fn an_inconsistent_enum_reference_rejects() {
    // The second field reuses the sum id but presents a different member id: a forged
    // identity that claims to be the same enum while carrying a different member set.
    let one = access_shape(SUM, [MEMBER_READER, MEMBER_WRITER, MEMBER_ADMIN]);
    let two = access_shape(SUM, [MEMBER_READER, MEMBER_WRITER, [0x99; 16]]);
    let rejection = verify(&build(FIELD_ONE, one, FIELD_TWO, two))
        .expect_err("an inconsistent enum reference must reject");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert!(
        rejection.detail().contains("different member set"),
        "expected an inconsistent-enum-reuse rejection, got {rejection:?}"
    );
}
