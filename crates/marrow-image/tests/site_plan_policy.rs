//! The site table's capacity policy (red R17) and the demand key's identity (red R18).
//!
//! Every operation site is minted through one bounded plan. The plan checks vacant
//! capacity *before* it mints a numeric id, so a fitting site id is always inside
//! `0..=8191`; the first unique demand past the cap saturates the logical count at
//! `MAX_SITES + 1` and records one earliest policy receipt, while an already-retained
//! demand still reuses the id it was given.
//!
//! Before this, `add_site` was a bare push whose id was `self.sites.len() as u16`, with
//! the bound seen only at `encode()`. A producer could push past `u16::MAX`, receive a
//! wrapped id, and embed the aliased id in emitted instruction operands — two distinct
//! durable nodes silently sharing one site operand, discovered nowhere.
//!
//! Every demand here is built through the checked construction seam: a Product is
//! declared once, root occurrences are appended over it, and a site is named by binding
//! one occurrence, one canonical declaration path published by the draft, and the one
//! operation target that node admits. The demand key is `(occurrence, path, target)`, so
//! a distinct occurrence over one declaration path is a distinct demand. That
//! bind-then-request protocol has exactly one owner in the workspace and is included here
//! rather than copied.

use marrow_image::bounds::MAX_SITES;
use marrow_image::{
    AdmittedRoot, CanonicalDeclarationPathSelector, DeclarationMember, DeclarationMemberDef,
    DeclarationMemberShape, DurableIndexComponent, DurableIndexShape, DurableValueShape,
    ImageBuildError, ImageDraft, KeyColumn, LedgerIdBytes, LegacyDraftSiteOperand, RecordTypeDef,
    RootOccurrenceDef, Scalar, SemanticTarget,
};

#[path = "common/site_seam.rs"]
mod site_seam;
use site_seam::site;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
/// A field-member seed past every seed a wide declaration uses, so a divergent
/// redeclaration names a node the bound declaration does not hold.
const DIVERGENT_FIELD: usize = MAX_SITES + 1;

fn product() -> LedgerIdBytes {
    LedgerIdBytes::from_bytes(PRODUCT_ID)
}

/// A distinct 16-byte ledger id seeded by `n`, so every field member below is a distinct
/// declaration node and every demand over it a distinct `(occurrence, node, target)`.
fn field_id(n: usize) -> LedgerIdBytes {
    let mut bytes = [0x50u8; 16];
    bytes[0] = (n & 0xff) as u8;
    bytes[1] = ((n >> 8) & 0xff) as u8;
    bytes[2] = ((n >> 16) & 0xff) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// Declare one Product of `fields` required int fields — the cheapest way to reach a
/// wide distinct demand set, since a demand is named by a declaration node and every
/// field is its own node.
fn declare_wide_product(draft: &mut ImageDraft, fields: usize) {
    let type_name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields: Vec::new(),
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            product(),
            record,
            (0..fields)
                .map(|n| DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: field_id(n),
                        required: true,
                        value: DurableValueShape::Scalar(Scalar::Int),
                    },
                })
                .collect(),
        )
        .expect("a well-formed declaration");
}

/// Append one singleton root occurrence over the declared Product, seeded by `n` so each
/// root has its own spelling and placement.
fn admit_root(draft: &mut ImageDraft, n: u8) -> AdmittedRoot {
    let name = draft.intern_string(&format!("r{n}"));
    draft
        .add_root_occurrence(
            product(),
            RootOccurrenceDef {
                name,
                keys: Vec::new(),
                placement: LedgerIdBytes::from_bytes([n; 16]),
                indexes: Vec::new(),
            },
        )
        .expect("the Product is declared")
}

/// A draft holding one wide Product declaration, one root over it, and that Product's
/// direct members in declaration order.
fn wide_draft(fields: usize) -> (ImageDraft, AdmittedRoot, Vec<DeclarationMember>) {
    let mut draft = ImageDraft::new();
    declare_wide_product(&mut draft, fields);
    let root = admit_root(&mut draft, 0x21);
    let members = draft.product_members(product()).expect("declared");
    (draft, root, members)
}

