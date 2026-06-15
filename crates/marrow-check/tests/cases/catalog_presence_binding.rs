use crate::support;
use std::fs;

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
use marrow_check::{CHECK_CATALOG_INTENT, DiagnosticPayload};

use support::catalog::{catalog, catalog_path, derived_id, entry as literal_entry, write_catalog};
use support::{check_with_accepted, temp_project, write};

/// A catalog entry whose stable id is minted deterministically from `label`, so a
/// fixture refers to a member by a readable name and still gets a `cat_`-shaped id the
/// catalog parser accepts. Tests that need a specific literal id call
/// [`literal_entry`] (the shared builder) directly.
fn entry(
    kind: CatalogEntryKind,
    canonical_path: &str,
    label: &str,
    aliases: &[&str],
) -> CatalogEntry {
    literal_entry(kind, canonical_path, &derived_id(label), aliases)
}

#[test]
fn first_source_check_proposes_catalog_ids_without_writing_accepted_catalog() {
    let root = temp_project("catalog-proposal", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
             store ^books(id: int): Book\n\
             \x20   index byTitle(title) unique\n\
             enum Status\n\
             \x20   active\n\
             \x20   archived\n",
        );
    });

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program.catalog.proposal.expect("catalog proposal");
    assert_eq!(proposal.epoch, 1);
    let module = program.facts.module_id("books").expect("books module");
    let resource = program.facts.resource_id(module, "Book").expect("Book");
    assert_eq!(
        program.facts.resource(resource).catalog_id,
        None,
        "unaccepted proposal IDs stay proposal-only"
    );
    assert!(
        proposal
            .entries
            .iter()
            .any(|entry| entry.kind == CatalogEntryKind::Resource && entry.path == "books::Book")
    );
    assert!(
        proposal
            .entries
            .iter()
            .any(|entry| entry.kind == CatalogEntryKind::Store && entry.path == "books::^books")
    );
}

#[test]
fn source_only_check_leaves_accepted_catalog_epoch_unchanged() {
    let root = temp_project("catalog-epoch", |root| {
        write(
            root,
            "src/books.mw",
            "module books\nresource Book\n    title: string\nstore ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });
    let before =
        CatalogMetadata::from_json(&fs::read_to_string(catalog_path(&root)).expect("read before"))
            .expect("accepted catalog parses before");

    let (report, program) = check_with_accepted(&root);
    let after =
        CatalogMetadata::from_json(&fs::read_to_string(catalog_path(&root)).expect("read after"))
            .expect("accepted catalog parses after");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert_eq!(program.catalog.accepted_epoch, Some(7));
    assert_eq!(before.epoch, after.epoch);
    assert_eq!(before, after);
}

#[test]
fn non_active_catalog_entries_and_aliases_do_not_bind_live_source_facts() {
    let root = temp_project("catalog-non-active", |root| {
        write(
            root,
            "src/library.mw",
            "module library\nresource Book\n    title: string\nstore ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            CatalogEntry {
                lifecycle: CatalogLifecycle::Reserved,
                ..entry(
                    CatalogEntryKind::Resource,
                    "books::ReservedBook",
                    "reserved-book",
                    &["library::Book"],
                )
            },
            entry(
                CatalogEntryKind::Store,
                "library::^books",
                "store-books",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "library::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "{:#?}",
        report.diagnostics
    );
    let module = program.facts.module_id("library").expect("module");
    let resource = program.facts.resource_id(module, "Book").expect("resource");
    assert_eq!(program.facts.resource(resource).catalog_id, None);
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty().expect("catalog renders"))
        .expect("proposal validates");
}

#[test]
fn reserved_catalog_path_blocks_source_reuse_without_intent() {
    let root = temp_project("catalog-reserved-path", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            CatalogEntry {
                lifecycle: CatalogLifecycle::Reserved,
                ..entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::title",
                    "member-title-old",
                    &[],
                )
            },
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    let expected_payload = DiagnosticPayload::ReservedCatalogPathReuse {
        source_kind: CatalogEntryKind::ResourceMember,
        source_path: "books::Book::title".to_string(),
        reserved_stable_id: derived_id("member-title-old"),
    };
    assert!(
        report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == CHECK_CATALOG_INTENT && diagnostic.payload == expected_payload
        }),
        "reserved path reuse must carry exact payload: {:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    let title_entries: Vec<_> = proposal
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::title"
        })
        .collect();
    assert_eq!(1, title_entries.len(), "{:#?}", proposal.entries);
    assert_eq!(CatalogLifecycle::Reserved, title_entries[0].lifecycle);
}

#[test]
fn retire_reserves_the_path_spelling_against_future_reuse() {
    let root = temp_project("catalog-retire-reserves-path", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire Book.title\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program.catalog.proposal.expect("proposal");
    let title = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::title"
        })
        .unwrap_or_else(|| panic!("proposal keeps retired title: {:#?}", proposal.entries));
    assert_eq!(CatalogLifecycle::Reserved, title.lifecycle);
    assert_eq!(derived_id("member-title"), title.stable_id);
}

#[test]
fn catalog_proposal_ids_do_not_collide_with_accepted_stable_ids() {
    let colliding_id = "cat_00000000000000000f32222e2032f199";
    let root = temp_project("catalog-proposal-id-collision", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            literal_entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                colliding_id,
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "{:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty().expect("catalog renders"))
        .expect("proposal validates");
}

#[test]
fn source_rename_without_accepted_catalog_intent_fails_closed() {
    let root = temp_project("catalog-rename-reject", |root| {
        write(
            root,
            "src/library.mw",
            "module library\nresource Book\n    title: string\nstore ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn accepted_catalog_alias_does_not_authorize_source_rollback() {
    let root = temp_project("catalog-rollback-reject", |root| {
        write(
            root,
            "src/books.mw",
            "module books\nresource Book\n    title: string\nstore ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            entry(
                CatalogEntryKind::Resource,
                "library::Book",
                "res-book",
                &["books::Book"],
            ),
            entry(
                CatalogEntryKind::Store,
                "library::^books",
                "store-books",
                &["books::^books"],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "library::Book::title",
                "member-title",
                &["books::Book::title"],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "{:#?}",
        report.diagnostics
    );
    let module = program.facts.module_id("books").expect("module");
    let resource = program.facts.resource_id(module, "Book").expect("resource");
    assert_eq!(program.facts.resource(resource).catalog_id, None);
}

#[test]
fn accepted_catalog_rename_preserves_stable_id() {
    let root = temp_project("catalog-rename-preserve", |root| {
        write(
            root,
            "src/library.mw",
            "module library\nresource Book\n    title: string\nstore ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            entry(
                CatalogEntryKind::Resource,
                "library::Book",
                "res-book",
                &["books::Book"],
            ),
            entry(
                CatalogEntryKind::Store,
                "library::^books",
                "store-books",
                &["books::^books"],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "library::Book::title",
                "member-title",
                &["books::Book::title"],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("library").expect("module");
    let resource = program.facts.resource_id(module, "Book").expect("resource");
    assert_eq!(
        program.facts.resource(resource).catalog_id.as_deref(),
        Some(derived_id("res-book").as_str())
    );
}

#[test]
fn catalog_proposals_preserve_accepted_aliases_and_lifecycle() {
    let root = temp_project("catalog-proposal-preserve", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            entry(
                CatalogEntryKind::Resource,
                "books::Book",
                "res-book",
                &["library::Book"],
            ),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "{:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    let resource = proposal
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::Resource && entry.path == "books::Book")
        .expect("resource proposal");
    assert_eq!(resource.aliases, ["library::Book"]);
}
