//! D03 slice 1: narrow managed indexes — declaration, admission, identity, and
//! image growth, observed through the full production path capture -> compile ->
//! verify.
//!
//! A `store` root may declare narrow compiler-maintained ordered indexes: a
//! nonunique index ending with the complete identity suffix, or a `unique` index
//! that may omit it. Each projects only the root's identity keys and top-level
//! orderable-key fields. Each index carries its own `Index` ledger identity, and the
//! verifier independently reconstructs the index set and derives the field/root
//! incidence — the maintenance consequence an exact write keeps coherent. Index reads
//! execute (a bounded nonunique scan and a unique lookup); their runtime behavior is
//! exercised in the VM `index_read` fixtures.

use marrow_verify::{
    DurableIndexComponent, LedgerIdBytes, SealedIndexComponent, SealedSite, SealedSiteTarget,
    SemanticNodeKind, SemanticStepKind, SemanticTarget,
};

fn rep(byte: u8) -> LedgerIdBytes {
    LedgerIdBytes::from_bytes([byte; 16])
}

/// A `Book` resource with an indexed `shelf` and a unique `isbn`, over a keyed root
/// with two managed indexes: nonunique `byShelf(shelf, id)` and unique `byIsbn(isbn)`.
const INDEXED_SOURCE: &str = r#"resource Book {
    required title: string
    shelf: string
    isbn: string
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
    index byIsbn[isbn] unique
}

pub fn label(): string {
    return "books"
}
"#;

/// A complete ledger for the indexed graph, including both `Index` anchors.
const INDEXED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.shelf 10101010101010101010101010101010\n\
     id field Book.isbn 11111111111111111111111111111111\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index books.byShelf 70707070707070707070707070707070\n\
     id index books.byIsbn 71717171717171717171717171717171\n\
     high-water 0\n\
     end\n";

/// A nominal field remains a distinct source type but erases to `int` in the
/// durable stored shape. The same erased scalar must therefore admit and type the
/// managed-index projection without introducing nominal identity into the image.
const NOMINAL_INDEX_SOURCE: &str = r#"type Rank: int in 0..=100

resource Book {
    required title: string
    rank: Rank
}

store ^books[id: int]: Book {
    index byRank[rank, id]
}

pub fn label(): string {
    return "books"
}
"#;

/// The complete ledger for [`NOMINAL_INDEX_SOURCE`]. The nominal itself mints no
/// durable anchor; its field, root, key, and managed index retain their ordinary
/// identities.
const NOMINAL_INDEX_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.rank 10101010101010101010101010101010\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index books.byRank 70707070707070707070707070707070\n\
     high-water 0\n\
     end\n";

fn verify_source(source: &str, ids: &str) -> Result<marrow_verify::VerifiedImage, String> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = match marrow_compile::compile(&project) {
        Ok(compiled) => compiled,
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => {
            return Err(diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>()
                .join(","));
        }
        Err(
            marrow_compile::CompileFailure::Invariant(_)
            | marrow_compile::CompileFailure::ResourceLimit(_),
        ) => {
            return Err("compiler invariant failure".to_string());
        }
    };
    marrow_verify::verify(&compiled.image.bytes).map_err(|r| format!("verify: {r:?}"))
}

/// The `check.*` codes a compile reports, in order. Used for admission rejections.
fn compile_codes(source: &str, ids: &str) -> Vec<&'static str> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    match marrow_compile::compile(&project) {
        Ok(_) => Vec::new(),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect(),
        Err(
            marrow_compile::CompileFailure::Invariant(_)
            | marrow_compile::CompileFailure::ResourceLimit(_),
        ) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
}

#[test]
fn a_keyed_root_with_a_nonunique_and_a_unique_index_verifies_with_complete_identity() {
    let image = verify_source(INDEXED_SOURCE, INDEXED_IDS).expect("indexed graph verifies");
    let indexes = image.indexes();
    assert_eq!(indexes.len(), 2, "two managed indexes seal");

    let by_shelf = &indexes[0];
    assert_eq!(by_shelf.id(), rep(0x70));
    assert_eq!(by_shelf.root(), 0);
    assert!(!by_shelf.unique(), "byShelf is nonunique");
    assert_eq!(
        by_shelf.components(),
        &[
            DurableIndexComponent::Field(rep(0x10)), // shelf
            DurableIndexComponent::Key(rep(0x0c)),   // id (complete identity suffix)
        ],
    );

    let by_isbn = &indexes[1];
    assert_eq!(by_isbn.id(), rep(0x71));
    assert!(by_isbn.unique(), "byIsbn is unique");
    assert_eq!(
        by_isbn.components(),
        &[DurableIndexComponent::Field(rep(0x11))], // isbn (identity omitted)
    );
}