/// Demand every field leaf of `members` under `root`.
fn demand_every_leaf(draft: &mut ImageDraft, root: &AdmittedRoot, members: &[DeclarationMember]) {
    for member in members {
        let _ = site(
            draft,
            root.occurrence(),
            member.path(),
            SemanticTarget::FieldLeaf,
        );
    }
}

/// Red R17, at the boundary an image is refused: a draft whose site demand crosses the
/// cap cannot be encoded, however many demands past it were refused.
///
/// The plan answers every excess demand with the over-policy operand, which carries no id
/// at all, and saturates its logical demand one past the cap; the encoder reads that
/// saturated demand rather than the retained row count, so the image is refused here
/// instead of carrying an aliased operand.
#[test]
fn a_draft_whose_demand_crosses_the_cap_cannot_be_encoded() {
    let (mut draft, root, members) = wide_draft(MAX_SITES);
    demand_every_leaf(&mut draft, &root, &members);
    assert!(
        !matches!(draft.encode(), Err(ImageBuildError::TooManySites)),
        "a demand of exactly MAX_SITES fits",
    );

    // A second occurrence over the same declaration: every one of its leaf demands is a
    // fresh `(occurrence, node, target)` key, so these are the demands past the cap.
    let excess = admit_root(&mut draft, 0x22);
    for member in members.iter().take(64) {
        let _ = site(
            &mut draft,
            excess.occurrence(),
            member.path(),
            SemanticTarget::FieldLeaf,
        );
    }

    assert!(matches!(draft.encode(), Err(ImageBuildError::TooManySites)));
}

/// A demand the plan already retains keeps its operand after the cap is crossed: the
/// crossing is nonblocking, and repeating an earlier reference must not start failing or
/// answer with the refusal.
#[test]
fn a_retained_demand_still_reuses_its_operand_after_the_cap_is_crossed() {
    let (mut draft, root, members) = wide_draft(MAX_SITES);
    let first = site(
        &mut draft,
        root.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    demand_every_leaf(&mut draft, &root, &members);

    let over = admit_root(&mut draft, 0x22);
    let refused = site(
        &mut draft,
        over.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    let again = site(
        &mut draft,
        root.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );

    assert_eq!(
        first, again,
        "a repeated reference to one demand returns the operand it was already given",
    );
    assert_ne!(
        first, refused,
        "an excess demand is refused rather than aliased onto a retained one",
    );
}

const FIELD_ID: [u8; 16] = [0x31; 16];
const GROUP_ID: [u8; 16] = [0x32; 16];
const BRANCH_ID: [u8; 16] = [0x33; 16];
const BRANCH_KEY_ID: [u8; 16] = [0x34; 16];
const SCAN_INDEX_ID: [u8; 16] = [0x35; 16];
const LOOKUP_INDEX_ID: [u8; 16] = [0x36; 16];
const COMPONENT_ID: [u8; 16] = [0x37; 16];

/// A draft whose one root reaches every admitted operation target exactly once: its own
/// placement (`WholePayload`), a field (`FieldLeaf`), a group (`GroupEntry`), a branch
/// (`WholePayload`), a nonunique index (`IndexScan`), and a unique index
/// (`IndexLookup`).
fn every_target_draft() -> (ImageDraft, AdmittedRoot, Vec<DeclarationMember>) {
    let mut draft = ImageDraft::new();
    let type_name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields: Vec::new(),
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let branch_name = draft.intern_string("b");
    draft
        .declare_product(
            product(),
            record,
            vec![
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: LedgerIdBytes::from_bytes(FIELD_ID),
                        required: true,
                        value: DurableValueShape::Scalar(Scalar::Int),
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Group {
                        id: LedgerIdBytes::from_bytes(GROUP_ID),
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Branch {
                        placement: LedgerIdBytes::from_bytes(BRANCH_ID),
                        name: branch_name,
                        record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Int,
                            id: LedgerIdBytes::from_bytes(BRANCH_KEY_ID),
                        }],
                    },
                },
            ],
        )
        .expect("a well-formed declaration");
    let root_name = draft.intern_string("r");
    let root = draft
        .add_root_occurrence(
            product(),
            RootOccurrenceDef {
                name: root_name,
                keys: Vec::new(),
                placement: LedgerIdBytes::from_bytes([0x21; 16]),
                indexes: vec![
                    DurableIndexShape {
                        id: LedgerIdBytes::from_bytes(SCAN_INDEX_ID),
                        unique: false,
                        components: vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
                            COMPONENT_ID,
                        ))],
                    },
                    DurableIndexShape {
                        id: LedgerIdBytes::from_bytes(LOOKUP_INDEX_ID),
                        unique: true,
                        components: vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
                            COMPONENT_ID,
                        ))],
                    },
                ],
            },
        )
        .expect("the Product is declared");
    let members = draft.product_members(product()).expect("declared");
    (draft, root, members)
}

