//! `type_at`/`scope_at`: position→type and visible-bindings for editor tooling,
//! reconstructing the cursor's lexical scope exactly as the checker does and
//! emitting no diagnostics of their own.

mod support;

use std::path::PathBuf;

use marrow_check::program::MarrowType;
use marrow_check::{
    CatalogEntryKind, CheckedProgram, ProjectSources, StoreIndexUsageBitmap, UseSiteKind,
    analyze_project, check_project, scope_at, type_at,
};
use marrow_schema::ScalarType;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{
    TreeStore, write_tree_backup_archive_chunk, write_tree_backup_archive_header,
};
use marrow_syntax::ParsedSource;

use support::{analyze_overlay, config, temp_root, write};

/// Analyze a single `src/m.mw` source and return the program plus the parse for
/// that file, so a test can position into the buffer it controls. The source is
/// written to disk so project discovery finds it, then re-supplied as an overlay
/// to exercise the same path editor tooling uses.
fn analyze(name: &str, source: &str) -> (CheckedProgram, ParsedSource, PathBuf) {
    let (snapshot, paths) = analyze_overlay(name, &[("src/m.mw", source)]);
    let path = paths.into_iter().next().expect("the written file path");
    let parsed = snapshot
        .files
        .into_iter()
        .find(|file| file.path == path)
        .expect("the overlaid file is analyzed")
        .parsed;
    (snapshot.program, parsed, path)
}

#[test]
fn type_at_a_literal_is_its_scalar_type() {
    let source = "module m\nfn f()\n    const n = 42\n";
    let (program, parsed, path) = analyze("type-at-literal", source);
    let offset = source.find("42").expect("literal present in source") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Int)), "{ty:?}");
}

#[test]
fn type_at_a_parameter_reference_is_the_parameter_type() {
    // `title` is a `string` parameter; a reference to it inside the body must
    // type to `string`, which requires the function's parameter scope.
    let source = "module m\nfn greet(title: string)\n    print(title)\n";
    let (program, parsed, path) = analyze("type-at-param", source);
    // Point at the *use* of `title` in `print(title)`, not the parameter decl.
    let offset = source.rfind("title").expect("use of title") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Str)), "{ty:?}");
}

#[test]
fn type_at_a_local_const_reference_is_its_inferred_type() {
    // A `const`/`var` introduced earlier in the block is visible later; the
    // reference must resolve through the reconstructed block scope.
    let source = "module m\nfn f()\n    const k = 7\n    print(k)\n";
    let (program, parsed, path) = analyze("type-at-local", source);
    let offset = source.rfind('k').expect("use of k");

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Int)), "{ty:?}");
}

#[test]
fn type_at_a_saved_field_read_is_the_declared_leaf_type() {
    // `^books(id).title` reads a `string` field of the `Book` resource. Typing
    // it requires both the saved-data machinery and the `id` parameter in scope.
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn peek(id: int): string\n    \
        return ^books(id).title\n";
    let (program, parsed, path) = analyze("type-at-saved-field", source);
    // Point at the `.title` leaf, so the smallest covering expression is the whole
    // field read `^books(id).title` rather than the inner `^books` root.
    let offset = source.rfind("title").expect("the .title field read") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Str)), "{ty:?}");
}

#[test]
fn type_at_an_identity_annotation_binding_is_the_identity_type() {
    // `const id: Id(^books) = ...` binds an identity; a reference to `id` types to
    // `Id(^books)`, the checker's `Identity("books")`.
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f(): Id(^books)\n    \
        const id: Id(^books) = nextId(^books)\n    \
        return id\n";
    let (program, parsed, path) = analyze("type-at-identity", source);
    let offset = source.rfind("id\n").expect("use of id in return") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(
        ty,
        Some(MarrowType::Identity("books".to_string())),
        "{ty:?}"
    );
}

#[test]
fn scope_at_lists_params_locals_and_module_constants() {
    // At the `print` line, the visible bindings are the module constant `BASE`,
    // the parameter `n`, and the local `doubled` introduced before the cursor.
    let source = "module m\n\
        const BASE: int = 10\n\
        fn f(n: int)\n    \
        const doubled = n + n\n    \
        print(doubled)\n";
    let (program, parsed, path) = analyze("scope-at", source);
    let offset = source.find("print(doubled)").expect("print line");

    let bindings = scope_at(&program, &path, &parsed, offset);
    let names: Vec<&str> = bindings.iter().map(|(name, _)| name.as_str()).collect();
    assert!(names.contains(&"BASE"), "module const visible: {names:?}");
    assert!(names.contains(&"n"), "parameter visible: {names:?}");
    assert!(names.contains(&"doubled"), "local visible: {names:?}");

    let ty_of = |want: &str| {
        bindings
            .iter()
            .find(|(name, _)| name == want)
            .map(|(_, ty)| ty)
    };
    assert_eq!(ty_of("n"), Some(&MarrowType::Primitive(ScalarType::Int)));
    assert_eq!(
        ty_of("doubled"),
        Some(&MarrowType::Primitive(ScalarType::Int))
    );
    assert_eq!(ty_of("BASE"), Some(&MarrowType::Primitive(ScalarType::Int)));
}

