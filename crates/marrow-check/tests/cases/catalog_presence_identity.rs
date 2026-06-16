use crate::support;
use std::path::Path;

use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogMetadata};
use marrow_check::{CHECK_CATALOG_INTENT, check_project_with_catalog};

use support::catalog::{catalog, derived_id, entry as literal_entry, write_catalog};
use support::{check_with_accepted, config, temp_project, write};

/// The baseline accepted catalog a state-establishing flow freezes for the project under
/// `root`: the proposal a first-run check produces. A real run commits this snapshot in
/// the store transaction, then renders `marrow.catalog.json`; this checker test reads it
/// straight off the proposal and passes it as caller-supplied analysis input.
fn commit_baseline(root: &Path) -> CatalogMetadata {
    let (report, program) =
        check_project_with_catalog(root, &config(), None).expect("first-run check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
        .catalog
        .proposal
        .expect("a first-run check proposes a baseline catalog")
}

/// A catalog entry whose stable id is minted deterministically from `label`, so a
/// fixture refers to a member by a readable name and still gets a `cat_`-shaped id the
/// catalog parser accepts.
fn entry(
    kind: CatalogEntryKind,
    canonical_path: &str,
    label: &str,
    aliases: &[&str],
) -> CatalogEntry {
    literal_entry(kind, canonical_path, &derived_id(label), aliases)
}

fn proposed_id_for_path(proposal: &CatalogMetadata, kind: CatalogEntryKind, path: &str) -> String {
    proposal
        .entries
        .iter()
        .find(|entry| entry.kind == kind && entry.path == path)
        .unwrap_or_else(|| panic!("proposal has an entry for {path}: {:#?}", proposal.entries))
        .stable_id
        .clone()
}

#[test]
fn proposed_ids_use_128_bit_random_shape() {
    let root = temp_project("catalog-id-128-bit", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n",
        );
    });

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program.catalog.proposal.expect("proposal");
    for entry in &proposal.entries {
        let Some(hex) = entry.stable_id.strip_prefix("cat_") else {
            panic!("stable id must use cat_ prefix: {}", entry.stable_id);
        };
        assert_eq!(
            32,
            hex.len(),
            "catalog stable ids must carry 128 random bits: {}",
            entry.stable_id
        );
        assert!(
            hex.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "catalog stable ids must be lowercase hex: {}",
            entry.stable_id
        );
    }
}

#[test]
fn caller_supplied_catalog_rejects_store_index_without_shape() {
    let root = temp_project("catalog-index-shape-required", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             \x20   index byTitle(title) unique\n",
        );
    });
    let accepted = catalog(vec![
        entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
        entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
        entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::title",
            "member-title",
            &[],
        ),
        entry(
            CatalogEntryKind::StoreIndex,
            "books::^books::byTitle",
            "index-by-title",
            &[],
        ),
    ]);

    let (report, _program) =
        check_project_with_catalog(&root, &config(), Some(&accepted)).expect("check");

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
fn evolve_rename_reads_the_stored_id_rather_than_recomputing_it() {
    // A rename carries the accepted entry's id onto the new path unchanged; the id
    // is read from storage, never re-derived from the new (or old) path.
    let root = temp_project("catalog-id-stable-across-rename", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.title -> Book.subtitle\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            literal_entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "cat_000000000000000000000000000000ff",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "{:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    let renamed = proposed_id_for_path(
        &proposal,
        CatalogEntryKind::ResourceMember,
        "books::Book::subtitle",
    );
    assert_eq!(
        renamed, "cat_000000000000000000000000000000ff",
        "the rename must keep the stored id, not derive a new one for the new path"
    );
}

