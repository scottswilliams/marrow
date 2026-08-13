//! Forged-image hostiles for the entry-identity instructions and the managed-index
//! reads. Each artifact carries a valid encoder-computed digest and violates
//! exactly one phase-3 stack-effect invariant, so it must reject at the verifier stage
//! that owns the invariant rather than at the digest gate. These make the verifier's
//! identity/index rejections conspicuous enforcement artifacts, not merely reachable by
//! inspection.
//!
//! Every site named here is minted through the construction seam's bind-then-request
//! protocol. That protocol has exactly one owner in the workspace and is included here
//! rather than copied.

use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, DraftTxn, DurableIndexComponent,
    DurableIndexShape, ExportId, FieldDef, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn,
    LedgerIdBytes, PlannedSiteRef, RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget,
    SpanEntry, ValueShapeNodeId,
};
use marrow_verify::verify;

#[path = "../../marrow-image/tests/common/site_seam.rs"]
mod site_seam;
use site_seam::site;

#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

#[path = "common/admitted.rs"]
mod admitted_helper;
use admitted_helper::admitted;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const KEY_ID: [u8; 16] = [0x0c; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
const TITLE_ID: [u8; 16] = [0x0e; 16];
const SHELF_ID: [u8; 16] = [0x1e; 16];
const ISBN_ID: [u8; 16] = [0x2e; 16];
const BY_SHELF_ID: [u8; 16] = [0x3b; 16];
const BY_ISBN_ID: [u8; 16] = [0x4b; 16];

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect()
}

/// One required text field of the shared `Rec` graph.
fn text_field(value: ValueShapeNodeId, id: [u8; 16]) -> DeclarationMemberDef {
    DeclarationMemberDef {
        parent: None,
        shape: DeclarationMemberShape::Field {
            id: LedgerIdBytes::from_bytes(id),
            required: true,
            value,
        },
    }
}

/// Site indices captured while building the shared `^r[k:int]: Rec` graph with a
/// nonunique `byShelf[shelf, k]` and a unique `byIsbn[isbn]`.
struct Graph {
    entry_site: PlannedSiteRef,
    scan_site: PlannedSiteRef,
    lookup_site: PlannedSiteRef,
    list_int: marrow_image::CollTypeId,
}

fn build_graph(draft: &mut DraftTxn<'_>) -> Graph {
    let rec = draft.intern_string("Rec").expect("a within-domain mint");
    let title = draft.intern_string("title").expect("a within-domain mint");
    let shelf = draft.intern_string("shelf").expect("a within-domain mint");
    let isbn = draft.intern_string("isbn").expect("a within-domain mint");
    let record = draft
        .add_record_type(RecordTypeDef {
            name: rec,
            fields: vec![
                FieldDef {
                    name: title,
                    ty: ImageType::scalar(Scalar::Text),
                    required: true,
                },
                FieldDef {
                    name: shelf,
                    ty: ImageType::scalar(Scalar::Text),
                    required: true,
                },
                FieldDef {
                    name: isbn,
                    ty: ImageType::scalar(Scalar::Text),
                    required: true,
                },
            ],
        })
        .expect("a within-domain mint");
    let root = draft.intern_string("r").expect("a within-domain mint");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let text = draft
        .value_scalar(Scalar::Text)
        .expect("the test arena mints");
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![
                text_field(text, TITLE_ID),
                text_field(text, SHELF_ID),
                text_field(text, ISBN_ID),
            ],
        )
        .expect("a well-formed declaration");
    // The managed indexes are occurrence facts, in declaration order: the nonunique
    // `byShelf` first, then the unique `byIsbn`.
    let r = draft
        .add_root_occurrence(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            RootOccurrenceDef {
                name: root,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                indexes: vec![
                    DurableIndexShape {
                        id: LedgerIdBytes::from_bytes(BY_SHELF_ID),
                        unique: false,
                        components: vec![
                            DurableIndexComponent::Field(LedgerIdBytes::from_bytes(SHELF_ID)),
                            DurableIndexComponent::Key(LedgerIdBytes::from_bytes(KEY_ID)),
                        ],
                    },
                    DurableIndexShape {
                        id: LedgerIdBytes::from_bytes(BY_ISBN_ID),
                        unique: true,
                        components: vec![DurableIndexComponent::Field(LedgerIdBytes::from_bytes(
                            ISBN_ID,
                        ))],
                    },
                ]
                .into(),
            },
        )
        .expect("the Product is declared");
    // The root whole-payload entry site first, then the two index read sites.
    let scan_path = r.index_paths()[0].clone();
    let lookup_path = r.index_paths()[1].clone();
    let entry_site = site(
        draft,
        r.occurrence(),
        r.placement_path(),
        SemanticTarget::WholePayload,
    );
    let scan_site = site(draft, r.occurrence(), &scan_path, SemanticTarget::IndexScan);
    let lookup_site = site(
        draft,
        r.occurrence(),
        &lookup_path,
        SemanticTarget::IndexLookup,
    );
    let list_int = draft
        .add_collection_type(marrow_image::CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        })
        .expect("a within-domain mint");
    Graph {
        entry_site,
        scan_site,
        lookup_site,
        list_int,
    }
}

