//! IDTRAV01 forged-image hostiles: an entry-identity column as a bounded-traversal or
//! family-probe ancestor key-path. The verifier's ancestor pop admits an identity column
//! but re-proves its root against the traversal site independently of the compiler. A
//! well-formed identity ancestor over the traversed layer's own root verifies; an identity
//! minted over a foreign root — the cross-root confusion a forged image would smuggle —
//! rejects even though both roots carry the same key scalar, so the site's key-column type
//! check alone cannot distinguish them.
//!
//! Every site named here is minted through the construction seam's bind-then-request
//! protocol. That protocol has exactly one owner in the workspace and is included here
//! rather than copied.

use marrow_image::{
    CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId, FieldDef,
    FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, LegacyDraftSiteOperand,
    RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry,
};
use marrow_verify::{VerifyPhase, verify};

#[path = "../../marrow-image/tests/common/site_seam.rs"]
mod site_seam;
use site_seam::site;

#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

/// The armed transaction a fresh savepoint admits over `owner`.
fn admitted(owner: &mut ImageDraft) -> DraftTxn<'_> {
    owner
        .begin_transaction(owner.savepoint())
        .expect("a fresh savepoint admits")
}

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
// Root A ("books"): an int key, a text field, and a `notes(text)` branch.
const A_PLACEMENT: [u8; 16] = [0x0b; 16];
const A_PRODUCT: [u8; 16] = [0x0d; 16];
const A_KEY: [u8; 16] = [0x0c; 16];
const A_FIELD: [u8; 16] = [0x0e; 16];
const A_SUBTITLE_FIELD: [u8; 16] = [0x33; 16];
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

/// Build root A ("books", int key, a required text field and a sparse text field, and a
/// `notes(text)` branch) at RootId 0 and root B ("tallies", int key, int field) at RootId 1,
/// plus the branch-entry site, the `List[text]` frozen-key collection a `notes` traversal
/// freezes, and the sparse field's leaf site. Returns the branch site, the list type, and the
/// sparse field-leaf site.
fn two_root_branch_draft(
    draft: &mut DraftTxn<'_>,
) -> (
    LegacyDraftSiteOperand,
    marrow_image::CollTypeId,
    LegacyDraftSiteOperand,
) {
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));

    let book = draft.intern_string("Book");
    let title = draft.intern_string("title");
    let subtitle = draft.intern_string("subtitle");
    let a_record = draft.add_record_type(RecordTypeDef {
        name: book,
        fields: vec![
            FieldDef {
                name: title,
                ty: ImageType::scalar(Scalar::Text),
                required: true,
            },
            FieldDef {
                name: subtitle,
                ty: ImageType::scalar(Scalar::Text),
                required: false,
            },
        ],
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
    let text_value = draft.value_scalar(Scalar::Text);
    // Commands 0/1/2 are the Product's direct members; command 3 nests under the branch.
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(A_PRODUCT),
            a_record,
            vec![
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(A_FIELD),
                        required: true,
                        value: text_value,
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(A_SUBTITLE_FIELD),
                        required: false,
                        value: text_value,
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Branch {
                        placement: LedgerIdBytes::from_bytes(A_BRANCH_PLACEMENT),
                        name: notes,
                        record: notes_record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Text,
                            id: LedgerIdBytes::from_bytes(A_BRANCH_KEY),
                        }],
                    },
                },
                DeclarationMemberDef {
                    parent: Some(2),
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(A_BRANCH_FIELD),
                        required: true,
                        value: text_value,
                    },
                },
            ],
        )
        .expect("a well-formed declaration");
    let a = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(A_PRODUCT),
            RootOccurrenceDef {
                name: a_root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(A_KEY),
                }],
                placement: LedgerIdBytes::from_bytes(A_PLACEMENT),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");

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
    let int_value = draft.value_scalar(Scalar::Int);
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(B_PRODUCT),
            b_record,
            vec![DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Field {
                    id: LedgerIdBytes::from_bytes(B_FIELD),
                    required: true,
                    value: int_value,
                },
            }],
        )
        .expect("a well-formed declaration");
    draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(B_PRODUCT),
            RootOccurrenceDef {
                name: b_root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(B_KEY),
                }],
                placement: LedgerIdBytes::from_bytes(B_PLACEMENT),
                indexes: Vec::new().into(),
            },
        )
        .expect("the Product is declared");

    // Root A's direct members in declaration order: the title field, the sparse subtitle
    // field, then the `notes` branch.
    let members = draft
        .product_members(LedgerIdBytes::from_bytes(A_PRODUCT))
        .expect("root A's Product is declared");
    let branch_site = site(
        draft,
        a.occurrence(),
        members[2].path(),
        SemanticTarget::WholePayload,
    );
    let list_ty = draft.add_collection_type(CollectionTypeDef::List {
        elem: ImageType::scalar(Scalar::Text),
    });
    let subtitle_site = site(
        draft,
        a.occurrence(),
        members[1].path(),
        SemanticTarget::FieldLeaf,
    );
    (branch_site, list_ty, subtitle_site)
}