/// The six distinct places that draft names, each paired with the one target its node
/// admits.
fn every_place(
    root: &AdmittedRoot,
    members: &[DeclarationMember],
) -> [(CanonicalDeclarationPathSelector, SemanticTarget); PLACE_COUNT] {
    [
        (root.placement_path().clone(), SemanticTarget::WholePayload),
        (members[0].path().clone(), SemanticTarget::FieldLeaf),
        (members[1].path().clone(), SemanticTarget::GroupEntry),
        (members[2].path().clone(), SemanticTarget::WholePayload),
        (root.index_paths()[0].clone(), SemanticTarget::IndexScan),
        (root.index_paths()[1].clone(), SemanticTarget::IndexLookup),
    ]
}

/// One place per admitted operation target, plus the branch's own whole-payload place.
const PLACE_COUNT: usize = 6;

/// Red R18: an eagerly minted row followed by a demand-lazy request on the same
/// `(occurrence, node, target)` is **one** row, and the second request reuses the first
/// row's id — for every operation target a place can admit.
///
/// The eager per-node sites a durable graph emits when it is built and the field-leaf
/// sites the lowerer allocates on first reference were two mint paths: the eager one
/// appended a row the demand map never saw, so a later request on the same demand
/// appended a second row for one node, and the two rows were separate operands for one
/// place. Both now enter the one plan, which answers a retained demand with the id it
/// already minted.
///
/// The two paths are disjoint on every production graph — an eager demand's node is a
/// placement, group, or index node and its target is never `FieldLeaf` — so unifying them
/// re-mints nothing and no image byte moves. This pins the behavior that makes that safe
/// rather than the disjointness, which a future graph shape could narrow.
#[test]
fn one_demand_minted_eagerly_then_lazily_is_one_row_with_one_id() {
    for index in 0..PLACE_COUNT {
        let (mut draft, root, members) = every_target_draft();
        let places = every_place(&root, &members);
        let (path, target) = &places[index];
        let eager = site(&mut draft, root.occurrence(), path, *target);
        let lazy = site(&mut draft, root.occurrence(), path, *target);

        assert_eq!(
            eager, lazy,
            "a repeated demand reuses the row already minted for it",
        );

        let (other_path, other_target) = &places[(index + 1) % PLACE_COUNT];
        assert_ne!(
            site(&mut draft, root.occurrence(), other_path, *other_target),
            eager,
            "the repeated demand appended no second row, so a fresh demand is a fresh row",
        );
    }
}

/// Every operation target a graph admits is reached by a distinct node, and each is its
/// own site row: no two of the six places one root names collapse onto a single operand.
#[test]
fn each_admitted_target_is_its_own_row() {
    let (mut draft, root, members) = every_target_draft();
    let places = every_place(&root, &members);

    let operands: Vec<LegacyDraftSiteOperand> = places
        .iter()
        .map(|(path, target)| site(&mut draft, root.occurrence(), path, *target))
        .collect();

    for (left, first) in operands.iter().enumerate() {
        for second in &operands[left + 1..] {
            assert_ne!(
                first, second,
                "two distinct places are two site rows, never one shared operand",
            );
        }
    }
}