#[test]
fn scope_at_excludes_locals_declared_after_the_cursor() {
    // `later` is declared after the cursor and must not be visible; `early` is.
    let source = "module m\n\
        fn f()\n    \
        const early = 1\n    \
        print(early)\n    \
        const later = 2\n";
    let (program, parsed, path) = analyze("scope-at-order", source);
    let offset = source.find("print(early)").expect("print line");

    let names: Vec<String> = scope_at(&program, &path, &parsed, offset)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(names.iter().any(|n| n == "early"), "{names:?}");
    assert!(!names.iter().any(|n| n == "later"), "{names:?}");
}

#[test]
fn scope_at_includes_a_loop_binding_typed_to_the_element() {
    // A `for` binding is in scope only within the loop body, typed to the
    // sequence element — the same rule the checker applies. Reconstructing the
    // cursor scope must push that frame.
    let source = "module m\n\
        fn f(items: sequence[int])\n    \
        for it in items\n        \
        print(it)\n";
    let (program, parsed, path) = analyze("scope-at-loop", source);
    let offset = source.find("print(it)").expect("loop body");

    let bindings = scope_at(&program, &path, &parsed, offset);
    let it = bindings
        .iter()
        .find(|(name, _)| name == "it")
        .map(|(_, ty)| ty);
    assert_eq!(
        it,
        Some(&MarrowType::Primitive(ScalarType::Int)),
        "{bindings:?}"
    );
}

#[test]
fn scope_at_includes_a_saved_group_loop_binding_typed_to_the_entry() {
    let source = "module m\n\
        resource Book\n    \
        versions(version: int)\n        \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f(id: Id(^books))\n    \
        for n, version in ^books(id).versions\n        \
        print(version.title)\n";
    let (program, parsed, path) = analyze("scope-at-saved-group-loop", source);
    let offset = source.find("print(version.title)").expect("loop body");

    let bindings = scope_at(&program, &path, &parsed, offset);
    let ty_of = |want: &str| {
        bindings
            .iter()
            .find(|(name, _)| name == want)
            .map(|(_, ty)| ty)
    };
    assert_eq!(ty_of("n"), Some(&MarrowType::Primitive(ScalarType::Int)));
    assert_eq!(
        ty_of("version"),
        Some(&MarrowType::GroupEntry {
            resource: "m::Book".to_string(),
            layers: vec!["versions".to_string()],
        }),
        "{bindings:?}"
    );
}

#[test]
fn scope_at_includes_a_catch_binding_typed_error() {
    // A `catch` clause binds an `Error` value for the duration of its block;
    // reconstructing the cursor scope inside the catch must push that frame.
    let source = "module m\n\
        fn f()\n    \
        try\n        \
        print(1)\n    \
        catch e\n        \
        print(e)\n";
    let (program, parsed, path) = analyze("scope-at-catch", source);
    let offset = source.rfind("print(e)").expect("catch body");

    let bindings = scope_at(&program, &path, &parsed, offset);
    let e = bindings
        .iter()
        .find(|(name, _)| name == "e")
        .map(|(_, ty)| ty);
    assert_eq!(e, Some(&MarrowType::Error), "{bindings:?}");
}

#[test]
fn scope_at_includes_use_import_aliases() {
    // A `use std::clock` brings the short name `clock` into scope for call
    // expansion. `scope_at` should surface the import so completion can offer it.
    let source = "module m\n\
        use std::clock\n\
        fn f()\n    \
        const now = std::clock::now()\n    \
        print(now)\n";
    let (program, parsed, path) = analyze("scope-at-import", source);
    let offset = source.find("print(now)").expect("print line");

    let names: Vec<String> = scope_at(&program, &path, &parsed, offset)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(
        names.iter().any(|n| n == "clock"),
        "import alias: {names:?}"
    );
}

