mod support;

use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use marrow_check::{
    CHECK_BARE_MAYBE_PRESENT_READ, CHECK_CATALOG_INTENT, PresenceProofPlace, PresenceProofRead,
    PresenceProofSource, PresenceProofStatus, StoreIndexKeySource, StoredValueMeaning,
    check_project,
};
use marrow_project::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};

use support::{config, temp_project, write};

fn catalog_path(root: &Path) -> PathBuf {
    root.join("marrow.catalog.json")
}

fn catalog(entries: Vec<CatalogEntry>) -> CatalogMetadata {
    CatalogMetadata::new(7, entries)
}

fn entry(
    kind: CatalogEntryKind,
    canonical_path: &str,
    stable_id: &str,
    aliases: &[&str],
) -> CatalogEntry {
    CatalogEntry {
        kind,
        path: canonical_path.to_string(),
        stable_id: fixture_id(stable_id),
        aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
        lifecycle: CatalogLifecycle::Active,
        accepted_key_shape: None,
        accepted_struct: None,
    }
}

fn fixture_id(label: &str) -> String {
    if label.starts_with("cat_") {
        return label.to_string();
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    label.hash(&mut hasher);
    let first = hasher.finish();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (label, "catalog-presence-fixture").hash(&mut hasher);
    let second = hasher.finish();
    format!("cat_{first:016x}{second:016x}")
}

fn write_catalog(root: &Path, metadata: &CatalogMetadata) {
    fs::write(catalog_path(root), metadata.to_json_pretty()).expect("write catalog");
}

#[test]
fn first_source_check_proposes_catalog_ids_without_writing_accepted_catalog() {
    let root = temp_project("catalog-proposal", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
             \x20   index byTitle(title) unique\n\
             enum Status\n\
             \x20   active\n\
             \x20   archived\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");
    let accepted_path = catalog_path(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        !accepted_path.exists(),
        "source-only check must not generate the accepted catalog file"
    );
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

/// The accepted catalog is the durable ABI: a torn write would brick the project, so
/// `write_accepted_catalog` must be all-or-nothing. After a successful write the only
/// catalog artifact in the directory is the complete target file — no temp staging file
/// is left behind, and overwriting a prior catalog never exposes a partial file. The
/// written bytes round-trip back to the same metadata.
#[test]
fn write_accepted_catalog_is_atomic_and_leaves_no_temp() {
    let root = temp_project("catalog-atomic", |_| {});

    // A prior, smaller catalog sits at the target. The overwrite with a larger catalog
    // must replace it wholesale, never leaving a half-written file or a stray temp.
    let prior = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    write_catalog(&root, &prior);

    let next = catalog(vec![
        entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
        entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
        entry(
            CatalogEntryKind::ResourceMember,
            "books::Book.title",
            "member-title",
            &[],
        ),
    ]);
    marrow_check::write_accepted_catalog(&root, &config(), &next).expect("write accepted catalog");

    let entries: Vec<String> = fs::read_dir(&*root)
        .expect("read project root")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into()
        })
        .collect();
    assert_eq!(
        entries,
        vec![String::from("marrow.catalog.json")],
        "a successful write leaves only the target file, with no temp staging artifact"
    );

    let bytes = fs::read_to_string(catalog_path(&root)).expect("read catalog");
    let round_tripped = CatalogMetadata::from_json(&bytes).expect("complete, parseable catalog");
    assert_eq!(
        round_tripped, next,
        "the written file is the complete catalog, not a truncated prefix"
    );
}

#[test]
fn source_only_check_leaves_accepted_catalog_epoch_unchanged() {
    let root = temp_project("catalog-epoch", |root| {
        write(
            root,
            "src/books.mw",
            "module books\nresource Book at ^books(id: int)\n    title: string\n",
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
    let before = fs::read_to_string(catalog_path(&root)).expect("read before");

    let (report, program) = check_project(&root, &config()).expect("check");
    let after = fs::read_to_string(catalog_path(&root)).expect("read after");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert_eq!(program.catalog.accepted_epoch, Some(7));
    assert_eq!(before, after);
}

#[test]
fn accepted_catalog_rejects_alias_and_stable_id_collisions() {
    for metadata in [
        catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(
                CatalogEntryKind::Resource,
                "library::Book",
                "res-library",
                &["books::Book"],
            ),
        ]),
        catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "res-book", &[]),
        ]),
    ] {
        let json = metadata.to_json_pretty();
        let error = CatalogMetadata::from_json(&json).expect_err("collision is rejected");

        assert_eq!(error.code, marrow_project::CATALOG_INVALID);
    }
}