/// One declaration path bound under two root occurrences is two demands, not one: the
/// occurrence is part of the key, so two roots over one Product declaration never
/// collapse into a single row.
///
/// A node admits exactly one operation target under the checked seam, so the occurrence —
/// not the target — is now the way two demands can share one declaration path, and it is
/// the remaining way two distinct durable nodes could be aliased onto one site operand.
#[test]
fn one_declaration_path_under_two_occurrences_is_two_rows() {
    let (mut draft, first_root, members) = wide_draft(2);
    let second_root = admit_root(&mut draft, 0x22);

    let first = site(
        &mut draft,
        first_root.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    let second = site(
        &mut draft,
        second_root.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );

    assert_ne!(
        first, second,
        "one declaration path under two occurrences is two site rows",
    );
}

/// A fitting operand renders as the logical site number it carries, so an instruction's
/// `Debug` reads exactly as it did when the operand was a bare `u16`.
#[test]
fn a_fitting_operand_renders_its_logical_site_number() {
    let (mut draft, root, members) = wide_draft(2);

    let zero = site(
        &mut draft,
        root.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    let one = site(
        &mut draft,
        root.occurrence(),
        members[1].path(),
        SemanticTarget::FieldLeaf,
    );

    assert_eq!(format!("{zero:?}"), "0");
    assert_eq!(format!("{one:?}"), "1");
}

/// An over-policy operand renders one fixed marker carrying no number, and every refusal
/// renders identically: there is no id to show, and inventing one — or letting the
/// rendering distinguish which demand was refused — would publish exactly the aliasing
/// the operand type exists to prevent.
#[test]
fn every_over_policy_operand_renders_one_fixed_redacted_marker() {
    let (mut draft, root, members) = wide_draft(MAX_SITES);
    demand_every_leaf(&mut draft, &root, &members);

    let over = admit_root(&mut draft, 0x22);
    let first = site(
        &mut draft,
        over.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    let second = site(
        &mut draft,
        over.occurrence(),
        members[1].path(),
        SemanticTarget::FieldLeaf,
    );

    let rendered = format!("{first:?}");
    assert_eq!(rendered, "over-policy");
    assert_eq!(
        rendered,
        format!("{second:?}"),
        "two refused demands are indistinguishable in a log",
    );
    assert!(
        !rendered.chars().any(|character| character.is_ascii_digit()),
        "a refusal carries no site number to mistake for an id",
    );
}

/// Equality is over the logical site ordinal, not over which plan minted it: one ordinal
/// from two independently built drafts compares equal.
///
/// Which draft answered is provenance the operand keeps privately — it is what
/// `add_function` spends — and it deliberately takes no part in equality, so a test that
/// compares an expected ordinal against a freshly built draft's operand still reads as an
/// ordinal comparison.
#[test]
fn one_ordinal_from_two_independent_drafts_compares_equal() {
    let (mut left, left_root, left_members) = wide_draft(1);
    let left_site = site(
        &mut left,
        left_root.occurrence(),
        left_members[0].path(),
        SemanticTarget::FieldLeaf,
    );

    // An independently built draft: its own tables, its own row stamps, and a placement
    // and member set that share nothing with the first.
    let mut right = ImageDraft::new();
    let type_name = right.intern_string("S");
    let record = right.add_record_type(RecordTypeDef {
        name: type_name,
        fields: Vec::new(),
    });
    right.set_application_identity(LedgerIdBytes::from_bytes([0x0f; 16]));
    right
        .declare_product(
            LedgerIdBytes::from_bytes([0x1d; 16]),
            record,
            vec![DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Field {
                    id: LedgerIdBytes::from_bytes([0x1e; 16]),
                    required: false,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
            }],
        )
        .expect("a well-formed declaration");
    let root_name = right.intern_string("s");
    let right_root = right
        .add_root_occurrence(
            LedgerIdBytes::from_bytes([0x1d; 16]),
            RootOccurrenceDef {
                name: root_name,
                keys: Vec::new(),
                placement: LedgerIdBytes::from_bytes([0x1b; 16]),
                indexes: Vec::new(),
            },
        )
        .expect("the Product is declared");
    let right_members = right
        .product_members(LedgerIdBytes::from_bytes([0x1d; 16]))
        .expect("declared");
    let right_site = site(
        &mut right,
        right_root.occurrence(),
        right_members[0].path(),
        SemanticTarget::FieldLeaf,
    );

    assert_eq!(
        left_site, right_site,
        "equality is over the logical site ordinal, not over which plan minted it",
    );
}

/// A template proof that crossed the site cap does not leave the real draft crossed.
///
/// The guard's checkpoint records the plan's policy state, so a receipt first recorded
/// inside the discarded pass is cleared with the demands that caused it. Keeping it would
/// let a throwaway proof refuse every later site of the real program and make an image that
/// fits report `TooManySites` — a crossing the finished draft never had.
#[test]
fn a_crossing_inside_a_discarded_proof_does_not_survive_it() {
    let (mut draft, root, members) = wide_draft(MAX_SITES);
    {
        let mut guard = draft.template_proof();
        let proof = guard.proof_draft();
        demand_every_leaf(proof, &root, &members);
        let excess = admit_root(proof, 0x22);
        let over = site(
            proof,
            excess.occurrence(),
            members[0].path(),
            SemanticTarget::FieldLeaf,
        );
        assert_eq!(
            format!("{over:?}"),
            "over-policy",
            "the proof itself crossed the cap"
        );
        assert!(matches!(proof.encode(), Err(ImageBuildError::TooManySites)));
    }
    assert!(
        !matches!(draft.encode(), Err(ImageBuildError::TooManySites)),
        "the discarded proof's crossing is not the finished draft's",
    );
}

/// A crossing recorded **before** the proof is not undone by discarding the proof: it
/// happened, and the operand that stands on it stays the operand it was.
#[test]
fn a_crossing_before_a_proof_survives_the_proof() {
    let (mut draft, root, members) = wide_draft(MAX_SITES);
    demand_every_leaf(&mut draft, &root, &members);
    let excess = admit_root(&mut draft, 0x22);
    let _ = site(
        &mut draft,
        excess.occurrence(),
        members[0].path(),
        SemanticTarget::FieldLeaf,
    );
    assert!(matches!(draft.encode(), Err(ImageBuildError::TooManySites)));
    {
        let mut guard = draft.template_proof();
        let _ = guard.proof_draft().intern_string("throwaway");
    }
    assert!(
        matches!(draft.encode(), Err(ImageBuildError::TooManySites)),
        "the crossing predates the proof and is not rolled back with it",
    );
}

/// Declare the already-declared Product identity a second time with a divergent member
/// graph — one field where the draft holds several. Two declarations wearing one identity,
/// which the draft records as a sticky conflict and the encoder refuses.
fn redeclare_divergent(draft: &mut ImageDraft) {
    let type_name = draft.intern_string("D");
    let record = draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields: Vec::new(),
    });
    draft
        .declare_product(
            product(),
            record,
            vec![DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Field {
                    id: field_id(DIVERGENT_FIELD),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Int),
                },
            }],
        )
        .expect("a well-formed declaration");
}

