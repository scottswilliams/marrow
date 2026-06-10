//! The checker consumes the accepted catalog as a caller-supplied provider input,
//! not by reading `marrow.catalog.json` itself. These tests inject the snapshot
//! directly through the `analyze_project` parameter and prove that identity binds
//! exactly as the disk-driven path does: same bound ids, same proposal. The catalog
//! file is never written here.

mod support;

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogMetadata};
use marrow_check::{
    CHECK_CATALOG_INTENT, CheckDiagnostic, ProjectSources, accepted_catalog_from_json,
    analyze_project,
};

use support::catalog::{catalog, derived_id, entry as literal_entry};
use support::{config, temp_root, write};

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
        "module books\nresource Book at ^books(id: int)\n    title: string\n",
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
    // The checker is read-only: providing no accepted snapshot leaves no file behind.
    assert!(
        !root.join("marrow.catalog.json").exists(),
        "a source-only check must not write the accepted catalog"
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
fn invalid_injected_snapshot_surfaces_catalog_intent() {
    let mut diagnostics: Vec<CheckDiagnostic> = Vec::new();
    let accepted = accepted_catalog_from_json(
        "{ not valid catalog json",
        std::path::Path::new("marrow.catalog.json"),
        &mut diagnostics,
    );

    assert!(accepted.is_none(), "invalid bytes parse to no snapshot");
    assert_eq!(
        with_code_in(&diagnostics, CHECK_CATALOG_INTENT).len(),
        1,
        "an invalid snapshot raises exactly one catalog-intent diagnostic: {diagnostics:#?}"
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
         resource Book at ^books(id: int)\n\
         \x20   title: string\n\
         \x20   required pages: int\n\
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

/// The diagnostics whose code is `code`, in order. The shared report helper takes a
/// `CheckReport`; these provider cases hold a bare diagnostics vec.
fn with_code_in<'a>(diagnostics: &'a [CheckDiagnostic], code: &str) -> Vec<&'a CheckDiagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == code)
        .collect()
}
