//! Durable-graph breadth: static `group` namespaces and keyed
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
//! (see `durable_branches`/`durable_nested_branches`), and a resource declaring a
//! root-level `group` of scalar/widened leaves is executable too — its whole read/replace/
//! erase and group-leaf operations run end to end in `durable_groups`. A group nested in a
//! branch or in another group parks. This module covers the identity side; its
//! executability assertions confirm a root-level group does not park the root.

use crate::common::{Diagnostics, Project};
use marrow_project::IdentityKind;
use marrow_verify::DurableContractId;

/// Compile and independently verify, returning the durable-contract identity.
fn contract_of(source: &str, ids: &str) -> DurableContractId {
    Project::single(source).ids(ids).image().durable_contract()
}

/// The typed rejection diagnostics for a project that must not compile; `why` names the
/// defect the fixture carries.
fn rejection(source: &str, ids: &str, why: &str) -> Diagnostics {
    match Project::single(source).ids(ids).try_image() {
        Ok(_) => panic!("expected a rejection: {why}"),
        Err(diagnostics) => diagnostics,
    }
}

// A resource with a top-level field, a static `group` holding a field, and a keyed
// `branch` holding a field and its own nested group.
const LIBRARY_SOURCE: &str = r#"resource Book {
    required title: string

    details {
        pages: int
    }

    notes[noteId: string] {
        required text: string
        createdAt: instant
    }
}

store ^books[id: int]: Book

pub fn label(): string {
    return "books"
}
"#;

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
    let appended =
        format!("{LIBRARY_SOURCE}\npub fn unrelated(n: int): int {{\n    return n + 1\n}}\n");
    assert_eq!(
        base,
        contract_of(&appended, LIBRARY_IDS),
        "unrelated storeless code does not drift the durable identity"
    );

    // Declare the same unrelated function ahead of the resource: declaration order
    // is not part of the identity either.
    let reordered =
        format!("pub fn unrelated(n: int): int {{\n    return n + 1\n}}\n\n{LIBRARY_SOURCE}");
    assert_eq!(
        base,
        contract_of(&reordered, LIBRARY_IDS),
        "declaration order does not drift the durable identity"
    );
}

#[test]
fn an_operation_over_a_root_level_group_bearing_root_is_executable() {
    // A resource declaring a root-level `group` of scalar/widened leaves is on the
    // flat-executable path: the group's leaves are a markerless value unit of the
    // containing entry, so a top-level field read (and the group operations, exercised end
    // to end in `durable_groups`) compiles cleanly rather than parking. (A keyed `branch`
    // on the same resource is executable too; the nested group inside that branch stays
    // parked, but it does not park the whole root.)
    let source = format!(
        "{LIBRARY_SOURCE}\npub fn firstTitle(id: int): string? {{\n    return ^books[id].title\n}}\n"
    );
    let outcome = Project::single(&source).ids(LIBRARY_IDS).try_image();
    assert!(
        outcome.is_ok(),
        "a root-level group-bearing root is executable: {:?}",
        outcome
            .err()
            .map(|diagnostics| format!("{:?}", diagnostics.all()))
    );
}

