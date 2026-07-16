//! D00 slice 3b: durable-graph breadth — static `group` namespaces and keyed
//! `branch` placements.
//!
//! A resource's durable shape is a member tree: its top-level fields, plus static
//! `group` field-path namespaces (unkeyed) and keyed `branch` placements (a nested
//! keyed subtree, a distinct graph node with its own placement id and key tuple).
//! Every group and branch is a distinct node with a complete ledger identity — a
//! `group`/`root` placement anchor, one `key` per branch column, and one `field`
//! per stored field with a group- or branch-qualified path — a slot in the image
//! DURABLE member tree, and a contribution to the durable-contract identity the
//! verifier independently re-encodes. A keyed `branch` of scalar fields is executable
//! (see `durable_branches`/`durable_nested_branches`); a resource declaring a static
//! `group` is not yet executable, so every durable operation over a group-bearing root is
//! a precise typed `check.unsupported` rejection rather than a silent drop. A group's
//! fields are part of the containing entry's materialized resource value, a model the flat
//! durable record does not yet carry, so groups park until that materialized-value model
//! lands.

use marrow_compile::{Compiled, SourceDiagnostic};
use marrow_verify::DurableContractId;

fn compile(source: &str, ids: &str) -> Result<Compiled, Vec<SourceDiagnostic>> {
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
    marrow_compile::compile(&project)
}

fn contract_of(source: &str, ids: &str) -> DurableContractId {
    let compiled = compile(source, ids).expect("compile");
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    image.durable_contract()
}

fn codes(diagnostics: &[SourceDiagnostic]) -> Vec<&str> {
    diagnostics.iter().map(|d| d.code).collect()
}

// A resource with a top-level field, a static `group` holding a field, and a keyed
// `branch` holding a field and its own nested group.
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

// The full ledger: application, product, the top-level field, the root placement
// and its key, the `details` group and its field, and the `notes` branch (a `root`
// placement), its key, and its two fields — every anchor group- or branch-qualified.
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

#[test]
fn a_group_and_branch_resource_completes_its_identity_and_verifies() {
    let id = contract_of(LIBRARY_SOURCE, LIBRARY_IDS);
    // Stable across recompilation.
    assert_eq!(id, contract_of(LIBRARY_SOURCE, LIBRARY_IDS));
}

/// The durable-contract identity tracks the durable graph, not the surrounding
/// program: adding unrelated storeless code and reordering declarations leaves
/// every root, key, group, branch placement, and field id — and so the contract
/// id — from drifting. This is the id-stability-under-unrelated-edits property
/// across the widened graph's kinds.
#[test]
fn unrelated_source_edits_do_not_drift_the_contract_id() {
    let base = contract_of(LIBRARY_SOURCE, LIBRARY_IDS);

    // Append an unrelated storeless function: the durable graph is untouched.
    let appended = format!("{LIBRARY_SOURCE}\npub fn unrelated(n: int): int\n    return n + 1\n");
    assert_eq!(
        base,
        contract_of(&appended, LIBRARY_IDS),
        "unrelated storeless code does not drift the durable identity"
    );

    // Declare the same unrelated function ahead of the resource: declaration order
    // is not part of the identity either.
    let reordered = format!("pub fn unrelated(n: int): int\n    return n + 1\n\n{LIBRARY_SOURCE}");
    assert_eq!(
        base,
        contract_of(&reordered, LIBRARY_IDS),
        "declaration order does not drift the durable identity"
    );
}

#[test]
fn an_operation_over_a_group_bearing_root_is_not_yet_executable() {
    // A resource declaring a static `group` is off the flat-executable path: the group's
    // scalar fields are part of the containing entry's materialized resource value and its
    // required-completeness, a model the flat durable record does not yet carry, so every
    // durable operation over the group-bearing root is a precise typed `check.unsupported`
    // rejection rather than a silent drop or a guessed partial-group semantics. (A keyed
    // `branch` on the same resource is executable; the group is what parks the root.)
    let source = format!(
        "{LIBRARY_SOURCE}\npub fn firstTitle(id: int): string?\n    return ^books(id).title\n"
    );
    let diagnostics = compile(&source, LIBRARY_IDS).expect_err("a group-bearing root parks");
    assert!(
        codes(&diagnostics).contains(&"check.unsupported"),
        "{diagnostics:?}"
    );
    // The rejection carries a located span (1-based line/column into the source).
    let hit = diagnostics
        .iter()
        .find(|d| d.code == "check.unsupported")
        .expect("a check.unsupported diagnostic");
    assert!(hit.line >= 1 && hit.column >= 1, "{hit:?}");
}

#[test]
fn a_missing_group_identity_fails_precisely() {
    let without_group = LIBRARY_IDS.replace(
        "id group Book.details 20202020202020202020202020202020\n",
        "",
    );
    let diagnostics = compile(LIBRARY_SOURCE, &without_group).expect_err("incomplete identity");
    assert!(
        codes(&diagnostics).contains(&"check.durable_identity"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("group `Book.details`")),
        "the gap names the group anchor: {diagnostics:?}"
    );
}

#[test]
fn a_missing_group_field_identity_fails_precisely() {
    let without_field = LIBRARY_IDS.replace(
        "id field Book.details.pages 21212121212121212121212121212121\n",
        "",
    );
    let diagnostics = compile(LIBRARY_SOURCE, &without_field).expect_err("incomplete identity");
    assert!(
        codes(&diagnostics).contains(&"check.durable_identity"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("field `Book.details.pages`")),
        "the gap names the group-qualified field path: {diagnostics:?}"
    );
}

