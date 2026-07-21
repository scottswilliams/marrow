//! IDTRAV01 forged-image hostiles: an entry-identity column as a bounded-traversal or
//! family-probe ancestor key-path. The verifier's ancestor pop admits an identity column
//! but re-proves its root against the traversal site independently of the compiler. A
//! well-formed identity ancestor over the traversed layer's own root verifies; an identity
//! minted over a foreign root — the cross-root confusion a forged image would smuggle —
//! rejects even though both roots carry the same key scalar, so the site's key-column type
//! check alone cannot distinguish them.

use marrow_image::{
    CollectionTypeDef, DurableMemberDef, DurableValueShape, ExportId, FieldDef, FunctionDef,
    ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity,
    Scalar, SemanticPath, SemanticStep, SemanticStepKind, SiteDef, SpanEntry,
};
use marrow_verify::{VerifyPhase, verify};

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
// Root A ("books"): an int key, a text field, and a `notes(text)` branch.
const A_PLACEMENT: [u8; 16] = [0x0b; 16];
const A_PRODUCT: [u8; 16] = [0x0d; 16];
const A_KEY: [u8; 16] = [0x0c; 16];
const A_FIELD: [u8; 16] = [0x0e; 16];
const A_BRANCH_PLACEMENT: [u8; 16] = [0x30; 16];
const A_BRANCH_KEY: [u8; 16] = [0x31; 16];
const A_BRANCH_FIELD: [u8; 16] = [0x32; 16];
// Root B ("tallies"): a distinct identity block, also an int key so only the root — never
// the key scalar — distinguishes an identity minted over it.
const B_PLACEMENT: [u8; 16] = [0x1b; 16];
const B_PRODUCT: [u8; 16] = [0x1d; 16];
const B_KEY: [u8; 16] = [0x1c; 16];
const B_FIELD: [u8; 16] = [0x1e; 16];

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

/// Build root A ("books", int key, text field, `notes(text)` branch) at RootId 0 and root B
/// ("tallies", int key, int field) at RootId 1, plus the branch-entry site and the
/// `List[text]` frozen-key collection a `notes` traversal freezes. Returns the branch site
/// index and the list-type index.
fn two_root_branch_draft(draft: &mut ImageDraft) -> (u16, u16) {
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));

    let book = draft.intern_string("Book");
    let title = draft.intern_string("title");
    let a_record = draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![FieldDef {
            name: title,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let notes = draft.intern_string("notes");
    let notes_qualified = draft.intern_string("Book.notes");
    let notes_text = draft.intern_string("text");
    let notes_record = draft.add_record_type(RecordTypeDef {
        name: notes_qualified,
        fields: vec![FieldDef {
            name: notes_text,
            ty: ImageType::scalar(Scalar::Text),
            required: true,
        }],
    });
    let a_root = draft.intern_string("books");
    draft.add_root(RootDef {
        name: a_root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes(A_KEY),
        }],
        record: a_record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(A_PLACEMENT),
            product: LedgerIdBytes::from_bytes(A_PRODUCT),
            indexes: Vec::new(),
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes(A_FIELD),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
                DurableMemberDef::Branch {
                    placement: LedgerIdBytes::from_bytes(A_BRANCH_PLACEMENT),
                    name: notes,
                    record: notes_record,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes(A_BRANCH_KEY),
                    }],
                    members: vec![DurableMemberDef::Field {
                        id: LedgerIdBytes::from_bytes(A_BRANCH_FIELD),
                        required: true,
                        value: DurableValueShape::Scalar(Scalar::Text),
                    }],
                },
            ],
        },
    });

    let tally = draft.intern_string("Tally");
    let count = draft.intern_string("count");
    let b_record = draft.add_record_type(RecordTypeDef {
        name: tally,
        fields: vec![FieldDef {
            name: count,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let b_root = draft.intern_string("tallies");
    draft.add_root(RootDef {
        name: b_root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes(B_KEY),
        }],
        record: b_record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(B_PLACEMENT),
            product: LedgerIdBytes::from_bytes(B_PRODUCT),
            indexes: Vec::new(),
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes(B_FIELD),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });

    let branch_site = draft
        .add_site(SiteDef::whole_payload(branch_entry_path()))
        .index();
    let list_ty = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Text),
        })
        .index();
    (branch_site, list_ty)
}

