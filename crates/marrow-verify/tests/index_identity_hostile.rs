//! Forged-image hostiles for the entry-identity instructions and the managed-index
//! reads (ID01). Each artifact carries a valid encoder-computed digest and violates
//! exactly one phase-3 stack-effect invariant, so it must reject at the verifier stage
//! that owns the invariant rather than at the digest gate. These make the verifier's
//! identity/index rejections conspicuous enforcement artifacts, not merely reachable by
//! inspection.

use marrow_image::{
    DurableIndexComponent, DurableIndexShape, DurableMemberDef, DurableValueShape, ExportId,
    FieldDef, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef,
    RootDef, RootIdentity, Scalar, SemanticPath, SemanticStep, SemanticStepKind, SiteDef, SpanEntry,
};
use marrow_verify::verify;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const KEY_ID: [u8; 16] = [0x0c; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
const TITLE_ID: [u8; 16] = [0x0e; 16];
const SHELF_ID: [u8; 16] = [0x1e; 16];
const ISBN_ID: [u8; 16] = [0x2e; 16];
const BY_SHELF_ID: [u8; 16] = [0x3b; 16];
const BY_ISBN_ID: [u8; 16] = [0x4b; 16];

fn root_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(PLACEMENT_ID),
        ),
    ])
}

fn index_path(index_id: [u8; 16]) -> SemanticPath {
    let mut steps = root_path().steps().to_vec();
    steps.push(SemanticStep::new(
        SemanticStepKind::Index,
        LedgerIdBytes::from_bytes(index_id),
    ));
    SemanticPath::from_steps(steps)
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

/// Site indices captured while building the shared `^r[k:int]: Rec` graph with a
/// nonunique `byShelf[shelf, k]` and a unique `byIsbn[isbn]`.
struct Graph {
    scan_site: u16,
    lookup_site: u16,
    list_int: u16,
}

fn build_graph(draft: &mut ImageDraft) -> Graph {
    let rec = draft.intern_string("Rec");
    let title = draft.intern_string("title");
    let shelf = draft.intern_string("shelf");
    let isbn = draft.intern_string("isbn");
    let record = draft.add_record_type(RecordTypeDef {
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
    });
    let root = draft.intern_string("r");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft.add_root(RootDef {
        name: root,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes(KEY_ID),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes(PRODUCT_ID),
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
            ],
            members: vec![
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes(TITLE_ID),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes(SHELF_ID),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
                DurableMemberDef::Field {
                    id: LedgerIdBytes::from_bytes(ISBN_ID),
                    required: true,
                    value: DurableValueShape::Scalar(Scalar::Text),
                },
            ],
        },
    });
    // Site 0 is the root whole-payload entry; then the two index read sites.
    draft.add_site(SiteDef::whole_payload(root_path()));
    let scan_site = draft
        .add_site(SiteDef::index_scan(index_path(BY_SHELF_ID)))
        .index();
    let lookup_site = draft
        .add_site(SiteDef::index_lookup(index_path(BY_ISBN_ID)))
        .index();
    let list_int = draft
        .add_collection_type(marrow_image::CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        })
        .index();
    Graph {
        scan_site,
        lookup_site,
        list_int,
    }
}

