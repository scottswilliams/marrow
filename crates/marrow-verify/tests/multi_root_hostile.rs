//! MR01 forged-image hostiles for multi-root images. Each artifact carries a valid
//! encoder-computed digest and violates exactly one cross-root invariant, so it must
//! reject at the verifier stage that owns the invariant rather than at the digest gate.
//! These make the verifier's cross-root rejections conspicuous enforcement artifacts.

use marrow_image::{
    DurableMemberDef, DurableValueShape, ExportId, FieldDef, FunctionDef, ImageDraft, ImageType,
    Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity, Scalar, SemanticPath,
    SemanticStep, SemanticStepKind, SiteDef, SpanEntry,
};
use marrow_verify::{VerifyPhase, verify};

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
// Root A ("assets"): placement/product/key/field ledger ids.
const A_PLACEMENT: [u8; 16] = [0x0b; 16];
const A_PRODUCT: [u8; 16] = [0x0d; 16];
const A_KEY: [u8; 16] = [0x0c; 16];
const A_FIELD: [u8; 16] = [0x0e; 16];
// Root B ("tallies"): a fully distinct identity block.
const B_PLACEMENT: [u8; 16] = [0x1b; 16];
const B_PRODUCT: [u8; 16] = [0x1d; 16];
const B_KEY: [u8; 16] = [0x1c; 16];
const B_FIELD: [u8; 16] = [0x1e; 16];

fn placement_path(placement: [u8; 16]) -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(placement),
        ),
    ])
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

/// Add root A ("assets", int key, one int field) and root B (`b_name`, `b_key_scalar`
/// key, one int field). The two roots' whole-payload sites are added as site 0 (A) and
/// site 1 (B). Returns the site indices.
fn build_two_roots(draft: &mut ImageDraft, b_name: &str, b_key_scalar: Scalar) -> (u16, u16) {
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));

    let a_field_name = draft.intern_string("name");
    let a_rec_name = draft.intern_string("Asset");
    let a_rec = draft.add_record_type(RecordTypeDef {
        name: a_rec_name,
        fields: vec![FieldDef {
            name: a_field_name,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let a_root = draft.intern_string("assets");
    draft.add_root(RootDef {
        name: a_root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes(A_KEY),
        }],
        record: a_rec,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(A_PLACEMENT),
            product: LedgerIdBytes::from_bytes(A_PRODUCT),
            indexes: vec![],
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes(A_FIELD),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });

    let b_field_name = draft.intern_string("count");
    let b_rec_name = draft.intern_string("Tally");
    let b_rec = draft.add_record_type(RecordTypeDef {
        name: b_rec_name,
        fields: vec![FieldDef {
            name: b_field_name,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let b_root = draft.intern_string(b_name);
    draft.add_root(RootDef {
        name: b_root,
        keys: vec![KeyColumn {
            scalar: b_key_scalar,
            id: LedgerIdBytes::from_bytes(B_KEY),
        }],
        record: b_rec,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(B_PLACEMENT),
            product: LedgerIdBytes::from_bytes(B_PRODUCT),
            indexes: vec![],
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes(B_FIELD),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });

    let a_site = draft
        .add_site(SiteDef::whole_payload(placement_path(A_PLACEMENT)))
        .index();
    let b_site = draft
        .add_site(SiteDef::whole_payload(placement_path(B_PLACEMENT)))
        .index();
    (a_site, b_site)
}

fn build_export(draft: &mut ImageDraft, code: Vec<Instr>, params: Vec<ImageType>, ret: ImageType) {
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let local_count = params.len() as u16;
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params,
        ret,
        local_count,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
}

/// Two roots that resolve to the same name would share one physical cell family, so a
/// write to one silently overwrites the other. The verifier rejects the collision
/// independently of the compiler.
#[test]
fn two_roots_sharing_a_name_are_rejected() {
    let mut draft = ImageDraft::new();
    // Root B reuses root A's name "assets" (a distinct string index, same content).
    build_two_roots(&mut draft, "assets", Scalar::Int);
    build_export(
        &mut draft,
        vec![Instr::LocalGet(0), Instr::Return],
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::scalar(Scalar::Int),
    );
    let rejection = verify(&draft.encode().expect("encode").bytes)
        .expect_err("two roots sharing a name must be rejected");
    assert!(
        rejection.detail().contains("share a name"),
        "expected a root-name-collision rejection, got {rejection:?}"
    );
}

/// Two distinctly-named roots verify together (the collision check does not reject a
/// well-formed two-root image).
#[test]
fn two_distinct_roots_verify() {
    let mut draft = ImageDraft::new();
    build_two_roots(&mut draft, "tallies", Scalar::Int);
    build_export(
        &mut draft,
        vec![Instr::LocalGet(0), Instr::Return],
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::scalar(Scalar::Int),
    );
    assert!(
        verify(&draft.encode().expect("encode").bytes).is_ok(),
        "two distinctly-named roots must verify"
    );
}

/// A forged image mints an identity over root A ("assets") and reaches a durable site on
/// root B ("tallies") with it — the cross-root confusion the checker rejects. Both roots
/// carry an `int` key, so the site's key-column type check cannot distinguish them; the
/// verifier must reject the confusion on the identity's root rather than admit it.
#[test]
fn a_cross_root_identity_reaching_a_foreign_site_is_rejected() {
    let mut draft = ImageDraft::new();
    let (_a_site, b_site) = build_two_roots(&mut draft, "tallies", Scalar::Int);
    // Mint Id(^assets, k) then use it as the key-path of a DurExists on ^tallies.
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity { root: 0, cols: 1 },
        Instr::IdentityKeyPath(1),
        Instr::DurExists(b_site),
        Instr::Return,
    ];
    build_export(
        &mut draft,
        code,
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::scalar(Scalar::Bool),
    );
    let rejection = verify(&draft.encode().expect("encode").bytes)
        .expect_err("an identity minted over ^assets must not reach a ^tallies site");
    assert_eq!(
        rejection.phase(),
        VerifyPhase::Function,
        "the cross-root identity confusion is a per-function stack-effect rejection",
    );
    assert!(
        rejection.detail().contains("different store root"),
        "expected a cross-root identity rejection, got {rejection:?}",
    );
}