#[test]
fn type_at_a_cross_module_resource_field_read_uses_the_one_resolver() {
    // A field read off a value typed as a resource declared in another module
    // (`thing.name` where `thing: shelf::Thing`) carries the canonical resource
    // name `shelf::Thing`. Typing it routes that name through the shared resolver,
    // so a bare-name shortcut cannot diverge from how calls and go-to-def resolve
    // the same resource. The field is a `string`, so the read types to `string`.
    let m = "module m\n\
        use shelf\n\
        fn describe(thing: shelf::Thing): string\n    \
        return thing.name\n";
    let shelf = "module shelf\nresource Thing\n    name: string\n";
    let (snapshot, paths) = analyze_overlay(
        "type-at-cross-module-field",
        &[("src/m.mw", m), ("src/shelf.mw", shelf)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let path = paths
        .into_iter()
        .find(|path| path.ends_with("m.mw"))
        .expect("the m.mw path");
    let parsed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("m.mw is analyzed")
        .parsed
        .clone();
    let offset = m.rfind("name").expect("the .name field read") + 1;

    let ty = type_at(&snapshot.program, &path, &parsed, offset);
    assert_eq!(ty, Some(MarrowType::Primitive(ScalarType::Str)), "{ty:?}");
}

#[test]
fn type_at_and_scope_at_emit_no_diagnostics() {
    // The whole point: tooling queries reuse the checker's inference without a
    // diagnostics sink. The queries take an immutable program and parse and return
    // only a type or bindings, so they cannot add to a project's diagnostics; this
    // test pins that the queries return real answers (so "no diagnostics" is not
    // vacuous) and that sweeping the buffer never panics.
    let source = "module m\n\
        fn f(title: string)\n    \
        const greeting = title\n    \
        print(greeting)\n";
    let (snapshot, paths) = analyze_overlay("no-diagnostics", &[("src/m.mw", source)]);
    let path = paths.into_iter().next().expect("the written file path");
    let before = snapshot.report.diagnostics.clone();
    assert!(!snapshot.report.has_errors(), "{before:#?}");
    let program = &snapshot.program;
    let parsed = &snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("the overlaid file is analyzed")
        .parsed;

    // Real answers, so the no-diagnostics guarantee is not vacuously true.
    let title = source.rfind("title").expect("use of title");
    assert_eq!(
        type_at(program, &path, parsed, title),
        Some(MarrowType::Primitive(ScalarType::Str)),
    );
    assert!(!scope_at(program, &path, parsed, title).is_empty());

    // Sweeping every offset is total and never panics, and leaves the analysis
    // report it was derived from untouched — the queries have no diagnostics sink.
    for offset in 0..=source.len() {
        let _ = type_at(program, &path, parsed, offset);
        let _ = scope_at(program, &path, parsed, offset);
    }
    assert_eq!(
        snapshot.report.diagnostics, before,
        "queries left the report unchanged"
    );
}

#[test]
fn sites_for_reports_saved_catalog_uses_from_lowered_bodies() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n    \
        shelf: string\n\
        store ^books(id: int): Book\n    \
        index byShelf(shelf, id)\n\
        fn title(id: int): string\n    \
        return ^books(id).title ?? \"\"\n\
        fn on_shelf(shelf: string): int\n    \
        return count(^books.byShelf(shelf))\n";
    let (snapshot, paths) = analyze_overlay("analysis-use-sites", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];

    let store = snapshot
        .program
        .facts
        .stores()
        .iter()
        .find(|store| store.root == "books")
        .expect("books store");
    let store_catalog_id = snapshot
        .program
        .store_catalog_id(store.id)
        .expect("store has a catalog id")
        .to_string();
    let title_catalog_id = proposal_id(
        &snapshot,
        CatalogEntryKind::ResourceMember,
        "m::Book::title",
    );
    let shelf_index_catalog_id = snapshot
        .program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.name == "byShelf")
        .and_then(|index| snapshot.program.store_index_catalog_id(index.id))
        .expect("byShelf has a catalog id")
        .to_string();

    let store_sites = snapshot.sites_for(&store_catalog_id);
    assert!(
        store_sites.iter().any(|site| {
            site.file == *file
                && site.kind == UseSiteKind::SavedRoot
                && source[site.span.start_byte..site.span.end_byte].contains("^books")
        }),
        "store use sites: {store_sites:#?}"
    );

    let title_sites = snapshot.sites_for(&title_catalog_id);
    assert!(
        title_sites.iter().any(|site| {
            site.file == *file
                && site.kind == UseSiteKind::ResourceMember
                && source[site.span.start_byte..site.span.end_byte].contains(".title")
        }),
        "title use sites: {title_sites:#?}"
    );

    let index_sites = snapshot.sites_for(&shelf_index_catalog_id);
    assert!(
        index_sites.iter().any(|site| {
            site.file == *file
                && site.kind == UseSiteKind::StoreIndex
                && source[site.span.start_byte..site.span.end_byte].contains("byShelf")
        }),
        "index use sites: {index_sites:#?}"
    );
}