#[test]
fn the_verifier_resolves_each_index_projection_to_record_and_key_positions() {
    // The path kernel maintains an index by position, not ledger id. The verifier
    // resolves each ledger-id projection component to a record-field or key-column
    // position against the same decoded root, in projection order. Record `Book` is
    // {title:0, shelf:1, isbn:2}; the key tuple is [id] at column 0.
    let image = verify_source(INDEXED_SOURCE, INDEXED_IDS).expect("verify");
    let indexes = image.indexes();

    // byShelf projects the `shelf` field then the identity key `id`.
    assert_eq!(
        indexes[0].projection(),
        &[SealedIndexComponent::Field(1), SealedIndexComponent::Key(0)],
    );
    // byIsbn projects only the `isbn` field (a unique index omits the identity suffix).
    assert_eq!(indexes[1].projection(), &[SealedIndexComponent::Field(2)]);
}

#[test]
fn a_nominal_field_managed_index_projects_its_erased_integer_shape() {
    let image = verify_source(NOMINAL_INDEX_SOURCE, NOMINAL_INDEX_IDS)
        .expect("a nominal-field managed index compiles and independently verifies");
    let indexes = image.indexes();
    assert_eq!(indexes.len(), 1, "the nominal index seals exactly once");
    assert_eq!(indexes[0].id(), rep(0x70));
    assert_eq!(
        indexes[0].components(),
        &[
            DurableIndexComponent::Field(rep(0x10)),
            DurableIndexComponent::Key(rep(0x0c)),
        ],
    );
    assert_eq!(
        indexes[0].projection(),
        &[SealedIndexComponent::Field(1), SealedIndexComponent::Key(0)],
    );

    let contract = image.durable_contract();
    let repeated = verify_source(NOMINAL_INDEX_SOURCE, NOMINAL_INDEX_IDS)
        .expect("the nominal index verifies repeatedly")
        .durable_contract();
    assert_eq!(contract, repeated, "the durable contract stays stable");
}

#[test]
fn a_nominal_field_root_operation_remains_parked() {
    let source = format!(
        "{NOMINAL_INDEX_SOURCE}\n\
         pub fn present(id: int): bool {{\n\
         \x20   return exists(^books[id])\n\
         }}\n"
    );
    assert_eq!(
        compile_codes(&source, NOMINAL_INDEX_IDS),
        vec!["check.unsupported"],
        "admitting the nominal index declaration does not make its root executable",
    );
}

#[test]
fn the_verifier_derives_field_and_root_incidence() {
    let image = verify_source(INDEXED_SOURCE, INDEXED_IDS).expect("verify");

    // FieldId -> [IndexId]: mutating `shelf` maintains byShelf; `isbn` maintains
    // byIsbn; `title` (unindexed) maintains nothing.
    assert_eq!(image.field_incidence(rep(0x10)), vec![rep(0x70)]);
    assert_eq!(image.field_incidence(rep(0x11)), vec![rep(0x71)]);
    assert!(image.field_incidence(rep(0x0e)).is_empty());

    // An identity-key projection component is not a field-maintenance trigger: the
    // key `id` (0x0c) appears in byShelf's projection but keys are immutable.
    assert!(image.field_incidence(rep(0x0c)).is_empty());

    // RootId -> [IndexId]: a whole-entry write on root 0 maintains both indexes.
    assert_eq!(image.root_incidence(0), vec![rep(0x70), rep(0x71)]);
}

#[test]
fn each_managed_index_is_a_graph_node_with_a_three_step_semantic_path() {
    let image = verify_source(INDEXED_SOURCE, INDEXED_IDS).expect("verify");
    let index_nodes: Vec<_> = image
        .semantic_nodes()
        .into_iter()
        .filter(|node| node.kind == SemanticNodeKind::Index)
        .collect();
    assert_eq!(index_nodes.len(), 2, "one graph node per managed index");
    for (node, index_id) in index_nodes.iter().zip([rep(0x70), rep(0x71)]) {
        let steps = node.path.steps();
        // The index node's path is [Application, Placement, Index]: the root path
        // extended by the index step, ending in the index's own ledger id.
        assert_eq!(
            steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![
                SemanticStepKind::Application,
                SemanticStepKind::Placement,
                SemanticStepKind::Index,
            ],
        );
        assert_eq!(node.path.node_id(), index_id);
    }
}

#[test]
fn index_read_sites_seal_flat_executable_reads() {
    let image = verify_source(INDEXED_SOURCE, INDEXED_IDS).expect("verify");
    let index_sites: Vec<&'static str> = image
        .sites()
        .iter()
        .filter_map(|site| match site {
            // Every index site seals flat-executable and is a read target — there is no
            // index-write site kind. The nonunique byShelf is a progressive-prefix scan;
            // the unique byIsbn an exact lookup.
            SealedSite::Flat {
                target: SealedSiteTarget::IndexScan(_),
                ..
            } => Some("scan"),
            SealedSite::Flat {
                target: SealedSiteTarget::IndexLookup(_),
                ..
            } => Some("lookup"),
            _ => None,
        })
        .collect();
    assert_eq!(index_sites, vec!["scan", "lookup"]);
}

