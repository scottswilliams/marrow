use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{
    CHECK_BARE_MAYBE_PRESENT_READ, CHECK_CATALOG_INTENT, PresenceProofPlace, PresenceProofRead,
    PresenceProofSource, check_project,
};
use marrow_project::{
    CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata, parse_config,
};

fn temp_project(name: &str, build: impl FnOnce(&Path)) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create project root");
    build(&root);
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

fn config() -> marrow_project::ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}

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
        stable_id: stable_id.to_string(),
        aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
        lifecycle: CatalogLifecycle::Active,
    }
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
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        !accepted_path.exists(),
        "source-only check must not generate the accepted catalog file"
    );
    let proposal = program.catalog.proposal.expect("catalog proposal");
    assert_eq!(proposal.epoch, 1);
    let module = program.facts.module_id("books").expect("books module");
    let resource = program.facts.resource_id(module, "Book").expect("Book");
    assert!(
        program.facts.resource(resource).catalog_id.is_empty(),
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
    fs::remove_dir_all(&root).ok();

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
            stable_id: "enum-member-archived".to_string(),
            aliases: vec!["books::Status::inactive".to_string()],
            lifecycle: CatalogLifecycle::Deprecated,
        },
    ]);

    let json = metadata.to_json_pretty();
    let parsed = CatalogMetadata::from_json(&json).expect("catalog parses");

    assert_eq!(parsed.epoch, 7);
    assert_eq!(parsed.digest, metadata.digest);
    assert_eq!(parsed.entries, metadata.entries);
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
                lifecycle: CatalogLifecycle::Removed,
                ..entry(
                    CatalogEntryKind::Resource,
                    "books::Book",
                    "removed-book",
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
    fs::remove_dir_all(&root).ok();

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
    assert!(program.facts.resource(resource).catalog_id.is_empty());
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty()).expect("proposal validates");
}

#[test]
fn catalog_proposal_ids_do_not_collide_with_accepted_stable_ids() {
    let colliding_id = "cat_0f32222e2032f199";
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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    assert!(program.facts.resource(resource).catalog_id.is_empty());
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
    fs::remove_dir_all(&root).ok();

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let module = program.facts.module_id("library").expect("module");
    let resource = program.facts.resource_id(module, "Book").expect("resource");
    assert_eq!(program.facts.resource(resource).catalog_id, "res-book");
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
                stable_id: "enum-old-status".to_string(),
                aliases: vec!["books::Status".to_string()],
                lifecycle: CatalogLifecycle::Deprecated,
            },
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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
        .find(|entry| entry.stable_id == "enum-old-status")
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
    fs::remove_dir_all(&root).ok();

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
    assert_eq!(active.catalog_id, "enum-member-active");
    assert_eq!(archived.catalog_id, "enum-member-archived");
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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
fn if_exists_narrowing_expires_when_a_key_binding_is_passed_out() {
    let root = temp_project("presence-if-exists-out-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(out value: int)\n\
             \x20   value = 2\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       setTo(out k)\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
fn if_exists_narrowing_expires_when_a_key_field_is_passed_out() {
    let root = temp_project("presence-if-exists-out-key-field", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Holder\n\
             \x20   required id: int\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(out value: int)\n\
             \x20   value = 2\n\
             fn guarded(id: int): string\n\
             \x20   var holder = Holder(id: id)\n\
             \x20   if exists(^books(holder.id).subtitle)\n\
             \x20       setTo(out holder.id)\n\
             \x20       return ^books(holder.id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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
             fn setTo(out value: int): bool\n\
             \x20   value = 2\n\
             \x20   return true\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       if setTo(out k)\n\
             \x20           return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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
             fn setTo(out value: int): bool\n\
             \x20   value = 2\n\
             \x20   return true\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle) and setTo(out k)\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
        !proof_sources.contains(&PresenceProofSource::AttachedDataPending),
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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
    fs::remove_dir_all(&root).ok();

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
        proof_sources.contains(&PresenceProofSource::AttachedDataPending),
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
    for proof in program.facts.presence_proofs() {
        match proof.source {
            PresenceProofSource::Declaration
            | PresenceProofSource::Narrowing
            | PresenceProofSource::AttachedDataPending => {}
        }
    }
}