#[test]
fn sites_for_reports_proposal_saved_layer_uses_from_lowered_bodies() {
    let source = "module m\n\
        resource Book\n    \
        versions(version: int)\n        \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f(id: Id(^books))\n    \
        for n, version in ^books(id).versions\n        \
        print(version.title)\n";
    let (snapshot, paths) =
        analyze_overlay("analysis-use-sites-proposal-layer", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];
    let versions_catalog_id = proposal_id(
        &snapshot,
        CatalogEntryKind::ResourceMember,
        "m::Book::versions",
    );

    let version_sites = snapshot.sites_for(&versions_catalog_id);
    assert!(
        version_sites.iter().any(|site| {
            site.file == *file
                && site.kind == UseSiteKind::ResourceMember
                && source[site.span.start_byte..site.span.end_byte].contains(".versions")
        }),
        "versions use sites: {version_sites:#?}"
    );
}

#[test]
fn sites_for_reports_accepted_saved_layer_uses_from_lowered_bodies() {
    let source = "module m\n\
        resource Book\n    \
        versions(version: int)\n        \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f(id: Id(^books))\n    \
        for n, version in ^books(id).versions\n        \
        print(version.title)\n";
    let root = temp_root("analysis-use-sites-accepted-layer");
    write(&root, "src/m.mw", source);
    let (report, program) = check_project(&root, &config()).expect("baseline check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let accepted = program.catalog.proposal.clone().expect("baseline proposal");
    let versions_catalog_id = accepted
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "m::Book::versions"
        })
        .expect("accepted versions layer")
        .stable_id
        .clone();

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze accepted source");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = root.join("src/m.mw");

    let version_sites = snapshot.sites_for(&versions_catalog_id);
    assert!(
        version_sites.iter().any(|site| {
            site.file == file
                && site.kind == UseSiteKind::ResourceMember
                && source[site.span.start_byte..site.span.end_byte].contains(".versions")
        }),
        "versions use sites: {version_sites:#?}"
    );
}

#[test]
fn sites_for_reports_saved_catalog_uses_from_module_constants() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        const DEFAULT = Status::active\n";
    let (snapshot, paths) = analyze_overlay("analysis-use-sites-const", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];
    let active_catalog_id =
        proposal_id(&snapshot, CatalogEntryKind::EnumMember, "m::Status::active");

    let active_sites = snapshot.sites_for(&active_catalog_id);
    assert!(
        active_sites.iter().any(|site| {
            site.file == *file
                && site.kind == UseSiteKind::EnumMember
                && source[site.span.start_byte..site.span.end_byte].contains("Status::active")
        }),
        "active use sites: {active_sites:#?}"
    );
}

#[test]
fn sites_for_reports_catalog_uses_from_evolve_transform_bodies() {
    let baseline = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n";
    let evolved = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        resource Book\n    \
        required title: string\n    \
        required state: Status\n\
        store ^books(id: int): Book\n\
        evolve\n    \
        transform Book.state\n        \
        return Status::active\n";
    let root = temp_root("analysis-use-sites-transform");
    write(&root, "src/m.mw", baseline);
    let (baseline_report, baseline_program) =
        check_project(&root, &config()).expect("baseline check");
    assert!(
        !baseline_report.has_errors(),
        "{:#?}",
        baseline_report.diagnostics
    );
    let accepted = baseline_program
        .catalog
        .proposal
        .clone()
        .expect("baseline proposal");
    write(&root, "src/m.mw", evolved);
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze evolved source");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let active_catalog_id = accepted
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::EnumMember && entry.path == "m::Status::active"
        })
        .expect("accepted active member")
        .stable_id
        .clone();
    let file = root.join("src/m.mw");
    let active_sites = snapshot.sites_for(&active_catalog_id);
    assert!(
        active_sites.iter().any(|site| {
            site.file == file
                && site.kind == UseSiteKind::EnumMember
                && evolved[site.span.start_byte..site.span.end_byte].contains("Status::active")
        }),
        "active use sites: {active_sites:#?}"
    );
}