#[test]
fn a_create_or_replace_collides_only_on_the_roots_unique_indexes() {
    let image = verify_source(INDEXED_SOURCE, INDEXED_IDS).expect("verify");
    // The closed unique_index_collision outcome layout for a create/replace on root 0
    // is exactly its unique index (byIsbn, 0x71); the nonunique byShelf never
    // collides.
    assert_eq!(image.unique_collision_outcomes(0), vec![rep(0x71)]);
}

#[test]
fn no_application_opcode_maintains_a_managed_index() {
    // The keep-list law and the release veto: managed-index maintenance is
    // compiler-owned and has no application write path. The absence is structural,
    // enforced on the two independent owners so that adding an index-write path here
    // trips this gate conspicuously: the frozen opcode set names no durable
    // index-maintenance opcode, and the operation-target set names no index *write*
    // target.

    // (1) The only `OP_DUR_*INDEX*` opcodes are the two reads (`SCAN`, `LOOKUP`); no
    // opcode maintains (writes) an index. Scanning the frozen opcode constants of
    // `marrow-image`'s `instr.rs` by source text — as the workspace's other tidy gates
    // scan source — keeps the law honest against a future index-maintenance byte.
    let instr_src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../marrow-image/src/instr.rs"
    ));
    for line in instr_src.lines() {
        let Some(rest) = line.trim_start().strip_prefix("pub const OP_DUR_") else {
            continue;
        };
        let name = rest
            .split(|c: char| c == ':' || c.is_whitespace())
            .next()
            .unwrap_or(rest);
        if !name.contains("INDEX") {
            continue;
        }
        assert!(
            name == "INDEX_SCAN" || name == "INDEX_LOOKUP",
            "durable opcode `OP_DUR_{name}` names an index but is not one of the two \
             reads; managed-index maintenance must remain compiler-owned with no \
             application write opcode",
        );
    }

    // (2) `SemanticTarget` carries exactly the whole-payload, field-leaf, and two index
    // *read* targets and no index *write* target. The exhaustive match fails to compile
    // if a variant is added until it is classified, and an index-write classification
    // trips the assertion.
    for target in [
        SemanticTarget::WholePayload,
        SemanticTarget::FieldLeaf,
        SemanticTarget::GroupEntry,
        SemanticTarget::IndexScan,
        SemanticTarget::IndexLookup,
    ] {
        let is_index_write = match target {
            SemanticTarget::WholePayload
            | SemanticTarget::FieldLeaf
            | SemanticTarget::GroupEntry
            | SemanticTarget::IndexScan
            | SemanticTarget::IndexLookup => false,
        };
        assert!(
            !is_index_write,
            "SemanticTarget::{target:?} is an index-write target; managed-index \
             maintenance must remain compiler-owned with no operation-target write path",
        );
    }
}

#[test]
fn a_missing_index_identity_is_a_precise_mintable_gap() {
    // Drop the byIsbn index anchor: the declaration is well-formed but its identity
    // is incomplete, so the compile fails with the mintable durable-identity gap.
    let missing = INDEXED_IDS.replace(
        "id index books.byIsbn 71717171717171717171717171717171\n",
        "",
    );
    let codes = compile_codes(INDEXED_SOURCE, &missing);
    assert_eq!(codes, vec!["check.durable_identity"]);
}

// --- admission rejections (extracted from the tag's compile_resource_index family) ---

/// A ledger with the base graph fully identified but no index anchors — used for
/// admission rejections, where the invalid index never resolves its own identity.
const BASE_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.shelf 10101010101010101010101010101010\n\
     id field Book.author 12121212121212121212121212121212\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

fn base_source(store_body: &str) -> String {
    format!(
        "resource Book {{\n\
         \x20   required title: string\n\
         \x20   shelf: string\n\
         \x20   author: string\n\
         }}\n\
         \n\
         store ^books[id: int]: Book {{\n{store_body}}}\n\
         \n\
         pub fn label(): string {{\n\
         \x20   return \"books\"\n\
         }}\n"
    )
}

#[test]
fn an_index_argument_naming_no_member_is_rejected() {
    let source = base_source("    index byMissing[missing, id]\n");
    assert_eq!(compile_codes(&source, BASE_IDS), vec!["check.type"]);
}

#[test]
fn a_nonunique_index_omitting_the_identity_key_is_rejected() {
    let source = base_source("    index byShelf[shelf]\n");
    assert_eq!(compile_codes(&source, BASE_IDS), vec!["check.type"]);
}