/// A divergent Product claim recorded inside a discarded proof does not survive it.
///
/// The conflict is sticky and gates encoding, while the declaration table that admits it is
/// truncated with the rest of the pass. Keeping the conflict without keeping the row that
/// caused it would leave the real draft refusing an image whose declarations never
/// disagreed, and falsify the guard's byte-identity contract.
#[test]
fn a_product_conflict_inside_a_discarded_proof_does_not_survive_it() {
    let (mut draft, root, members) = wide_draft(4);
    demand_every_leaf(&mut draft, &root, &members);
    let before = draft.encode().expect("a fitting draft").bytes;
    {
        let mut guard = draft.template_proof();
        let proof = guard.proof_draft();
        redeclare_divergent(proof);
        assert!(matches!(
            proof.encode(),
            Err(ImageBuildError::ProductGraphConflict)
        ));
    }
    let after = draft
        .encode()
        .expect("the discarded proof's conflict is not the finished draft's")
        .bytes;
    assert_eq!(before, after, "the proof appended nothing that survived it");
}

/// A divergent Product claim recorded **before** the proof is not undone by discarding the
/// proof: the two disagreeing declarations are both still in the table, so the image stays
/// refused.
#[test]
fn a_product_conflict_before_a_proof_survives_the_proof() {
    let (mut draft, root, members) = wide_draft(4);
    demand_every_leaf(&mut draft, &root, &members);
    redeclare_divergent(&mut draft);
    assert!(matches!(
        draft.encode(),
        Err(ImageBuildError::ProductGraphConflict)
    ));
    {
        let mut guard = draft.template_proof();
        let _ = guard.proof_draft().intern_string("throwaway");
    }
    assert!(
        matches!(draft.encode(), Err(ImageBuildError::ProductGraphConflict)),
        "the conflict predates the proof and is not rolled back with it",
    );
}