/// Encode a single read-only export of `code` with `params` and `ret` over the shared
/// graph, then verify. `Err(())` when the image is refused.
fn verify_one(code: Vec<Instr>, params: Vec<ImageType>, ret: ImageType) -> Result<(), ()> {
    let mut draft = ImageDraft::new();
    build_graph(&mut draft);
    build_export(&mut draft, code, params, ret);
    verify(&draft.encode().expect("encode").bytes)
        .map(|_| ())
        .map_err(|_| ())
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

fn text() -> ImageType {
    ImageType::scalar(Scalar::Text)
}
fn int() -> ImageType {
    ImageType::scalar(Scalar::Int)
}
fn opt_id() -> ImageType {
    ImageType::Identity {
        root: 0,
        optional: true,
    }
}
fn list_ret(idx: u16) -> ImageType {
    ImageType::Collection {
        idx,
        optional: false,
    }
}

// --- Well-formed baselines: the valid images the forgeries perturb must verify. ---

#[test]
fn a_valid_index_scan_and_lookup_verify() {
    // Scan holds the one leading field (shelf) as a prefix and freezes `List[int]`.
    let mut draft = ImageDraft::new();
    let g = build_graph(&mut draft);
    let scan = vec![
        Instr::LocalGet(0),
        Instr::DurIndexScan {
            site: g.scan_site,
            limit: 5,
            from: false,
            list_ty: g.list_int,
        },
        Instr::Pop, // drop the on-more Bool; return the list
        Instr::Return,
    ];
    build_export(&mut draft, scan, vec![text()], list_ret(g.list_int));
    assert!(verify(&draft.encode().expect("encode").bytes).is_ok());

    let mut draft = ImageDraft::new();
    let g = build_graph(&mut draft);
    let lookup = vec![
        Instr::LocalGet(0),
        Instr::DurIndexLookup(g.lookup_site),
        Instr::Return,
    ];
    build_export(&mut draft, lookup, vec![text()], opt_id());
    assert!(verify(&draft.encode().expect("encode").bytes).is_ok());
}

// --- Index read-kind forgeries. ---

#[test]
fn a_scan_over_a_unique_index_is_refused() {
    let mut draft = ImageDraft::new();
    let g = build_graph(&mut draft);
    // `DurIndexScan` pointed at the unique lookup site.
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurIndexScan {
            site: g.lookup_site,
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
    let mut draft = ImageDraft::new();
    let g = build_graph(&mut draft);
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurIndexLookup(g.scan_site),
        Instr::Return,
    ];
    build_export(&mut draft, code, vec![text()], opt_id());
    assert!(verify(&draft.encode().expect("encode").bytes).is_err());
}

#[test]
fn a_scan_list_of_the_wrong_element_type_is_refused() {
    let mut draft = ImageDraft::new();
    let g = build_graph(&mut draft);
    let list_text = draft
        .add_collection_type(marrow_image::CollectionTypeDef::List { elem: text() })
        .index();
    // The scanned identity key is `int`, so a `List[string]` frozen type is refused.
    let code = vec![
        Instr::LocalGet(0),
        Instr::DurIndexScan {
            site: g.scan_site,
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

#[test]
fn a_make_identity_over_an_out_of_range_root_is_refused() {
    // `MakeIdentity` naming root index 9 — the image has one root.
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity { root: 9, cols: 1 },
        Instr::DurIndexLookup(0), // unreachable; the forged MakeIdentity rejects first
        Instr::Return,
    ];
    assert!(verify_one(code, vec![int()], opt_id()).is_err());
}

#[test]
fn a_make_identity_with_the_wrong_column_count_is_refused() {
    // The root has one key column; `cols: 2` disagrees.
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity { root: 0, cols: 2 },
        Instr::Return,
    ];
    assert!(
        verify_one(
            code,
            vec![int()],
            ImageType::Identity {
                root: 0,
                optional: false,
            },
        )
        .is_err()
    );
}

#[test]
fn an_identity_key_path_with_the_wrong_column_count_is_refused() {
    // Build a bare identity, then spread it claiming two columns for a one-key root.
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity { root: 0, cols: 1 },
        Instr::IdentityKeyPath(2),
        Instr::Return,
    ];
    assert!(verify_one(code, vec![int()], ImageType::Unit).is_err());
}

#[test]
fn a_valid_identity_round_trip_verifies() {
    // The baseline the forgeries perturb: build an identity and spread it back to its
    // one key column, leaving an int the function returns.
    let code = vec![
        Instr::LocalGet(0),
        Instr::MakeIdentity { root: 0, cols: 1 },
        Instr::IdentityKeyPath(1),
        Instr::Return,
    ];
    assert!(verify_one(code, vec![int()], int()).is_ok());
}