#[test]
fn accepted_catalog_round_trips_stable_ids_aliases_lifecycle_epoch_and_digest() {
    let metadata = catalog(vec![
        entry(
            CatalogEntryKind::Resource,
            "books::Book",
            "res-book",
            &["library::Book"],
        ),
        CatalogEntry {
            kind: CatalogEntryKind::EnumMember,
            path: "books::Status::archived".to_string(),
            stable_id: fixture_id("enum-member-archived"),
            aliases: vec!["books::Status::inactive".to_string()],
            lifecycle: CatalogLifecycle::Deprecated,
            accepted_key_shape: None,
            accepted_struct: None,
        },
    ]);

    let json = metadata.to_json_pretty();
    let parsed = CatalogMetadata::from_json(&json).expect("catalog parses");

    assert_eq!(parsed.epoch, 7);
    assert!(
        parsed.digest.starts_with("sha256:"),
        "catalog digest must be collision-resistant: {}",
        parsed.digest
    );
    assert_eq!("sha256:".len() + 64, parsed.digest.len());
    assert_eq!(parsed.digest, metadata.digest);
    assert_eq!(parsed.entries, metadata.entries);
}

#[test]
fn accepted_catalog_round_trips_reserved_lifecycle() {
    let metadata = catalog(vec![CatalogEntry {
        lifecycle: CatalogLifecycle::Reserved,
        ..entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::oldTitle",
            "member-old-title",
            &[],
        )
    }]);

    let json = metadata.to_json_pretty();
    assert!(json.contains("\"lifecycle\": \"reserved\""), "{json}");
    let parsed = CatalogMetadata::from_json(&json).expect("catalog parses");

    assert_eq!(parsed.entries[0].lifecycle, CatalogLifecycle::Reserved);
}

#[test]
fn non_sha_catalog_digest_is_rejected() {
    let metadata = catalog(vec![entry(
        CatalogEntryKind::Resource,
        "books::Book",
        "res-book",
        &[],
    )]);
    let json = metadata.to_json_pretty().replacen(
        &format!("\"digest\": \"{}\"", metadata.digest),
        "\"digest\": \"weak:0000000000000000\"",
        1,
    );

    let error = CatalogMetadata::from_json(&json).expect_err("non-SHA digest rejected");

    assert_eq!(error.code, marrow_project::CATALOG_INVALID);
}

#[test]
fn removed_catalog_lifecycle_is_rejected() {
    let json = r#"{
  "epoch": 7,
  "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
  "entries": [
    {
      "kind": "resource",
      "path": "books::Book",
      "stableId": "cat_00000000000000000000000000000001",
      "aliases": [],
      "lifecycle": "removed",
      "acceptedKeyShape": null,
      "acceptedStruct": null
    }
  ]
}"#;

    let error = CatalogMetadata::from_json(json).expect_err("removed lifecycle rejected");

    assert_eq!(error.code, marrow_project::CATALOG_INVALID);
    assert!(
        error.message.contains("removed"),
        "wrong rejection: {error:?}"
    );
}