#[test]
fn committed_ids_are_stable_across_rechecks() {
    // Random allocation is not reproducible, but a committed baseline is: once the
    // ids are frozen in the accepted catalog, re-checking unchanged source reads them
    // back unchanged rather than minting fresh ones.
    let root = temp_project("catalog-id-stable-after-commit", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n",
        );
    });

    // The first-run proposal is the baseline a state-establishing flow freezes. Its
    // Book id is the committed identity, so re-checking against it must bind that exact
    // id rather than minting a fresh one.
    let baseline = commit_baseline(&root);
    let frozen = baseline
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::Resource && entry.path == "books::Book")
        .expect("committed Book entry")
        .stable_id
        .clone();

    let (recheck, program) =
        check_project_with_catalog(&root, &config(), Some(&baseline)).expect("re-check");
    assert!(!recheck.has_errors(), "{:#?}", recheck.diagnostics);

    let module = program.facts.module_id("books").expect("books module");
    let resource = program.facts.resource_id(module, "Book").expect("Book");
    let bound = program.facts.resource(resource).catalog_id.clone();
    assert_eq!(
        bound.as_deref(),
        Some(frozen.as_str()),
        "a committed id must survive a re-check unchanged"
    );
}

#[test]
fn committed_leaf_member_records_its_token_in_the_structural_signature_only() {
    // A leaf member's accepted leaf token is the one durable structural signature with its
    // `leaf:` prefix, never a second persisted field: the committed catalog records the token
    // only in `accepted_struct`, the serialized JSON carries no `acceptedLeaf` key, and the
    // token reads back off the signature for retype detection.
    let root = temp_project("catalog-leaf-token-from-struct", |root| {
        write(
            root,
            "src/books.mw",
            "module books\nresource Book\n    title: string\nstore ^books(id: int): Book\n",
        );
    });

    let accepted = commit_baseline(&root);

    let json = accepted.to_json_pretty().expect("catalog renders");
    assert!(
        !json.contains("acceptedLeaf"),
        "the committed catalog must not persist a redundant acceptedLeaf field: {json}"
    );

    let title = accepted
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::title"
        })
        .expect("committed title member entry");
    assert_eq!(
        title.accepted_struct.as_deref(),
        Some("leaf:string"),
        "the leaf member's signature records its leaf token"
    );
    assert_eq!(
        title.accepted_leaf_token(),
        Some("string"),
        "the accepted leaf token is derived from the signature"
    );
}

#[test]
fn committed_keyed_leaf_member_folds_its_key_shape_into_the_signature() {
    // A keyed leaf is a leaf position whose accepted signature folds its key
    // shape onto the value type, so a change to its key arity, key type, or value type is caught
    // as one leaf type change. The committed baseline records `leaf:[<key>]<value>`, proving the
    // production mint carries the key prefix, not only a hand-built fixture.
    let root = temp_project("catalog-keyed-leaf-token", |root| {
        write(
            root,
            "src/books.mw",
            "module books\nresource Book\n    tags(pos: int): string\nstore ^books(id: int): Book\n",
        );
    });

    let accepted = commit_baseline(&root);
    let tags = accepted
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::tags"
        })
        .expect("committed tags member entry");
    assert_eq!(
        tags.accepted_struct.as_deref(),
        Some("leaf:[int]string"),
        "a keyed leaf folds its [key] shape onto the value in its leaf signature"
    );
}

#[test]
fn member_accepted_before_structural_signatures_were_recorded_is_not_reported_as_changed() {
    // The accepted catalog predates structural-signature recording, so its member entry carries
    // no signature and thus no accepted leaf token. Re-checking unchanged source must not read
    // filling that signature in as a type change: the member's stored bytes are unchanged, so no
    // proposal is emitted.
    let root = temp_project("catalog-struct-backfill-no-change", |root| {
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

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        program.catalog.proposal.is_none(),
        "filling a missing accepted leaf for unchanged source must not propose a change: {:#?}",
        program.catalog.proposal
    );
}

#[test]
fn distinct_new_members_receive_distinct_ids() {
    // Two new members allocated against an empty catalog must not collide on one id.
    let root = temp_project("catalog-id-distinct", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n",
        );
    });

    let (report, program) = check_with_accepted(&root);
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let proposal = program.catalog.proposal.expect("proposal");
    let title = proposed_id_for_path(
        &proposal,
        CatalogEntryKind::ResourceMember,
        "books::Book::title",
    );
    let subtitle = proposed_id_for_path(
        &proposal,
        CatalogEntryKind::ResourceMember,
        "books::Book::subtitle",
    );
    assert_ne!(
        title, subtitle,
        "two distinct members must not share a stable id"
    );
}