/// Dropping the guard restores every draft owner the proof appended to, so the finished
/// draft encodes to the exact bytes it would have without the pass.
#[test]
fn a_discarded_proof_leaves_the_draft_byte_identical() {
    let (mut draft, root, members) = wide_draft(4);
    demand_every_leaf(&mut draft, &root, &members);
    let before = draft.encode().expect("a fitting draft").bytes;
    {
        let mut guard = draft.template_proof();
        let proof = guard.proof_draft();
        let _ = proof.intern_string("throwaway");
        let extra = admit_root(proof, 0x33);
        demand_every_leaf(proof, &extra, &members);
    }
    let after = draft.encode().expect("a fitting draft").bytes;
    assert_eq!(before, after, "the proof appended nothing that survived it");
}

// --- The site-plan policy corpora (custody design 2.16, corpora 2-4) ---

/// Corpus 2: 1,024 keyed roots each touching 64 fields of one shared Product — 66,560
/// logical site demands over an identity census of only 2,114 ledger rows.
///
/// The demand set is eight times the site table's capacity while the *declaration* it
/// addresses is tiny, which is the shape that made the old length-narrowing mint dangerous:
/// the demand count, not the declared graph, is what crosses. The plan retains exactly
/// `MAX_SITES` rows with ids exactly `0..MAX_SITES`, records exactly one receipt at the
/// earliest crossing, and still answers a demand it already retains with the id it gave.
#[test]
fn one_thousand_roots_touching_sixty_four_fields_saturate_exactly_once() {
    const ROOTS: usize = 1_024;
    const FIELDS: usize = 64;
    // The identity census this corpus needs: the application, the Product, one root
    // placement and one key column per root, and one field per declared member.
    assert_eq!(1 + 1 + ROOTS + ROOTS + FIELDS, 2_114);
    assert_eq!(ROOTS * FIELDS + ROOTS, 66_560, "the logical demand count");

    let mut draft = ImageDraft::new();
    declare_wide_product(&mut draft, FIELDS);
    let members = draft.product_members(product()).expect("declared");

    let mut first_root: Option<AdmittedRoot> = None;
    let mut first_operand: Option<LegacyDraftSiteOperand> = None;
    let mut fitting = 0usize;
    let mut over_policy = 0usize;
    let mut answer = |operand: &LegacyDraftSiteOperand| {
        if format!("{operand:?}") == "over-policy" {
            over_policy += 1;
        } else {
            fitting += 1;
        }
    };
    for n in 0..ROOTS {
        let name = draft.intern_string(&format!("r{n}"));
        let admitted = draft
            .add_root_occurrence(
                product(),
                RootOccurrenceDef {
                    name,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: field_id(0x10_0000 + n),
                    }],
                    placement: field_id(0x20_0000 + n),
                    indexes: Vec::new(),
                },
            )
            .expect("the Product is declared");
        let root_site = site(
            &mut draft,
            admitted.occurrence(),
            admitted.placement_path(),
            SemanticTarget::WholePayload,
        );
        answer(&root_site);
        for member in &members {
            let leaf = site(
                &mut draft,
                admitted.occurrence(),
                member.path(),
                SemanticTarget::FieldLeaf,
            );
            answer(&leaf);
        }
        if first_root.is_none() {
            first_root = Some(admitted);
            first_operand = Some(root_site);
        }
    }

    assert_eq!(fitting, MAX_SITES, "the plan retains exactly its capacity");
    assert_eq!(
        fitting + over_policy,
        ROOTS * FIELDS + ROOTS,
        "every demand was answered, fitting or refused",
    );

    // Re-requesting the very first demand still answers with its own id, however far past
    // the cap the plan is: the crossing is nonblocking, and a repeated reference must not
    // begin to fail or be handed the refusal.
    let first = first_root.expect("root 0");
    let repeat = site(
        &mut draft,
        first.occurrence(),
        first.placement_path(),
        SemanticTarget::WholePayload,
    );
    assert_eq!(
        repeat,
        first_operand.expect("recorded at root 0"),
        "a retained demand reuses its id after the crossing",
    );
    assert!(matches!(draft.encode(), Err(ImageBuildError::TooManySites)));
}

