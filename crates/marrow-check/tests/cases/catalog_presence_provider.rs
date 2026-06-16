//! The checker consumes the accepted catalog as a caller-supplied provider input, threaded
//! through the `analyze_project` parameter. These tests inject the snapshot directly and
//! prove that identity binds against it: the accepted ids carry forward onto live facts, and
//! a source-only check proposes a first epoch while writing nothing.
use crate::support;
use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogMetadata};
use marrow_check::{ProjectSources, analyze_project};

use support::catalog::{catalog, derived_id, entry as literal_entry};
use support::{config, temp_root, write};

/// An accepted catalog entry whose stable id is minted from a readable `label`, so a fixture
/// names a member by label and the assertions that look the id back up agree without sharing
/// a literal constant.
fn entry(
    kind: CatalogEntryKind,
    canonical_path: &str,
    label: &str,
    aliases: &[&str],
) -> CatalogEntry {
    literal_entry(kind, canonical_path, &derived_id(label), aliases)
}

/// The `books::Book` source one accepted snapshot already carries identity for.
fn books_source(root: &std::path::Path) {
    write(
        root,
        "src/books.mw",
        "module books\nresource Book\n    title: string\nstore ^books(id: int): Book\n",
    );
}

/// The accepted snapshot whose ids the binding must carry forward unchanged.
fn books_accepted() -> CatalogMetadata {
    catalog(vec![
        entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
        entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
        entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::title",
            "member-title",
            &[],
        ),
    ])
}

#[test]
fn source_only_check_proposes_epoch_one_and_writes_nothing() {
    let root = temp_root("provider-source-only");
    books_source(&root);

    let snapshot =
        analyze_project(&root, &config(), &ProjectSources::new(), None).expect("analyze");

    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let proposal = snapshot
        .program
        .catalog
        .proposal
        .expect("first-run proposal");
    assert_eq!(proposal.epoch, 1);
    assert_eq!(snapshot.program.catalog.accepted_epoch, None);
    // The checker is read-only: it proposes a baseline but establishes no durable state,
    // so the project directory holds only the source it came in with.
    let entries: Vec<_> = std::fs::read_dir(&*root)
        .expect("read project root")
        .map(|entry| entry.expect("dir entry").file_name())
        .collect();
    assert_eq!(
        entries,
        [std::ffi::OsString::from("src")],
        "a source-only check must not write any durable artifact: {entries:?}"
    );
}

#[test]
fn injected_snapshot_binds_identity_exactly_as_the_accepted_catalog_did() {
    let root = temp_root("provider-identity-preserved");
    books_source(&root);
    let accepted = books_accepted();

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze");

    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let program = &snapshot.program;
    assert_eq!(program.catalog.accepted_epoch, Some(7));

    // The accepted ids are carried forward onto the live source facts exactly: the
    // resource binds the accepted resource id, not a freshly minted one.
    let module = program.facts.module_id("books").expect("books module");
    let resource = program.facts.resource_id(module, "Book").expect("Book");
    assert_eq!(
        program.facts.resource(resource).catalog_id.as_deref(),
        Some(derived_id("res-book").as_str()),
        "the injected accepted id binds onto the live resource fact"
    );

    // Source matches the accepted snapshot exactly, so there is no proposal to advance.
    assert!(
        program.catalog.proposal.is_none(),
        "an unchanged program against its accepted snapshot proposes nothing"
    );
    assert_eq!(
        program.catalog.accepted_entries, accepted.entries,
        "the accepted entries are the injected snapshot's, verbatim"
    );
}

#[test]
fn proposal_only_member_binds_activation_default_not_ordinary_facts() {
    // A brand-new member current source adds has no accepted id; its identity lives
    // only in the proposal. An `evolve default` over it binds through the proposal id,
    // while the live resource fact keeps the accepted-only binding (no proposal id leaks
    // onto ordinary facts).
    let root = temp_root("provider-proposal-only-default");
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   title: string\n\
         \x20   required pages: int\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   default Book.pages = 0\n",
    );
    let accepted = books_accepted();

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze");

    let program = &snapshot.program;
    let proposal = program
        .catalog
        .proposal
        .as_ref()
        .expect("a new member advances the proposal");
    let pages_proposal_id = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::pages"
        })
        .expect("proposal carries the new member")
        .stable_id
        .clone();

    // The default binds through the proposal id of the brand-new member.
    let default = program
        .catalog
        .evolve_defaults
        .iter()
        .find(|default| default.catalog_id == pages_proposal_id)
        .expect("default binds the new member's proposal id");
    assert_eq!(default.catalog_id, pages_proposal_id);

    // The accepted-only ids never carry the proposal-only member, so no live fact is
    // bound to it: it has no accepted identity yet.
    assert!(
        !program
            .catalog
            .accepted_entries
            .iter()
            .any(|entry| entry.stable_id == pages_proposal_id),
        "a proposal-only id is not an accepted entry"
    );
}