/// The whole-payload path of root A's `notes` branch entry: application -> root placement
/// -> branch placement.
fn branch_entry_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(A_PLACEMENT),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(A_BRANCH_PLACEMENT),
        ),
    ])
}

fn build_export(draft: &mut ImageDraft, code: Vec<Instr>) {
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::Unit,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
}

/// A well-formed identity ancestor: `Id(^books, k)` spread into root A's one key column
/// locates the `notes` branch parent. The traversal's ancestor pop accepts the identity
/// column after re-proving its root is the branch's own root, so the image verifies — the
/// verify-level acceptance the checker relies on.
#[test]
fn an_identity_ancestor_over_the_traversed_root_verifies() {
    let mut draft = ImageDraft::new();
    let (branch_site, list_ty) = two_root_branch_draft(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity { root: 0, cols: 1 },
        Instr::IdentityKeyPath(1),
        Instr::DurIterateBounded {
            site: branch_site,
            limit: 3,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    build_export(&mut draft, code);
    assert!(
        verify(&draft.encode().expect("encode").bytes).is_ok(),
        "an identity ancestor over the branch's own root must verify",
    );
}

/// A forged image mints `Id(^tallies, k)` and feeds it as the `notes` traversal's ancestor
/// key-path. The identity's root (tallies, RootId 1) is not the branch's root (books, RootId
/// 0); both carry an int key, so the ancestor column's scalar matches and only the re-proof
/// of the identity's root catches the confusion. The bounded-traversal ancestor pop rejects
/// it independently of the compiler.
#[test]
fn a_cross_root_identity_traversal_ancestor_is_rejected() {
    let mut draft = ImageDraft::new();
    let (branch_site, list_ty) = two_root_branch_draft(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity { root: 1, cols: 1 },
        Instr::IdentityKeyPath(1),
        Instr::DurIterateBounded {
            site: branch_site,
            limit: 3,
            from: false,
            list_ty,
        },
        Instr::Pop,
        Instr::Pop,
        Instr::Return,
    ];
    build_export(&mut draft, code);
    let rejection = verify(&draft.encode().expect("encode").bytes)
        .expect_err("a foreign-root identity ancestor must be rejected");
    assert_eq!(
        rejection.phase(),
        VerifyPhase::Function,
        "the cross-root identity confusion is a per-function stack-effect rejection",
    );
    assert_eq!(
        rejection.detail(),
        "an entry identity keys a durable operation on a different store root",
    );
}

/// The family-probe sibling: `exists(^tallies-identity . notes)` — a `DurFamilyExists` whose
/// ancestor key-path is the same forged foreign-root identity. Its ancestor pop shares the
/// re-proof, so the cross-root confusion is rejected there too.
#[test]
fn a_cross_root_identity_family_probe_ancestor_is_rejected() {
    let mut draft = ImageDraft::new();
    let (branch_site, _list_ty) = two_root_branch_draft(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity { root: 1, cols: 1 },
        Instr::IdentityKeyPath(1),
        Instr::DurFamilyExists(branch_site),
        Instr::Pop,
        Instr::Return,
    ];
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let func = draft.add_function(FunctionDef {
        name,
        source: src,
        params: vec![ImageType::scalar(Scalar::Int)],
        ret: ImageType::Unit,
        local_count: 1,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", "f"), func);
    let rejection = verify(&draft.encode().expect("encode").bytes)
        .expect_err("a foreign-root identity family-probe ancestor must be rejected");
    assert_eq!(
        rejection.detail(),
        "an entry identity keys a durable operation on a different store root",
    );
}
