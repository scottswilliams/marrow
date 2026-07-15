//! D02 slice 1: the derived stable semantic path of every durable graph node.
//!
//! Every node of a program's durable graph — a root placement, a static `group`
//! namespace, a keyed `branch` placement, and each stored field — has a derived
//! stable [`SemanticPath`]: the ordered chain of kind-tagged ledger ids from the
//! application down to the node. The path follows the ledger ids, not the source
//! spelling, so a rename that moves a ledger anchor (id unchanged) leaves every
//! node's path unchanged, while re-minting an id changes exactly the paths that
//! pass through it. This is the D02 exit-gate row-1 property, observed through the
//! full production path: capture -> compile -> verify -> semantic nodes.

use marrow_verify::{SemanticNodeKind, SemanticStepKind};

/// A resource with a top-level field, a static `group` holding a field, and a keyed
/// `branch` holding two fields — every durable node kind in one graph.
const LIBRARY_SOURCE: &str = "resource Book\n\
     \x20   required title: string\n\
     \n\
     \x20   details\n\
     \x20       pages: int\n\
     \n\
     \x20   notes(noteId: string)\n\
     \x20       required text: string\n\
     \x20       createdAt: instant\n\
     \n\
     store ^books(id: int): Book\n\
     \n\
     pub fn label(): string\n\
     \x20   return \"books\"\n";

const LIBRARY_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     id field Book.notes.createdAt 33333333333333333333333333333333\n\
     high-water 0\n\
     end\n";

/// The `(kind, terminal-ledger-id)` fingerprint of every derived semantic node,
/// sorted, observed through the full production path. The terminal id is the last
/// step's ledger id — the node's own placement/group/field id.
fn node_fingerprints(source: &str, ids: &str) -> Vec<(SemanticNodeKind, [u8; 16])> {
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
    let compiled = marrow_compile::compile(&project).expect("compile");
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    let mut nodes: Vec<(SemanticNodeKind, [u8; 16])> = image
        .semantic_nodes()
        .iter()
        .map(|node| {
            let terminal = node.path.steps().last().expect("a node path is non-empty");
            (node.kind, *terminal.id.bytes())
        })
        .collect();
    nodes.sort();
    nodes
}

fn rep(byte: u8) -> [u8; 16] {
    [byte; 16]
}

#[test]
fn every_durable_node_has_a_semantic_path_ending_in_its_ledger_id() {
    let mut expected = vec![
        (SemanticNodeKind::Root, rep(0x0b)),
        (SemanticNodeKind::Field, rep(0x0e)),  // Book.title
        (SemanticNodeKind::Group, rep(0x20)),  // Book.details
        (SemanticNodeKind::Field, rep(0x21)),  // Book.details.pages
        (SemanticNodeKind::Branch, rep(0x30)), // Book.notes
        (SemanticNodeKind::Field, rep(0x32)),  // Book.notes.text
        (SemanticNodeKind::Field, rep(0x33)),  // Book.notes.createdAt
    ];
    expected.sort();
    assert_eq!(node_fingerprints(LIBRARY_SOURCE, LIBRARY_IDS), expected);
}

#[test]
fn a_field_path_runs_from_the_application_through_its_container() {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        LIBRARY_SOURCE.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(LIBRARY_IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    let nodes = image.semantic_nodes();

    // The group-nested field `pages`: application -> root placement -> group -> field.
    let pages = nodes
        .iter()
        .find(|n| n.path.steps().last().unwrap().id.bytes() == &rep(0x21))
        .expect("the group field is a node");
    let kinds: Vec<SemanticStepKind> = pages.path.steps().iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![
            SemanticStepKind::Application,
            SemanticStepKind::Placement, // ^books root
            SemanticStepKind::Group,     // details
            SemanticStepKind::Field,     // pages
        ]
    );
    let ids: Vec<[u8; 16]> = pages.path.steps().iter().map(|s| *s.id.bytes()).collect();
    assert_eq!(ids, vec![rep(0x0a), rep(0x0b), rep(0x20), rep(0x21)]);

    // The branch field `notes.text`: the branch step is a Placement (a branch is a
    // keyed node just like a root), not a Group.
    let text = nodes
        .iter()
        .find(|n| n.path.steps().last().unwrap().id.bytes() == &rep(0x32))
        .expect("the branch field is a node");
    let kinds: Vec<SemanticStepKind> = text.path.steps().iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![
            SemanticStepKind::Application,
            SemanticStepKind::Placement, // ^books root
            SemanticStepKind::Placement, // notes branch
            SemanticStepKind::Field,     // text
        ]
    );
}

#[test]
fn a_rename_with_a_moved_anchor_preserves_every_semantic_path() {
    let base = node_fingerprints(LIBRARY_SOURCE, LIBRARY_IDS);

    // Rename the `details` group to `info`, moving both its anchor and its field's
    // anchor while their ids stay. Paths follow ids, so every node's path — its
    // whole chain — is unchanged.
    let renamed_source = LIBRARY_SOURCE.replace("details", "info");
    let renamed_ids = LIBRARY_IDS
        .replace("Book.details.pages", "Book.info.pages")
        .replace("Book.details", "Book.info");
    assert_eq!(
        base,
        node_fingerprints(&renamed_source, &renamed_ids),
        "a rename whose anchors moved (ids unchanged) preserves every semantic path"
    );
}

#[test]
fn re_minting_a_group_id_changes_exactly_the_paths_through_it() {
    let base = node_fingerprints(LIBRARY_SOURCE, LIBRARY_IDS);
    // Re-mint the `details` group id: the group node and its nested field node move
    // to the fresh id; every other node is untouched.
    let re_minted = LIBRARY_IDS.replace(
        "id group Book.details 20202020202020202020202020202020",
        "id group Book.details 22222222222222222222222222222222",
    );
    let after = node_fingerprints(LIBRARY_SOURCE, &re_minted);
    assert_ne!(base, after);
    // The group's terminal id changed to the fresh id.
    assert!(after.contains(&(SemanticNodeKind::Group, rep(0x22))));
    assert!(!after.contains(&(SemanticNodeKind::Group, rep(0x20))));
    // The field `title`, unrelated to the group, keeps its path.
    assert!(after.contains(&(SemanticNodeKind::Field, rep(0x0e))));
}