#[test]
fn a_missing_branch_placement_identity_fails_precisely() {
    let without_branch =
        LIBRARY_IDS.replace("id root Book.notes 30303030303030303030303030303030\n", "");
    let diagnostics = compile(LIBRARY_SOURCE, &without_branch).expect_err("incomplete identity");
    assert!(
        codes(&diagnostics).contains(&"check.durable_identity"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_missing_branch_key_identity_fails_precisely() {
    let without_key = LIBRARY_IDS.replace(
        "id key Book.notes.noteId 31313131313131313131313131313131\n",
        "",
    );
    let diagnostics = compile(LIBRARY_SOURCE, &without_key).expect_err("incomplete identity");
    assert!(
        codes(&diagnostics).contains(&"check.durable_identity"),
        "{diagnostics:?}"
    );
}

#[test]
fn operating_on_a_group_or_branch_resource_is_not_yet_executable() {
    let source = "resource Book\n\
         \x20   required title: string\n\
         \n\
         \x20   details\n\
         \x20       pages: int\n\
         \n\
         store ^books(id: int): Book\n\
         \n\
         pub fn title(id: int): string?\n\
         \x20   return ^books(id).title\n";
    let ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         id group Book.details 20202020202020202020202020202020\n\
         id field Book.details.pages 21212121212121212121212121212121\n\
         high-water 0\n\
         end\n";
    // The identity is complete, so this is a not-yet-executable rejection — not an
    // identity gap.
    let diagnostics = compile(source, ids).expect_err("not yet executable");
    assert!(
        codes(&diagnostics).contains(&"check.unsupported"),
        "{diagnostics:?}"
    );
    assert!(
        !codes(&diagnostics).contains(&"check.durable_identity"),
        "a complete-identity group resource reports executability, not a gap: {diagnostics:?}"
    );
}

#[test]
fn renaming_a_group_with_a_moved_anchor_preserves_the_identity() {
    let base = contract_of(LIBRARY_SOURCE, LIBRARY_IDS);

    // Rename the `details` group to `info`, moving both its anchor and its field's
    // anchor while their ids stay. Identity follows the ids, so it is preserved.
    let renamed_source = LIBRARY_SOURCE.replace("details", "info");
    let renamed_ids = LIBRARY_IDS
        .replace("Book.details.pages", "Book.info.pages")
        .replace("Book.details", "Book.info");
    assert_eq!(
        base,
        contract_of(renamed_source.as_str(), renamed_ids.as_str()),
        "a group rename whose anchors moved preserves the identity"
    );

    // A re-minted group id at the same anchor is a different graph.
    let re_minted = renamed_ids.replace(
        "20202020202020202020202020202020",
        "22222222222222222222222222222222",
    );
    assert_ne!(
        base,
        contract_of(renamed_source.as_str(), re_minted.as_str()),
        "a fresh group id is a different durable identity"
    );
}

#[test]
fn re_minting_a_branch_placement_changes_the_identity() {
    let base = contract_of(LIBRARY_SOURCE, LIBRARY_IDS);
    let re_minted = LIBRARY_IDS.replace(
        "id root Book.notes 30303030303030303030303030303030",
        "id root Book.notes 3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f",
    );
    assert_ne!(
        base,
        contract_of(LIBRARY_SOURCE, &re_minted),
        "a fresh branch placement id is a different durable identity"
    );
}

#[test]
fn promoting_a_group_field_to_a_top_level_field_changes_the_identity() {
    let base = contract_of(LIBRARY_SOURCE, LIBRARY_IDS);

    // Move `pages` out of the `details` group to a top-level field of the resource,
    // keeping its ledger id at the new (top-level) anchor. The graph structure
    // changed — a top-level field versus a group-nested field — so the identity
    // changes even though no id was re-minted.
    let flat_source = "resource Book\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         \n\
         \x20   notes(noteId: string)\n\
         \x20       required text: string\n\
         \x20       createdAt: instant\n\
         \n\
         store ^books(id: int): Book\n\
         \n\
         pub fn label(): string\n\
         \x20   return \"books\"\n";
    // The group is gone; `pages` now anchors at `Book.pages` with the same id, and
    // the `details` group anchor is dropped.
    let flat_ids = LIBRARY_IDS
        .replace(
            "id field Book.details.pages 21212121212121212121212121212121\n",
            "id field Book.pages 21212121212121212121212121212121\n",
        )
        .replace(
            "id group Book.details 20202020202020202020202020202020\n",
            "",
        );
    assert_ne!(
        base,
        contract_of(flat_source, &flat_ids),
        "a group-nested field and a top-level field of the same id are different graphs"
    );
}

#[test]
fn a_retired_group_anchor_cannot_be_reused() {
    // Retire the `details` group anchor: re-declaring at it fails closed, never
    // reusing the retired id.
    let retired_ids = "marrow ids v0\n\
         machine-written by marrow; do not edit\n\
         id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
         id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
         id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
         id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
         id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
         id field Book.details.pages 21212121212121212121212121212121\n\
         id root Book.notes 30303030303030303030303030303030\n\
         id key Book.notes.noteId 31313131313131313131313131313131\n\
         id field Book.notes.text 32323232323232323232323232323232\n\
         id field Book.notes.createdAt 33333333333333333333333333333333\n\
         retired group Book.details 20202020202020202020202020202020 1\n\
         high-water 1\n\
         end\n";
    let diagnostics = compile(LIBRARY_SOURCE, retired_ids).expect_err("retired anchor");
    assert!(
        codes(&diagnostics).contains(&"check.durable_identity"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|d| d.message.contains("retired")),
        "the diagnostic names the retirement: {diagnostics:?}"
    );
}