fn build_export(draft: &mut DraftTxn<'_>, code: Vec<Instr>) {
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let func = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::Unit,
            local_count: 1,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
}

/// A well-formed identity ancestor: `Id(^books, k)` spread into root A's one key column
/// locates the `notes` branch parent. The traversal's ancestor pop accepts the identity
/// column after re-proving its root is the branch's own root, so the image verifies — the
/// verify-level acceptance the checker relies on.
#[test]
fn an_identity_ancestor_over_the_traversed_root_verifies() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let (branch_site, list_ty, _subtitle_site) = two_root_branch_draft(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity {
            root: marrow_image::RootId::from_index(0),
            cols: 1,
        },
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let (branch_site, list_ty, _subtitle_site) = two_root_branch_draft(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity {
            root: marrow_image::RootId::from_index(1),
            cols: 1,
        },
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
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let (branch_site, _list_ty, _subtitle_site) = two_root_branch_draft(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity {
            root: marrow_image::RootId::from_index(1),
            cols: 1,
        },
        Instr::IdentityKeyPath(1),
        Instr::DurFamilyExists(branch_site),
        Instr::Pop,
        Instr::Return,
    ];
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let func = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::Unit,
            local_count: 1,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
    let rejection = verify(&draft.encode().expect("encode").bytes)
        .expect_err("a foreign-root identity family-probe ancestor must be rejected");
    assert_eq!(
        rejection.detail(),
        "an entry identity keys a durable operation on a different store root",
    );
}

/// The strict present-entry set sibling: `DurSetSparsePresent` reads its key-path from local
/// slots rather than the stack, so a forged image stores a foreign-root identity into the key
/// slot it names. The slot-type re-proof (`slot_keys_column`) rejects the cross-root confusion
/// during the per-instruction pass — before the flow-phase presence check runs — so a
/// well-typed but foreign-root identity slot never reaches a durable field write.
#[test]
fn a_cross_root_identity_key_slot_in_a_strict_set_is_rejected() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let (_branch_site, _list_ty, subtitle_site) = two_root_branch_draft(&mut draft);
    let value = draft.intern_text("x");
    // Mint Id(^tallies, k) into slot 1, then name slot 1 as the strict set's key-path over a
    // ^books field site. Both roots key on int, so only the identity's root distinguishes them.
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity {
            root: marrow_image::RootId::from_index(1),
            cols: 1,
        },
        Instr::IdentityKeyPath(1),
        Instr::LocalSet(1),
        Instr::ConstLoad(value),
        Instr::SomeWrap,
        Instr::DurSetSparsePresent {
            site: subtitle_site,
            key_slots: vec![1],
        },
        Instr::Return,
    ];
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("f");
    let func = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::Unit,
            local_count: 2,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
    let rejection = verify(&draft.encode().expect("encode").bytes)
        .expect_err("a foreign-root identity key slot must be rejected");
    assert_eq!(
        rejection.phase(),
        VerifyPhase::Function,
        "the slot-type re-proof is a per-function stack-effect rejection",
    );
    assert_eq!(
        rejection.detail(),
        "set-sparse-present key slot has the wrong type",
    );
}