/// Corpus 3: 4,000 roots over one shared Product of 100 static groups, each carrying one
/// scalar field, with no group ever operated on.
///
/// This is the corpus the eager-per-occurrence policy could not represent: pre-seeding each
/// occurrence's whole member graph demands `4,000 x (1 root + 100 groups)` = 404,000 sites
/// against a table that holds 8,192. A repeated Product now pre-seeds only each occurrence's
/// root whole-payload site, so the same graph costs 4,000 rows and the image encodes — and
/// its identity census stays at 4,202 ledger rows, well inside `MAX_IDS_ROWS`.
#[test]
fn four_thousand_roots_over_a_hundred_unoperated_groups_cost_one_site_each() {
    const ROOTS: usize = 4_000;
    const GROUPS: usize = 100;
    const LEGACY_DEMAND: usize = ROOTS * (1 + GROUPS);
    const _: () = assert!(
        LEGACY_DEMAND > MAX_SITES && ROOTS <= MAX_SITES,
        "the legacy demand is unrepresentable and the occurrence-only demand fits",
    );
    assert_eq!(
        LEGACY_DEMAND, 404_000,
        "what pre-seeding every occurrence's member graph would demand",
    );
    // The identity census: the application, the Product, one placement per keyless root,
    // and one row per declared group namespace and group field.
    assert_eq!(1 + 1 + ROOTS + 2 * GROUPS, 4_202);

    let mut draft = ImageDraft::new();
    let type_name = draft.intern_string("R");
    let record = draft.add_record_type(RecordTypeDef {
        name: type_name,
        fields: Vec::new(),
    });
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let mut commands = Vec::with_capacity(2 * GROUPS);
    for group in 0..GROUPS {
        let parent = u16::try_from(commands.len()).expect("inside the member bound");
        commands.push(DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Group {
                id: field_id(0x30_0000 + group),
            },
        });
        commands.push(DeclarationMemberDef {
            parent: Some(parent),
            shape: DeclarationMemberShape::Field {
                id: field_id(0x40_0000 + group),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            },
        });
    }
    draft
        .declare_product(product(), record, commands)
        .expect("a well-formed declaration");

    for n in 0..ROOTS {
        let name = draft.intern_string(&format!("r{n}"));
        let admitted = draft
            .add_root_occurrence(
                product(),
                RootOccurrenceDef {
                    name,
                    keys: Vec::new(),
                    placement: field_id(0x50_0000 + n),
                    indexes: Vec::new(),
                },
            )
            .expect("the Product is declared");
        site(
            &mut draft,
            admitted.occurrence(),
            admitted.placement_path(),
            SemanticTarget::WholePayload,
        );
    }

    assert!(
        !matches!(draft.encode(), Err(ImageBuildError::TooManySites)),
        "4,000 occurrence sites over a 200-row declaration is inside the site table",
    );
}