fn proposal_id(
    snapshot: &marrow_check::AnalysisSnapshot,
    kind: CatalogEntryKind,
    path: &str,
) -> String {
    snapshot
        .program
        .catalog
        .proposal
        .as_ref()
        .expect("first-run analysis proposes catalog ids")
        .entries
        .iter()
        .find(|entry| entry.kind == kind && entry.path == path)
        .unwrap_or_else(|| panic!("missing proposal entry {kind:?} {path}"))
        .stable_id
        .clone()
}

#[test]
fn store_index_facts_carry_reserved_empty_usage_bitmap() {
    let source = "module m\n\
        resource Book\n    \
        shelf: string\n\
        store ^books(id: int): Book\n    \
        index byShelf(shelf, id)\n";
    let (snapshot, _) = analyze_overlay("analysis-index-usage-bitmap", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let index = snapshot
        .program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.name == "byShelf")
        .expect("byShelf fact");
    assert_eq!(index.usage, StoreIndexUsageBitmap::default());
    assert!(
        !index.usage.any(),
        "the reserved usage bitmap shape is present but intentionally unpopulated"
    );
}

#[test]
fn evolution_preview_schema_only_defers_live_store_data() {
    let source = "module m\n\
        resource Book\n    \
        title: string\n\
        store ^books(id: int): Book\n";
    let (snapshot, _) =
        analyze_overlay("analysis-evolution-preview-schema", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let facts = marrow_check::evolution::evolution_preview(&snapshot, None)
        .expect("schema-only evolution preview");

    assert_eq!(facts.source_digest, snapshot.program.source_digest());
    assert_eq!(facts.backup, None);
    assert_eq!(
        facts.live_store,
        marrow_check::evolution::LiveStorePreviewStatus::Deferred
    );
}

#[test]
fn evolution_preview_reads_counts_and_samples_from_backup() {
    let source = "module m\n\
        resource Book\n    \
        title: string\n\
        store ^books(id: int): Book\n";
    let (snapshot, _) =
        analyze_overlay("analysis-evolution-preview-backup", &[("src/m.mw", source)]);
    let root = support::temp_root("analysis-evolution-preview-backup-archive");
    let archive = root.join("books.mwbackup");
    let store = TreeStore::memory();
    let store_id = CatalogId::new("cat_00000000000000000000000000000001").expect("store id");
    store
        .write_node(&store_id, &[SavedKey::Int(1)])
        .expect("seed backup cell");
    write_minimal_backup_archive(&archive, &store);

    let facts = marrow_check::evolution::evolution_preview(&snapshot, Some(&archive))
        .expect("backup evolution preview");
    let backup = facts.backup.expect("backup facts");

    assert_eq!(backup.cell_count, 1);
    assert_eq!(
        backup.sample_catalog_ids,
        vec!["cat_00000000000000000000000000000001".to_string()]
    );
    assert!(!backup.samples_truncated);
}

#[test]
fn evolution_preview_marks_backup_samples_truncated_only_after_omitting_distinct_ids() {
    let source = "module m\n\
        resource Book\n    \
        title: string\n\
        store ^books(id: int): Book\n";
    let (snapshot, _) = analyze_overlay(
        "analysis-evolution-preview-truncated-samples",
        &[("src/m.mw", source)],
    );
    let root = support::temp_root("analysis-evolution-preview-truncated-samples-archive");
    let archive = root.join("books.mwbackup");
    let store = TreeStore::memory();
    for n in 0..17 {
        let store_id = CatalogId::new(format!("cat_{n:032}")).expect("store id");
        store
            .write_node(&store_id, &[SavedKey::Int(n)])
            .expect("seed backup cell");
    }
    write_minimal_backup_archive(&archive, &store);

    let facts = marrow_check::evolution::evolution_preview(&snapshot, Some(&archive))
        .expect("backup evolution preview");
    let backup = facts.backup.expect("backup facts");

    assert_eq!(backup.cell_count, 17);
    assert_eq!(backup.sample_catalog_ids.len(), 16);
    assert!(backup.samples_truncated);
}

fn write_minimal_backup_archive(path: &std::path::Path, store: &TreeStore) {
    let mut file = std::fs::File::create(path).expect("create backup archive");
    write_tree_backup_archive_header(&mut file).expect("write archive header");
    write_tree_backup_archive_chunk(&mut file, b"{}").expect("write placeholder manifest");
    write_tree_backup_archive_chunk(&mut file, b"").expect("write empty catalog section");
    store
        .visit_backup_cells(|cell| {
            cell.write_framed(&mut file).expect("write backup cell");
            Ok(())
        })
        .expect("visit backup cells");
}