#[test]
fn a_nonunique_index_with_the_identity_key_not_last_is_rejected() {
    let source = base_source("    index byShelf[id, shelf]\n");
    assert_eq!(compile_codes(&source, BASE_IDS), vec!["check.type"]);
}

#[test]
fn an_index_repeating_a_projection_component_is_rejected() {
    // A repeated component adds no ordering distinction and would double-maintain one
    // cell; each projection component appears at most once.
    let source = base_source("    index byShelf[shelf, shelf, id]\n");
    assert_eq!(compile_codes(&source, BASE_IDS), vec!["check.type"]);
}

#[test]
fn a_duration_field_is_not_an_orderable_managed_index_component() {
    let source = r#"resource Event {
    required span: duration
}

store ^events[id: int]: Event {
    index bySpan[span, id]
}

pub fn label(): string {
    return "events"
}
"#;
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Event 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id field Event.span 10101010101010101010101010101010\n\
         id root events 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id key events.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         id index events.bySpan 70707070707070707070707070707070\n\
         high-water 0\n\
         end\n";
    assert_eq!(compile_codes(source, ids), vec!["check.type"]);
}

#[test]
fn an_index_component_that_is_not_an_orderable_key_scalar_is_rejected() {
    // A dense `struct`-typed field is a widened durable value, not an orderable
    // durable-key scalar, so it cannot be a projection leaf.
    let source = r#"struct Money {
    cents: int
}

resource Book {
    required title: string
    price: Money
}

store ^books[id: int]: Book {
    index byPrice[price, id]
}

pub fn label(): string {
    return "books"
}
"#;
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         id field Book.price 10101010101010101010101010101010\n\
         id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         high-water 0\n\
         end\n";
    assert_eq!(compile_codes(source, ids), vec!["check.type"]);
}

#[test]
fn an_index_name_colliding_with_a_field_is_rejected() {
    let source = base_source("    index shelf[author, id]\n");
    assert_eq!(compile_codes(&source, BASE_IDS), vec!["check.type"]);
}

#[test]
fn a_duplicate_index_name_is_rejected() {
    // The first `byShelf` is well-formed (so it resolves its `Index` anchor); the
    // second collides on the name. Its anchor is present so the collision is the sole
    // diagnostic.
    let ids = BASE_IDS.replace(
        "high-water 0\n",
        "id index books.byShelf 70707070707070707070707070707070\nhigh-water 0\n",
    );
    let source = base_source("    index byShelf[shelf, id]\n    index byShelf[title, id]\n");
    assert_eq!(compile_codes(&source, &ids), vec!["check.type"]);
}

#[test]
fn a_root_exceeding_the_managed_index_cap_is_rejected() {
    // The checker caps a store root at eight managed indexes (well below the image's
    // structural decode bound). Nine well-formed declarations are refused on the count
    // alone — the graph is discarded before any index mints an identity, so no ledger
    // anchors are needed.
    let mut body = String::new();
    for n in 1..=9 {
        body.push_str(&format!("    index by{n}[shelf, id]\n"));
    }
    assert_eq!(
        compile_codes(&base_source(&body), BASE_IDS),
        vec!["check.type"]
    );
}

#[test]
fn an_index_on_a_singleton_root_is_rejected() {
    let source = r#"resource Settings {
    theme: string
}

store ^settings: Settings {
    index byTheme[theme]
}

pub fn label(): string {
    return "s"
}
"#;
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Settings 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id field Settings.theme 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         id root settings 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         high-water 0\n\
         end\n";
    assert_eq!(compile_codes(source, ids), vec!["check.type"]);
}

#[test]
fn a_source_index_read_compiles() {
    // The managed-index read runtime has landed: a bounded scan of a nonunique index
    // (binding the source `Id(^books)`) and a bracket lookup of a unique index compile
    // cleanly through the production pipeline. Runtime behavior is exercised end to end
    // in the VM `index_read` fixtures; this asserts the source forms are admitted.
    let source = r#"resource Book {
    required title: string
    shelf: string
    isbn: string
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
    index byIsbn[isbn] unique
}

pub fn countOnShelf(s: string): int {
    var n = 0
    for id in ^books.byShelf[s] at most 100 {
        n += 1
    } on more {
        n = -1
    }
    return n
}

pub fn find(s: string): Id(^books)? {
    if const found = ^books.byIsbn[s] {
        return found
    }
    return absent
}
"#;
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         id field Book.shelf 10101010101010101010101010101010\n\
         id field Book.isbn 20202020202020202020202020202020\n\
         id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         id index books.byShelf 70707070707070707070707070707070\n\
         id index books.byIsbn 80808080808080808080808080808080\n\
         high-water 0\n\
         end\n";
    assert!(compile_codes(source, ids).is_empty());
}
