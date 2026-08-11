//! MR01 forged-image hostiles for multi-root images. Each artifact carries a valid
//! encoder-computed digest and violates exactly one cross-root invariant, so it must
//! reject at the verifier stage that owns the invariant rather than at the digest gate.
//! These make the verifier's cross-root rejections conspicuous enforcement artifacts.

use marrow_image::{
    DurableMemberDef, DurableValueShape, ExportId, FieldDef, FunctionDef, ImageBuildError,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, LegacyDraftSiteOperand, RecordTypeDef,
    RootDef, RootIdentity, Scalar, SemanticPath, SiteDef, SpanEntry, TypeId, image_id,
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
    SemanticPath::root(
        LedgerIdBytes::from_bytes(APPLICATION_ID),
        LedgerIdBytes::from_bytes(placement),
    )
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
fn build_two_roots(
    draft: &mut ImageDraft,
    b_name: &str,
    b_key_scalar: Scalar,
) -> (LegacyDraftSiteOperand, LegacyDraftSiteOperand) {
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

    let a_site = draft.request_site(SiteDef::whole_payload(placement_path(A_PLACEMENT)));
    let b_site = draft.request_site(SiteDef::whole_payload(placement_path(B_PLACEMENT)));
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

// --- Shared Product declarations: the accepted reference and its refusals -------------

/// Two roots over **one** Product: the same product identity, the same member graph, and
/// the same entry record, with their own placements, key tuples, and names. Root B's
/// draft rows are built from root A's declaration, exactly as the compiler builds them.
fn build_shared_product(draft: &mut ImageDraft) -> TypeId {
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let field_name = draft.intern_string("name");
    let rec_name = draft.intern_string("Asset");
    let record = draft.add_record_type(RecordTypeDef {
        name: rec_name,
        fields: vec![FieldDef {
            name: field_name,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    // A second record type, so a forged entry-record index is in range and the rejection
    // is the Product's, not a bounds check.
    let other_name = draft.intern_string("Other");
    draft.add_record_type(RecordTypeDef {
        name: other_name,
        fields: vec![],
    });
    let members = || {
        vec![DurableMemberDef::Field {
            id: LedgerIdBytes::from_bytes(A_FIELD),
            required: true,
            value: DurableValueShape::Scalar(Scalar::Int),
        }]
    };
    for (name, placement, key) in [
        ("assets", A_PLACEMENT, A_KEY),
        ("tallies", B_PLACEMENT, B_KEY),
    ] {
        let root_name = draft.intern_string(name);
        draft.add_root(RootDef {
            name: root_name,
            keys: vec![KeyColumn {
                scalar: Scalar::Int,
                id: LedgerIdBytes::from_bytes(key),
            }],
            record,
            identity: RootIdentity {
                placement: LedgerIdBytes::from_bytes(placement),
                product: LedgerIdBytes::from_bytes(A_PRODUCT),
                indexes: vec![],
                members: members(),
            },
        });
        draft.request_site(SiteDef::whole_payload(placement_path(placement)));
    }
    record
}

/// A shared-Product image with one storeless export.
fn shared_product_image() -> Vec<u8> {
    let mut draft = ImageDraft::new();
    build_shared_product(&mut draft);
    build_export(
        &mut draft,
        vec![Instr::LocalGet(0), Instr::Return],
        vec![ImageType::scalar(Scalar::Int)],
        ImageType::scalar(Scalar::Int),
    );
    draft.encode().expect("encode").bytes
}

/// Recompute and rewrite the image digest over the payload (every byte after the slot),
/// so a forged artifact reaches the table phase rather than stopping at the envelope.
fn rehash(bytes: &mut [u8]) {
    let id = image_id(&bytes[37..]);
    bytes[5..37].copy_from_slice(&id.0);
}

/// Overwrite the `nth` (0-based) occurrence of `needle` with `replacement` and rehash.
fn forge(bytes: &mut [u8], needle: &[u8], nth: usize, replacement: &[u8]) {
    assert_eq!(needle.len(), replacement.len());
    let at = bytes
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(offset, _)| offset)
        .nth(nth)
        .expect("the pattern occurs often enough to forge");
    bytes[at..at + needle.len()].copy_from_slice(replacement);
    rehash(bytes);
}

/// One Product declaration may serve two root occurrences: the repeated Product identity
/// is a reference to the declaration accepted at the first occurrence, not a duplicate
/// identity. The two roots keep their own placements, key ids, and names.
#[test]
fn two_roots_over_one_product_verify() {
    let image = verify(&shared_product_image()).expect("a shared Product must verify");
    assert_eq!(image.roots().len(), 2);
    assert_eq!(image.roots()[0].name(), "assets");
    assert_eq!(image.roots()[1].name(), "tallies");
    assert_eq!(
        image.roots()[0].record(),
        image.roots()[1].record(),
        "one declaration, one entry record"
    );
}

/// A forged image claims one Product identity from two roots while the second root
/// declares a different member graph — two declarations wearing one identity. The
/// verifier compares the repeated claim against the accepted declaration and refuses it.
#[test]
fn a_repeated_product_declaring_another_graph_is_rejected() {
    let mut bytes = shared_product_image();
    // The second root's field identity, forged to an id the declaration does not carry.
    forge(&mut bytes, &A_FIELD, 1, &[0x77; 16]);
    let rejection = verify(&bytes).expect_err("a divergent repeated Product must be rejected");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "a repeated durable Product declares a different member graph"
    );
}

/// The same graph with a different entry record is refused too: the materialized record
/// the roots execute over is a declaration fact, so a second occurrence may not rebind it.
#[test]
fn a_repeated_product_declaring_another_entry_record_is_rejected() {
    let mut bytes = shared_product_image();
    // Each root's record index precedes its placement; forge the second root's to the
    // spare record type. The root frame is `name ‖ keys ‖ record ‖ placement ‖ product`,
    // so patch the two bytes immediately before the second placement id.
    let at = bytes
        .windows(16)
        .position(|window| window == B_PLACEMENT)
        .expect("root B's placement is on the wire");
    bytes[at - 2..at].copy_from_slice(&1u16.to_be_bytes());
    rehash(&mut bytes);
    let rejection = verify(&bytes).expect_err("a rebound entry record must be rejected");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(
        rejection.detail(),
        "a repeated durable Product declares a different entry record"
    );
}

/// Two root rows carrying one placement identity are one durable node claimed twice. That
/// is a duplicate *occurrence*, and the verifier says so rather than reporting the generic
/// identity collision its ledger scan would otherwise find.
#[test]
fn a_duplicate_root_occurrence_is_rejected() {
    let mut bytes = shared_product_image();
    forge(&mut bytes, &B_PLACEMENT, 0, &A_PLACEMENT);
    let rejection = verify(&bytes).expect_err("a duplicate root occurrence must be rejected");
    assert_eq!(rejection.phase(), VerifyPhase::Table);
    assert_eq!(rejection.detail(), "duplicate durable root occurrence");
}

/// The producer half of the same law: the draft holds one declaration row per Product
/// identity, so a second occurrence that claims a different graph has nowhere to live.
/// The draft records the conflict and refuses to encode rather than canonicalizing one of
/// the two claims away — a divergent graph can never reach an artifact at all.
#[test]
fn a_draft_refuses_to_encode_two_graphs_under_one_product() {
    let mut draft = ImageDraft::new();
    let record = build_shared_product(&mut draft);
    let root_name = draft.intern_string("extra");
    draft.add_root(RootDef {
        name: root_name,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes([0x66; 16]),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes([0x67; 16]),
            product: LedgerIdBytes::from_bytes(A_PRODUCT),
            indexes: vec![],
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes(B_FIELD),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });
    assert_eq!(
        draft
            .encode()
            .expect_err("a divergent Product claim must not encode"),
        ImageBuildError::ProductGraphConflict
    );
}