#[test]
fn a_missing_group_identity_fails_precisely() {
    let without_group = LIBRARY_IDS.replace(
        "id group Book.details 20202020202020202020202020202020\n",
        "",
    );
    let diagnostics = rejection(LIBRARY_SOURCE, &without_group, "incomplete identity");
    assert!(
        diagnostics.has_code("check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
    assert!(
        diagnostics.names_identity_gap(IdentityKind::Group, "Book.details", false),
        "the gap names the group anchor: {:?}",
        diagnostics.messages()
    );
}

#[test]
fn a_missing_group_field_identity_fails_precisely() {
    let without_field = LIBRARY_IDS.replace(
        "id field Book.details.pages 21212121212121212121212121212121\n",
        "",
    );
    let diagnostics = rejection(LIBRARY_SOURCE, &without_field, "incomplete identity");
    assert!(
        diagnostics.has_code("check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
    assert!(
        diagnostics.names_identity_gap(IdentityKind::Field, "Book.details.pages", false),
        "the gap names the group-qualified field path: {:?}",
        diagnostics.messages()
    );
}

#[test]
fn a_missing_branch_referencement_identity_fails_precisely() {
    let without_branch =
        LIBRARY_IDS.replace("id root Book.notes 30303030303030303030303030303030\n", "");
    let diagnostics = rejection(LIBRARY_SOURCE, &without_branch, "incomplete identity");
    assert!(
        diagnostics.has_code("check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
}

#[test]
fn a_missing_branch_key_identity_fails_precisely() {
    let without_key = LIBRARY_IDS.replace(
        "id key Book.notes.noteId 31313131313131313131313131313131\n",
        "",
    );
    let diagnostics = rejection(LIBRARY_SOURCE, &without_key, "incomplete identity");
    assert!(
        diagnostics.has_code("check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
}

#[test]
fn a_complete_identity_root_level_group_resource_is_executable() {
    let source = r#"resource Book {
    required title: string

    details {
        pages: int
    }
}

store ^books[id: int]: Book

pub fn title(id: int): string? {
    return ^books[id].title
}

pub fn pages(id: int): int? {
    return ^books[id].details.pages
}
"#;
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
    // A complete-identity root-level group resource is executable: a top-level field read
    // and a group-leaf read both compile.
    let outcome = Project::single(source).ids(ids).try_image();
    assert!(
        outcome.is_ok(),
        "a complete-identity root-level group resource is executable: {:?}",
        outcome
            .err()
            .map(|diagnostics| format!("{:?}", diagnostics.all()))
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
fn re_minting_a_branch_referencement_changes_the_identity() {
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
    let flat_source = r#"resource Book {
    required title: string
    pages: int

    notes[noteId: string] {
        required text: string
        createdAt: instant
    }
}

store ^books[id: int]: Book

pub fn label(): string {
    return "books"
}
"#;
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
    let diagnostics = rejection(LIBRARY_SOURCE, retired_ids, "retired anchor");
    assert!(
        diagnostics.has_code("check.durable_identity"),
        "{:?}",
        diagnostics.all()
    );
    assert!(
        diagnostics.names_identity_gap(IdentityKind::Group, "Book.details", true),
        "the gap names the retired group anchor: {:?}",
        diagnostics.messages()
    );
}

// Two stored resources whose member orders differ from key order: `Book` writes
// `title` before `author` and its `details` leaves `year` before `edition`, and
// `Booklet` — a name with `Book` as a prefix — shares the field name `title`.
const ORDER_SOURCE: &str = r#"resource Book {
    required title: string
    author: string

    details {
        year: int
        edition: int
    }
}

resource Booklet {
    required title: string
    pages: int
}

store ^books[id: int]: Book
store ^booklets[id: int]: Booklet

pub fn label(): string {
    return "books"
}
"#;

const ORDER_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.author 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.year 21212121212121212121212121212121\n\
     id field Book.details.edition 22222222222222222222222222222222\n\
     id product Booklet 40404040404040404040404040404040\n\
     id field Booklet.title 41414141414141414141414141414141\n\
     id field Booklet.pages 42424242424242424242424242424242\n\
     id root booklets 43434343434343434343434343434343\n\
     id key booklets.id 44444444444444444444444444444444\n\
     high-water 0\n\
     end\n";

/// A record's field list and its group's leaf list are each its own members in
/// declaration order, read through the independently verified image: no member of
/// another record or of the record's group joins a list, and none is reordered.
#[test]
fn a_record_and_its_group_keep_declared_member_order() {
    let image = Project::single(ORDER_SOURCE).ids(ORDER_IDS).image();
    let names = |record| -> Vec<String> {
        image
            .record_type(record)
            .fields()
            .iter()
            .map(|field| field.name().to_string())
            .collect()
    };
    let root = |name: &str| {
        image
            .roots()
            .iter()
            .find(|root| root.name() == name)
            .unwrap_or_else(|| panic!("root {name}"))
    };

    let books = root("books");
    assert_eq!(names(books.record()), ["title", "author", "details"]);
    assert_eq!(
        books.groups().len(),
        1,
        "the group is executable, so sealed"
    );
    assert_eq!(names(books.groups()[0].record()), ["year", "edition"]);
    assert_eq!(names(root("booklets").record()), ["title", "pages"]);
}