#[test]
fn non_active_catalog_entries_and_aliases_do_not_bind_live_source_facts() {
    let root = temp_project("catalog-non-active", |root| {
        write(
            root,
            "src/library.mw",
            "module library\nresource Book at ^books(id: int)\n    title: string\n",
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

    let (report, program) = check_project(&root, &config()).expect("check");

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
    CatalogMetadata::from_json(&proposal.to_json_pretty()).expect("proposal validates");
}

#[test]
fn reserved_catalog_path_blocks_source_reuse_without_intent() {
    let root = temp_project("catalog-reserved-path", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n",
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

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT
                && diagnostic.message.contains("reserved")),
        "reserved path reuse must be diagnosed: {:#?}",
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
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
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

    let (report, program) = check_project(&root, &config()).expect("check");

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
    assert_eq!(fixture_id("member-title"), title.stable_id);
}

#[test]
fn catalog_proposal_ids_do_not_collide_with_accepted_stable_ids() {
    let colliding_id = "cat_00000000000000000f32222e2032f199";
    let root = temp_project("catalog-proposal-id-collision", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             \x20   subtitle: string\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                colliding_id,
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "{:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty()).expect("proposal validates");
    assert!(
        proposal
            .entries
            .iter()
            .filter(|entry| entry.stable_id == colliding_id)
            .count()
            == 1,
        "{:#?}",
        proposal.entries
    );
}

#[test]
fn source_rename_without_accepted_catalog_intent_fails_closed() {
    let root = temp_project("catalog-rename-reject", |root| {
        write(
            root,
            "src/library.mw",
            "module library\nresource Book at ^books(id: int)\n    title: string\n",
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

    let (report, _program) = check_project(&root, &config()).expect("check");

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
            "module books\nresource Book at ^books(id: int)\n    title: string\n",
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

    let (report, program) = check_project(&root, &config()).expect("check");

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
            "module library\nresource Book at ^books(id: int)\n    title: string\n",
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

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("library").expect("module");
    let resource = program.facts.resource_id(module, "Book").expect("resource");
    assert_eq!(
        program.facts.resource(resource).catalog_id.as_deref(),
        Some(fixture_id("res-book").as_str())
    );
}

#[test]
fn catalog_proposals_preserve_accepted_aliases_and_lifecycle() {
    let root = temp_project("catalog-proposal-preserve", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             \x20   subtitle: string\n",
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
            CatalogEntry {
                kind: CatalogEntryKind::Enum,
                path: "books::OldStatus".to_string(),
                stable_id: fixture_id("enum-old-status"),
                aliases: vec!["books::Status".to_string()],
                lifecycle: CatalogLifecycle::Deprecated,
                accepted_key_shape: None,
                accepted_struct: None,
            },
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

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
    let deprecated = proposal
        .entries
        .iter()
        .find(|entry| entry.stable_id == fixture_id("enum-old-status"))
        .expect("deprecated entry");
    assert_eq!(deprecated.lifecycle, CatalogLifecycle::Deprecated);
    assert_eq!(deprecated.aliases, ["books::Status"]);
}

#[test]
fn enum_member_facts_use_catalog_ids_independent_of_source_order() {
    let root = temp_project("catalog-enum-order", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   archived\n\
             \x20   active\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "books::Status", "enum-status", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::active",
                "enum-member-active",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::archived",
                "enum-member-archived",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("books").expect("module");
    let status = program.facts.enum_id(module, "Status").expect("enum");
    let active = program
        .facts
        .enum_members()
        .iter()
        .find(|member| member.enum_id == status && member.name == "active")
        .expect("active");
    let archived = program
        .facts
        .enum_members()
        .iter()
        .find(|member| member.enum_id == status && member.name == "archived")
        .expect("archived");
    assert_eq!(
        active.catalog_id.as_deref(),
        Some(fixture_id("enum-member-active").as_str())
    );
    assert_eq!(
        archived.catalog_id.as_deref(),
        Some(fixture_id("enum-member-archived").as_str())
    );
}

#[test]
fn enum_field_value_meaning_uses_catalog_member_identity_after_source_reorder() {
    let root = temp_project("catalog-enum-value-meaning", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   archived\n\
             \x20   active\n\
             resource Order at ^orders(id: int)\n\
             \x20   state: Status\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "books::Status", "enum-status", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::active",
                "enum-member-active",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::archived",
                "enum-member-archived",
                &[],
            ),
            entry(CatalogEntryKind::Resource, "books::Order", "res-order", &[]),
            entry(
                CatalogEntryKind::Store,
                "books::^orders",
                "store-orders",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Order::state",
                "member-state",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("books").expect("module");
    let status = program.facts.enum_id(module, "Status").expect("enum");
    let order = program
        .facts
        .resource_id(module, "Order")
        .expect("resource");
    let state = program
        .facts
        .resource_members()
        .iter()
        .find(|member| member.resource == order && member.name == "state")
        .expect("state member");

    let Some(StoredValueMeaning::Enum { enum_id, members }) = &state.value_meaning else {
        panic!("state should store by enum member identity: {state:#?}");
    };
    assert_eq!(*enum_id, status);
    let catalog_ids = sorted_enum_member_catalog_ids(&program.facts, members);
    assert_eq!(
        catalog_ids,
        [
            fixture_id("enum-member-active"),
            fixture_id("enum-member-archived")
        ]
    );
}

#[test]
fn enum_index_key_meaning_uses_catalog_member_identity_after_source_reorder() {
    let root = temp_project("catalog-enum-index-meaning", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   archived\n\
             \x20   active\n\
             resource Order at ^orders(id: int)\n\
             \x20   state: Status\n\
             \x20   index byState(state, id)\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "books::Status", "enum-status", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::active",
                "enum-member-active",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Status::archived",
                "enum-member-archived",
                &[],
            ),
            entry(CatalogEntryKind::Resource, "books::Order", "res-order", &[]),
            entry(
                CatalogEntryKind::Store,
                "books::^orders",
                "store-orders",
                &[],
            ),
            entry(
                CatalogEntryKind::StoreIndex,
                "books::^orders::byState",
                "index-by-state",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Order::state",
                "member-state",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("books").expect("module");
    let status = program.facts.enum_id(module, "Status").expect("enum");
    let order = program
        .facts
        .resource_id(module, "Order")
        .expect("resource");
    let state = program
        .facts
        .resource_members()
        .iter()
        .find(|member| member.resource == order && member.name == "state")
        .expect("state member");
    let index = program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.name == "byState")
        .expect("byState index");
    let key = index
        .keys
        .iter()
        .find(|key| key.name == "state")
        .expect("state key");

    assert_eq!(key.source, StoreIndexKeySource::ResourceMember(state.id));
    let StoredValueMeaning::Enum { enum_id, members } = &key.value_meaning else {
        panic!("state key should store by enum member identity: {key:#?}");
    };
    assert_eq!(*enum_id, status);
    let catalog_ids = sorted_enum_member_catalog_ids(&program.facts, members);
    assert_eq!(
        catalog_ids,
        [
            fixture_id("enum-member-active"),
            fixture_id("enum-member-archived")
        ]
    );
}

#[test]
fn enum_field_value_meaning_fails_closed_for_unresolved_bare_enum_names() {
    let root = temp_project("catalog-enum-value-meaning-fail-closed", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             enum Status\n\
             \x20   active\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Order at ^orders(id: int)\n\
             \x20   state: Status\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "a::Status", "enum-status", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "a::Status::active",
                "enum-member-active",
                &[],
            ),
            entry(CatalogEntryKind::Resource, "b::Order", "res-order", &[]),
            entry(CatalogEntryKind::Store, "b::^orders", "store-orders", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "b::Order::state",
                "member-state",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (_report, program) = check_project(&root, &config()).expect("check");

    let module = program.facts.module_id("b").expect("module");
    let order = program
        .facts
        .resource_id(module, "Order")
        .expect("resource");
    let state = program
        .facts
        .resource_members()
        .iter()
        .find(|member| member.resource == order && member.name == "state")
        .expect("state member");

    assert_eq!(state.value_meaning, None, "{state:#?}");
}

fn sorted_enum_member_catalog_ids(
    facts: &marrow_check::CheckedFacts,
    members: &[marrow_check::EnumMemberId],
) -> Vec<String> {
    let mut ids: Vec<String> = members
        .iter()
        .map(|id| {
            facts
                .enum_members()
                .iter()
                .find(|member| member.id == *id)
                .expect("enum member")
                .catalog_id
                .clone()
                .expect("accepted enum member catalog id")
        })
        .collect();
    ids.sort();
    ids
}

#[test]
fn coalesce_rejects_non_saved_function_calls_outside_the_presence_ledger() {
    let root = temp_project("presence-coalesce-call", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             fn value(): string\n\
             \x20   return \"title\"\n\
             fn fallback(): string\n\
             \x20   return value() ?? \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_OPERATOR_TYPE),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrows_reads_inside_the_then_block() {
    let root = temp_project("presence-if-exists", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        program
            .facts
            .presence_proofs()
            .iter()
            .any(|proof| proof.source == PresenceProofSource::Narrowing),
        "{:#?}",
        program.facts.presence_proofs()
    );
}

#[test]
fn if_exists_narrowing_is_key_sensitive() {
    let root = temp_project("presence-if-exists-keyed", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(a: int, b: int): string\n\
             \x20   if exists(^books(a).subtitle)\n\
             \x20       return ^books(b).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_is_binding_sensitive() {
    let root = temp_project("presence-if-exists-shadowed-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       const id: int = 2\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_binding_is_assigned() {
    let root = temp_project("presence-if-exists-mutated-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       k = 2\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_binding_is_passed_inout() {
    let root = temp_project("presence-if-exists-inout-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(inout value: int)\n\
             \x20   value = 2\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       setTo(inout k)\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_field_is_assigned() {
    let root = temp_project("presence-if-exists-mutated-key-field", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Holder\n\
             \x20   required id: int\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(id: int): string\n\
             \x20   var holder = Holder(id: id)\n\
             \x20   if exists(^books(holder.id).subtitle)\n\
             \x20       holder.id = 2\n\
             \x20       return ^books(holder.id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_field_is_passed_inout() {
    let root = temp_project("presence-if-exists-inout-key-field", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Holder\n\
             \x20   required id: int\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(inout value: int)\n\
             \x20   value = 2\n\
             fn guarded(id: int): string\n\
             \x20   var holder = Holder(id: id)\n\
             \x20   if exists(^books(holder.id).subtitle)\n\
             \x20       setTo(inout holder.id)\n\
             \x20       return ^books(holder.id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_nested_condition_mutates_key() {
    let root = temp_project("presence-if-exists-nested-condition-mutates-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(inout value: int): bool\n\
             \x20   value = 2\n\
             \x20   return true\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       if setTo(inout k)\n\
             \x20           return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_ignores_condition_proofs_after_key_mutation() {
    let root = temp_project("presence-if-exists-condition-mutates-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(inout value: int): bool\n\
             \x20   value = 2\n\
             \x20   return true\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle) and setTo(inout k)\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_saved_field_is_deleted() {
    let root = temp_project("presence-if-exists-delete-field", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       delete ^books(id).subtitle\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_saved_root_is_replaced() {
    let root = temp_project("presence-if-exists-replace-root", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       ^books(id) = Book(title: \"new\")\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_called_function_writes_saved_data() {
    let root = temp_project("presence-if-exists-call-writes-saved", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn dropSubtitle(id: int)\n\
             \x20   delete ^books(id).subtitle\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       dropSubtitle(id)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_called_function_transitively_writes_saved_data() {
    let root = temp_project("presence-if-exists-call-transitive-writes-saved", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn dropSubtitle(id: int)\n\
             \x20   delete ^books(id).subtitle\n\
             fn relay(id: int)\n\
             \x20   dropSubtitle(id)\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       relay(id)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_only_child_of_parent_is_deleted() {
    let root = temp_project("presence-if-exists-delete-only-child", |root| {
        write(
            root,
            "src/items.mw",
            "module items\n\
             resource Item at ^items(id: int)\n\
             \x20   note: string\n\
             fn stale(id: int): Item\n\
             \x20   if exists(^items(id))\n\
             \x20       delete ^items(id).note\n\
             \x20       return ^items(id)\n\
             \x20   return Item()\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn unique_index_coalesce_records_presence_proof() {
    let root = temp_project("presence-index-coalesce", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \n\
             \x20   index byIsbn(isbn) unique\n\
             \n\
             fn lookup(isbn: string, fallback: Id(^books)): Id(^books)\n\
             \x20   return ^books.byIsbn(isbn) ?? fallback\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_OPERATOR_TYPE),
        "{:#?}",
        report.diagnostics
    );
    let proof = program
        .facts
        .presence_proofs()
        .iter()
        .find(|proof| proof.source == PresenceProofSource::Narrowing)
        .expect("narrowing proof");
    assert!(
        matches!(proof.place, PresenceProofPlace::StoreIndex(_)),
        "{:#?}",
        program.facts.presence_proofs()
    );
    assert_eq!(proof.read, PresenceProofRead::Direct);
    assert_eq!(proof.keys.len(), 1);
}

#[test]
fn next_coalesce_records_read_site_resolution() {
    let root = temp_project("presence-next-coalesce", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
             fn nextPos(id: int, pos: int): int\n\
             \x20   return next(^books(id).tags(pos)) ?? -1\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proof_sources: Vec<_> = program
        .facts
        .presence_proofs()
        .iter()
        .map(|proof| proof.source)
        .collect();
    assert!(
        proof_sources.contains(&PresenceProofSource::Narrowing),
        "{proof_sources:#?}"
    );
    let next_proof = program
        .facts
        .presence_proofs()
        .iter()
        .find(|proof| proof.read == PresenceProofRead::Next)
        .expect("next proof");
    assert!(matches!(next_proof.place, PresenceProofPlace::Saved(_)));
    assert_eq!(next_proof.keys.len(), 3);
    assert!(
        !proof_sources.contains(&PresenceProofSource::AttachedData),
        "{proof_sources:#?}"
    );
}

#[test]
fn for_loop_over_saved_layer_narrows_iterated_entry_reads() {
    let root = temp_project("presence-loop-narrowing", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   tags(pos: int): string\n\
             fn f()\n\
             \x20   for pos in ^books(1).tags\n\
             \x20   \x20   write(^books(1).tags(pos))\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        program
            .facts
            .presence_proofs()
            .iter()
            .any(|proof| proof.source == PresenceProofSource::Narrowing),
        "{:#?}",
        program.facts.presence_proofs()
    );
}

#[test]
fn unknown_cannot_reenter_a_saved_identity_keyspace() {
    let root = temp_project("identity-unknown-keyspace", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn save(raw: unknown)\n\
             \x20   ^books(raw).title = \"bad\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_KEY_TYPE),
        "unknown must not act as any for saved identity keys: {:#?}",
        report.diagnostics
    );
}

#[test]
fn values_loop_does_not_narrow_value_as_an_entry_key() {
    let root = temp_project("presence-values-loop-not-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for score in values(^books(1).scores)\n\
             \x20   \x20   write(^books(1).scores(score))\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn single_binding_entries_loop_does_not_narrow_entry_as_a_key() {
    let root = temp_project("presence-single-entry-loop-not-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for entry in entries(^books(1).scores)\n\
             \x20   \x20   write(^books(1).scores(entry))\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn two_binding_keys_loop_does_not_narrow_ordinal_as_a_key() {
    let root = temp_project("presence-two-binding-keys-loop-not-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for ordinal, pos in keys(^books(1).scores)\n\
             \x20   \x20   write(^books(1).scores(ordinal))\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn two_binding_reversed_keys_loop_does_not_narrow_ordinal_as_a_key() {
    let root = temp_project("presence-two-binding-reversed-keys-loop-not-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for ordinal, pos in reversed(keys(^books(1).scores))\n\
             \x20   \x20   write(^books(1).scores(ordinal))\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn two_binding_saved_path_loop_narrows_the_key_binding() {
    let root = temp_project("presence-two-binding-saved-path-loop-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for pos, score in ^books(1).scores\n\
             \x20   \x20   write(^books(1).scores(pos))\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn duplicate_entries_loop_bindings_do_not_narrow_the_visible_value_as_a_key() {
    let root = temp_project("presence-duplicate-entries-loop-bindings-not-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for x, x in entries(^books(1).scores)\n\
             \x20   \x20   write(^books(1).scores(x))\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrowing_expires_when_same_condition_calls_saved_writer() {
    let root = temp_project("presence-if-exists-condition-call-writes-saved", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn dropSubtitle(id: int): bool\n\
             \x20   delete ^books(id).subtitle\n\
             \x20   return true\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle) and dropSubtitle(id)\n\
             \x20   \x20   return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn bare_maybe_present_read_errors_and_resolved_reads_record_allowed_proof_sources() {
    let root = temp_project("presence-ledger", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             fn requiredTitle(id: int): string\n\
             \x20   return ^books(id).title\n\
             fn bare(id: int): string\n\
             \x20   return ^books(id).subtitle\n\
             fn fallback(id: int): string\n\
             \x20   return ^books(id).subtitle ?? \"untitled\"\n\
             fn found(id: int): bool\n\
             \x20   return exists(^books(id).subtitle)\n\
             fn optional(id: int): string\n\
             \x20   return ^books(id)?.subtitle ?? \"untitled\"\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
    let proof_sources: Vec<_> = program
        .facts
        .presence_proofs()
        .iter()
        .map(|proof| proof.source)
        .collect();
    assert!(
        proof_sources.contains(&PresenceProofSource::AttachedData),
        "{proof_sources:#?}"
    );
    assert!(
        proof_sources.contains(&PresenceProofSource::Declaration),
        "{proof_sources:#?}"
    );
    assert!(
        proof_sources.contains(&PresenceProofSource::Narrowing),
        "{proof_sources:#?}"
    );
    assert!(
        program
            .facts
            .presence_proofs()
            .iter()
            .any(|proof| proof.status == PresenceProofStatus::PendingAttachedData),
        "{:#?}",
        program.facts.presence_proofs()
    );
    assert!(
        program
            .facts
            .presence_proofs()
            .iter()
            .any(|proof| proof.status == PresenceProofStatus::Discharged),
        "{:#?}",
        program.facts.presence_proofs()
    );
    let mut proof_ids: Vec<_> = program
        .facts
        .presence_proofs()
        .iter()
        .map(|proof| proof.id)
        .collect();
    proof_ids.sort_by_key(|id| id.0);
    proof_ids.dedup();
    assert_eq!(
        proof_ids.len(),
        program.facts.presence_proofs().len(),
        "presence proof ids must be unique"
    );
    for proof in program.facts.presence_proofs() {
        match proof.source {
            PresenceProofSource::Declaration
            | PresenceProofSource::Narrowing
            | PresenceProofSource::AttachedData => {}
        }
    }
}

#[test]
fn evolve_rename_authorizes_a_saved_data_backed_member_rename() {
    let root = temp_project("evolve-rename-member", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             evolve\n\
             \x20   rename Book.title -> Book.subtitle\n",
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

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "evolve rename intent must satisfy the catalog binding: {:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty()).expect("proposal validates");
    let renamed = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::subtitle"
        })
        .expect("renamed member entry");
    assert_eq!(renamed.stable_id, fixture_id("member-title"));
    assert_eq!(renamed.lifecycle, CatalogLifecycle::Active);
    assert!(
        renamed
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::title"),
        "old path must be recorded as an alias: {renamed:#?}"
    );
    // No stale entry remains at the old member path.
    assert!(
        !proposal
            .entries
            .iter()
            .any(|entry| entry.path == "books::Book::title"),
        "the old path must not linger as a separate entry: {:#?}",
        proposal.entries
    );
}

#[test]
fn source_member_rename_without_evolve_intent_still_fails_closed() {
    let root = temp_project("evolve-rename-member-no-intent", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n",
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

    let (report, _program) = check_project(&root, &config()).expect("check");

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
fn evolve_retire_marks_the_proposal_entry_reserved() {
    let root = temp_project("evolve-retire", |root| {
        // The source has dropped `subtitle`; the accepted catalog still records it.
        // `retire` declares the destructive intent while reserving the old spelling.
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             evolve\n\
             \x20   retire Book.subtitle\n",
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
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::subtitle",
                "member-subtitle",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program.catalog.proposal.expect("proposal");
    let retired = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember
                && entry.stable_id == fixture_id("member-subtitle")
        })
        .expect("retired member entry");
    assert_eq!(retired.lifecycle, CatalogLifecycle::Reserved);
}

#[test]
fn evolve_retire_of_a_still_declared_resource_member_fails_closed() {
    // The source still declares `Book.title` while `retire` names it. A retire is a
    // destructive drop of data the running program still reads and writes, so it
    // must be rejected until the source declaration is actually gone; the proposal
    // entry must stay Active rather than be silently reserved.
    let root = temp_project("evolve-retire-member-still-declared", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
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

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "retiring a still-declared resource member must fail closed: {:#?}",
        report.diagnostics
    );
    assert_entry_stays_active(&program, "member-title");
}

#[test]
fn evolve_retire_of_a_still_declared_saved_root_fails_closed() {
    // The source still declares the saved root `^books` while `retire` names it.
    // Retiring it would drop a store the running program still reads and writes, so
    // it must be rejected and the store entry must stay Active.
    let root = temp_project("evolve-retire-root-still-declared", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             evolve\n\
             \x20   retire ^books\n",
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

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "retiring a still-declared saved root must fail closed: {:#?}",
        report.diagnostics
    );
    assert_entry_stays_active(&program, "store-books");
}

#[test]
fn evolve_retire_of_a_still_declared_store_index_fails_closed() {
    // The source still declares the store index `^books.byTitle` while `retire`
    // names it. Retiring it would drop a derived index the running program still
    // maintains, so it must be rejected and the index entry must stay Active.
    let root = temp_project("evolve-retire-index-still-declared", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   index byTitle(title) unique\n\
             evolve\n\
             \x20   retire ^books.byTitle\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::StoreIndex,
                "books::^books::byTitle",
                "index-by-title",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "retiring a still-declared store index must fail closed: {:#?}",
        report.diagnostics
    );
    assert_entry_stays_active(&program, "index-by-title");
}

/// A rejected retire must never reserve its target. The target keeps its
/// accepted lifecycle: when the binding produced a proposal the entry is Active
/// there, and when nothing else changed no proposal is emitted at all, so the
/// accepted catalog (which had the entry Active) stands unchanged.
fn assert_entry_stays_active(program: &marrow_check::CheckedProgram, stable_id: &str) {
    let Some(proposal) = &program.catalog.proposal else {
        return;
    };
    let entry = proposal
        .entries
        .iter()
        .find(|entry| entry.stable_id == fixture_id(stable_id))
        .expect("proposal must keep the retire target entry");
    assert_eq!(
        entry.lifecycle,
        CatalogLifecycle::Active,
        "a retire the source still declares must not reserve the entry: {entry:#?}"
    );
}

#[test]
fn evolve_target_that_resolves_to_nothing_is_diagnosed() {
    let root = temp_project("evolve-unknown-target", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             evolve\n\
             \x20   retire Book.missing\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_EVOLVE_TARGET),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_binding_that_would_collide_identity_is_reported_at_check() {
    // A rename carries the accepted `member-a` onto `Book.c` while the source also
    // freshly declares `Book.c`. The two would share the path `books::Book::c` in
    // the proposal, an identity collision that must surface as a check diagnostic
    // rather than be deferred to apply. The proposal a check produces must always
    // validate.
    let root = temp_project("evolve-binding-identity-collision", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   c: string\n\
             evolve\n\
             \x20   rename Book.a -> Book.c\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::a",
                "member-a",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::c",
                "member-c",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "an identity collision must be reported at check: {:#?}",
        report.diagnostics
    );
    if let Some(proposal) = program.catalog.proposal {
        CatalogMetadata::from_json(&proposal.to_json_pretty())
            .expect("a proposal a check produces must validate");
    }
}

#[test]
fn evolve_rename_whose_source_is_still_declared_fails_closed() {
    // A rename means the old spelling is gone from source. Here `Book.a` is still
    // a live source member while a rename also carries it to `Book.c`, so the
    // accepted entry `member-a` must not be aliased onto two live source members.
    let root = temp_project("evolve-rename-source-still-declared", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   a: string\n\
             \x20   c: string\n\
             evolve\n\
             \x20   rename Book.a -> Book.c\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::a",
                "member-a",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "a rename whose source is still declared must fail closed: {:#?}",
        report.diagnostics
    );
    let module = program.facts.module_id("books").expect("module");
    let resource = program.facts.resource_id(module, "Book").expect("resource");
    let bound: Vec<&str> = program
        .facts
        .resource_members()
        .iter()
        .filter(|member| {
            member.resource == resource
                && member.catalog_id.as_deref() == Some(fixture_id("member-a").as_str())
        })
        .map(|member| member.name.as_str())
        .collect();
    assert!(
        bound.len() <= 1,
        "stable id member-a must not bind two source members: {bound:#?}"
    );
}

#[test]
fn evolve_rename_onto_a_live_accepted_target_fails_closed() {
    // Both `Book.a` and `Book.b` are live accepted entries the source still
    // declares; renaming `a` onto `b` would silently no-op (b already binds), so
    // a declared intent that cannot move identity must be diagnosed.
    let root = temp_project("evolve-rename-onto-live-target", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   a: string\n\
             \x20   b: string\n\
             evolve\n\
             \x20   rename Book.a -> Book.b\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::a",
                "member-a",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::b",
                "member-b",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "a rename onto a live accepted target must fail closed: {:#?}",
        report.diagnostics
    );
}

#[test]
fn two_renames_onto_the_same_target_conflict() {
    // The rename graph must be injective: two renames targeting `Book.c` cannot
    // both carry their identity forward, so the collision is diagnosed instead of
    // collapsing last-writer-wins and orphaning one accepted entry.
    let root = temp_project("evolve-rename-target-conflict", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   c: string\n\
             evolve\n\
             \x20   rename Book.a -> Book.c\n\
             \x20   rename Book.b -> Book.c\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::a",
                "member-a",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::b",
                "member-b",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "two renames onto one target must conflict: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_transform_body_reports_undefined_names_and_calls() {
    // A transform body is held to the same name-resolution rules a function body
    // is: an undefined identifier and an unknown call are caught at check time, not
    // left as unchecked free text.
    let root = temp_project("evolve-transform-undefined", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             evolve\n\
             \x20   transform Book.title\n\
             \x20   \x20   const x: string = totallyUndefinedVar\n\
             \x20   \x20   const y: string = nonexistentFn()\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_UNRESOLVED_NAME),
        "undefined identifier in a transform body must be reported: {:#?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_UNRESOLVED_CALL),
        "unknown call in a transform body must be reported: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_default_value_type_mismatch_is_diagnosed() {
    let root = temp_project("evolve-default-type", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required count: int\n\
             evolve\n\
             \x20   default Book.count = \"not a number\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_EVOLVE_TYPE),
        "{:#?}",
        report.diagnostics
    );
}

/// A plausible deterministic 128-bit path-derived id. Proposed ids must not match
/// this shape-derived value; source spelling is not durable identity.
fn path_derived_128_id(kind: CatalogEntryKind, path: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (kind, path, "path-derived-a").hash(&mut hasher);
    let first = hasher.finish();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (kind, path, "path-derived-b").hash(&mut hasher);
    let second = hasher.finish();
    format!("cat_{first:016x}{second:016x}")
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
fn proposed_ids_are_not_derived_from_the_member_path() {
    // Two members at different source paths must receive ids that are not a hash of
    // their path, so changing a path never changes which id a member would derive.
    let root = temp_project("catalog-id-not-path-derived", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             \x20   subtitle: string\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program.catalog.proposal.expect("proposal");
    for path in ["books::Book::title", "books::Book::subtitle"] {
        let proposed = proposed_id_for_path(&proposal, CatalogEntryKind::ResourceMember, path);
        let derived = path_derived_128_id(CatalogEntryKind::ResourceMember, path);
        assert_ne!(
            proposed, derived,
            "id for {path} must not be a hash of its path"
        );
    }
}

#[test]
fn proposed_ids_use_128_bit_random_shape() {
    let root = temp_project("catalog-id-128-bit", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   title: string\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

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
fn parallel_catalog_additions_merge_without_regenerating_ids() {
    let branch_a_id = "cat_11111111111111111111111111111111";
    let branch_b_id = "cat_22222222222222222222222222222222";
    let metadata = CatalogMetadata::new(
        9,
        vec![
            entry(
                CatalogEntryKind::Resource,
                "branch_a::Book",
                branch_a_id,
                &[],
            ),
            entry(
                CatalogEntryKind::Resource,
                "branch_b::Magazine",
                branch_b_id,
                &[],
            ),
        ],
    );

    let parsed = CatalogMetadata::from_json(&metadata.to_json_pretty()).expect("catalog parses");

    assert_eq!(parsed.entries[0].stable_id, branch_a_id);
    assert_eq!(parsed.entries[1].stable_id, branch_b_id);
    assert!(parsed.digest.starts_with("sha256:"));
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
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             evolve\n\
             \x20   rename Book.title -> Book.subtitle\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "cat_000000000000000000000000000000ff",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");

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
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             \x20   subtitle: string\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    marrow_check::commit_pending_identity(&root, &config(), &program)
        .expect("commit baseline")
        .expect("baseline written");

    // The accepted file now carries the frozen ids. Read it directly so the
    // assertion is against the committed identity, then confirm a re-check binds
    // exactly that id rather than minting a fresh one.
    let frozen = {
        let json = fs::read_to_string(catalog_path(&root)).expect("read accepted catalog");
        let accepted = CatalogMetadata::from_json(&json).expect("accepted catalog parses");
        accepted
            .entries
            .iter()
            .find(|entry| entry.kind == CatalogEntryKind::Resource && entry.path == "books::Book")
            .expect("committed Book entry")
            .stable_id
            .clone()
    };

    let (recheck, program) = check_project(&root, &config()).expect("recheck");
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
            "module books\nresource Book at ^books(id: int)\n    title: string\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    marrow_check::commit_pending_identity(&root, &config(), &program)
        .expect("commit baseline")
        .expect("baseline written");

    let json = fs::read_to_string(catalog_path(&root)).expect("read accepted catalog");

    assert!(
        !json.contains("acceptedLeaf"),
        "the committed catalog must not persist a redundant acceptedLeaf field: {json}"
    );

    let accepted = CatalogMetadata::from_json(&json).expect("accepted catalog parses");
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
fn committed_keyed_leaf_map_member_folds_its_key_shape_into_the_signature() {
    // A keyed-leaf `map[K, V]` member is a leaf position whose accepted signature folds its key
    // shape onto the value type, so a change to its key arity, key type, or value type is caught
    // as one leaf type change. The committed baseline records `leaf:[<key>]<value>`, proving the
    // production mint carries the key prefix, not only a hand-built fixture.
    let root = temp_project("catalog-keyed-leaf-map-token", |root| {
        write(
            root,
            "src/books.mw",
            "module books\nresource Book at ^books(id: int)\n    tags(pos: int): string\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    marrow_check::commit_pending_identity(&root, &config(), &program)
        .expect("commit baseline")
        .expect("baseline written");

    let json = fs::read_to_string(catalog_path(&root)).expect("read accepted catalog");

    let accepted = CatalogMetadata::from_json(&json).expect("accepted catalog parses");
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
        "a keyed-leaf map member folds its [key] shape onto the value in its leaf signature"
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
            "module books\nresource Book at ^books(id: int)\n    title: string\n",
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

    let (report, program) = check_project(&root, &config()).expect("check");

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
             resource Book at ^books(id: int)\n\
             \x20   title: string\n\
             \x20   subtitle: string\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");
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