/// Encode a single read-only export of `code` with `params` and `ret` over the shared
/// graph, then verify. `Err(())` when the image is refused.
fn verify_one(
    code: impl FnOnce(&Graph) -> Vec<Instr>,
    params: Vec<ImageType>,
    ret: ImageType,
) -> Result<(), ()> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let graph = build_graph(&mut draft);
    let code = code(&graph);
    build_export(&mut draft, code, params, ret);
    verify(&draft.encode().expect("encode").bytes)
        .map(|_| ())
        .map_err(|_| ())
}

fn build_export(
    draft: &mut DraftTxn<'_>,
    code: Vec<Instr>,
    params: Vec<ImageType>,
    ret: ImageType,
) {
    let src = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    let name = draft.intern_string("f").expect("a within-domain mint");
    let local_count = params.len() as u16;
    let func = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params,
            ret,
            local_count,
            spans: spans(&code),
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "f"), func);
}

fn text() -> ImageType {
    ImageType::scalar(Scalar::Text)
}
fn int() -> ImageType {
    ImageType::scalar(Scalar::Int)
}
fn opt_id() -> ImageType {
    ImageType::Identity {
        root: marrow_image::RootId::from_index(0),
        optional: true,
    }
}
fn list_ret(idx: marrow_image::CollTypeId) -> ImageType {
    ImageType::Collection {
        idx,
        optional: false,
    }
}

// --- Well-formed baselines: the valid images the forgeries perturb must verify. ---

#[test]
fn a_valid_index_scan_and_lookup_verify() {
    // Scan holds the one leading field (shelf) as a prefix and freezes `List[int]`.
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let g = build_graph(&mut draft);
    let scan = vec![
        Instr::LocalGet(0),
        Instr::DurIndexScan {
            site: g.scan_site.clone(),
            limit: 5,
            from: false,
            list_ty: g.list_int,
        },
        Instr::Pop, // drop the on-more Bool; return the list
        Instr::Return,
    ];
    build_export(&mut draft, scan, vec![text()], list_ret(g.list_int));
    assert!(verify(&draft.encode().expect("encode").bytes).is_ok());

    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let g = build_graph(&mut draft);
    let lookup = vec![
        Instr::LocalGet(0),
        Instr::DurIndexLookup(g.lookup_site.clone()),
        Instr::Return,
    ];
    build_export(&mut draft, lookup, vec![text()], opt_id());
    assert!(verify(&draft.encode().expect("encode").bytes).is_ok());
}

// --- Index read-kind forgeries. ---

#[test]
fn a_scan_over_a_unique_index_is_refused() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let g = build_graph(&mut draft);
    // `DurIndexScan` pointed at the unique lookup site.
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurIndexScan {
            site: g.lookup_site.clone(),
            limit: 5,
            from: false,
            list_ty: g.list_int,
        },
        Instr::Pop,
        Instr::Return,
    ];
    build_export(&mut draft, code, vec![text()], list_ret(g.list_int));
    assert!(verify(&draft.encode().expect("encode").bytes).is_err());
}

#[test]
fn a_lookup_over_a_nonunique_index_is_refused() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let g = build_graph(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurIndexLookup(g.scan_site.clone()),
        Instr::Return,
    ];
    build_export(&mut draft, code, vec![text()], opt_id());
    assert!(verify(&draft.encode().expect("encode").bytes).is_err());
}

#[test]
fn a_scan_list_of_the_wrong_element_type_is_refused() {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let g = build_graph(&mut draft);
    let list_text = draft
        .add_collection_type(marrow_image::CollectionTypeDef::List { elem: text() })
        .expect("a within-domain mint");
    // The scanned identity key is `int`, so a `List[string]` frozen type is refused.
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurIndexScan {
            site: g.scan_site.clone(),
            limit: 5,
            from: false,
            list_ty: list_text,
        },
        Instr::Pop,
        Instr::Return,
    ];
    build_export(&mut draft, code, vec![text()], list_ret(list_text));
    assert!(verify(&draft.encode().expect("encode").bytes).is_err());
}

// --- Entry-identity instruction forgeries. ---

// An out-of-range `MakeIdentity` root and a `cols` count disagreeing with the root's
// key arity are refused by the producer since the coherence hoist; their pins live in
// `legacy_ok_pins.rs`, so no duplicate probes are kept here.

#[test]
fn an_identity_key_path_with_the_wrong_column_count_is_refused() {
    // Build a bare identity, then spread it claiming two columns for a one-key root.
    let code = |_: &Graph| {
        vec![
            Instr::LocalGet(0),
            Instr::MakeIdentity {
                root: marrow_image::RootId::from_index(0),
                cols: 1,
            },
            Instr::IdentityKeyPath(2),
            Instr::Return,
        ]
    };
    assert!(verify_one(code, vec![int()], ImageType::Unit).is_err());
}

#[test]
fn a_valid_identity_round_trip_verifies() {
    // The baseline the forgeries perturb: build an identity of the root and spread it
    // back to its one key column, which keys a durable operation on that same root — the
    // spread column's only legitimate consumer. (A spread key column is a distinct typed
    // operand, not a plain int, so it flows to a durable key-path rather than a return.)
    let code = |g: &Graph| {
        vec![
            Instr::LocalGet(0),
            Instr::MakeIdentity {
                root: marrow_image::RootId::from_index(0),
                cols: 1,
            },
            Instr::IdentityKeyPath(1),
            Instr::DurExists(g.entry_site.clone()),
            Instr::Return,
        ]
    };
    assert!(verify_one(code, vec![int()], ImageType::scalar(Scalar::Bool)).is_ok());
}
