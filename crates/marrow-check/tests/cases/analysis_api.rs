//! `type_at`/`scope_at`: position→type and visible-bindings for editor tooling,
//! reconstructing the cursor's lexical scope exactly as the checker does and
//! emitting no diagnostics of their own.
use crate::support;
use std::path::PathBuf;

use marrow_check::program::MarrowType;
use marrow_check::tooling::{
    DataChild, DataPathSegment, DataPresence, MAX_VALUE_PREVIEW_LIMIT, resolve_data_path,
    sample_integrity_problem_details, sample_integrity_problems, stamped_data_children,
    stamped_data_roots_in_store, stamped_integrity_problem_details, stamped_preview_data_path,
    stamped_read_data_path,
};
use marrow_check::{
    CatalogEntryKind, CheckedProgram, DiagnosticPayload, ProjectSources, SurfaceCatalogBlocker,
    SurfaceCatalogStatus, SurfaceReadFootprint, SurfaceReadOperationKind, UseSiteKind,
    analyze_project, check_project, scope_at, type_at,
};
use marrow_project::parse_config;
use marrow_schema::{SCHEMA_DUPLICATE_MEMBER, ScalarType, Type};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{
    CommitMetadata, DataPathSegment as StoreDataPathSegment, EngineProfile, StoreUid, TreeStore,
    write_tree_backup_archive_chunk, write_tree_backup_archive_header,
};
use marrow_store::value::{SavedValue, encode_value};
use marrow_syntax::ParsedSource;

use support::{analyze_overlay, config, temp_root, write};

const BACKUP_SAMPLE_MEMBER_ID: &str = "cat_ffffffffffffffffffffffffffffffff";

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
    // The whole point: tooling lookups reuse the checker's inference without a
    // diagnostics sink. The lookups take an immutable program and parse and return
    // only a type or bindings, so they cannot add to a project's diagnostics; this
    // test pins that the lookups return real answers (so "no diagnostics" is not
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
    // report it was derived from untouched — the lookups have no diagnostics sink.
    for offset in 0..=source.len() {
        let _ = type_at(program, &path, parsed, offset);
        let _ = scope_at(program, &path, parsed, offset);
    }
    assert_eq!(
        snapshot.report.diagnostics, before,
        "lookups left the report unchanged"
    );
}

#[test]
fn analysis_snapshot_exposes_snapshot_bound_surface_read_operations() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n    \
        author: string\n\
        store ^books(shelf: string, id: int): Book\n    \
        index byAuthor(author, shelf, id)\n\
        surface Books from ^books\n    \
        fields title\n    \
        collection ^books.byAuthor as byAuthor\n";
    let (snapshot, paths) =
        analyze_overlay("analysis-surface-read-operations", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let path = paths.into_iter().next().expect("source path");

    let operations: Vec<_> = snapshot.surface_read_operations().collect();
    assert_eq!(operations.len(), 2, "{operations:#?}");
    assert!(
        operations.iter().all(|operation| operation.file == path),
        "{operations:#?}"
    );

    let point = operations
        .iter()
        .find(|operation| {
            matches!(
                operation.operation.kind,
                SurfaceReadOperationKind::PointRead { .. }
            )
        })
        .expect("point read operation");
    assert_eq!(point.surface.name, "Books");
    let SurfaceReadFootprint::FullRecord { resource } = point.operation.footprint;
    assert_eq!(snapshot.program.facts.resource(resource).name, "Book");
    let projection: Vec<&str> = point
        .operation
        .projection
        .iter()
        .map(|member| {
            snapshot
                .program
                .facts
                .resource_members()
                .iter()
                .find(|fact| fact.id == *member)
                .expect("projection member")
                .name
                .as_str()
        })
        .collect();
    assert_eq!(projection, vec!["title"]);

    let by_author = operations
        .iter()
        .find(|operation| {
            matches!(
                operation.operation.kind,
                SurfaceReadOperationKind::PagedIndexCollection {
                    exact_key_count: 1,
                    identity_key_count: 2,
                    ..
                }
            )
        })
        .expect("by-author page operation");
    assert_eq!(by_author.surface.id, point.surface.id);
}

#[test]
fn surface_read_operation_analysis_reflects_catalog_status_without_identity_claim() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        surface Books from ^books\n    \
        fields title\n";
    let root = temp_root("analysis-surface-catalog-status");
    write(&root, "src/m.mw", source);
    let (report, program) = check_project(&root, &config()).expect("baseline check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let accepted = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes catalog ids");

    let source_only = analyze_project(&root, &config(), &ProjectSources::new(), None)
        .expect("source-only analysis");
    let stable = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("stable analysis");
    assert_eq!(source_only.content_identity(), stable.content_identity());

    let source_only_status = source_only
        .surface_read_operations()
        .next()
        .expect("source-only operation")
        .surface
        .catalog_status
        .clone();
    let stable_status = stable
        .surface_read_operations()
        .next()
        .expect("stable operation")
        .surface
        .catalog_status
        .clone();
    assert_eq!(
        source_only_status,
        SurfaceCatalogStatus::SourceOnly(vec![
            SurfaceCatalogBlocker::PendingCatalogProposal,
            SurfaceCatalogBlocker::MissingAcceptedCatalogIds,
        ])
    );
    assert_eq!(stable_status, SurfaceCatalogStatus::Stable);
}

#[test]
fn surface_read_operation_analysis_excludes_configured_test_file_surfaces() {
    let root = temp_root("analysis-surface-test-file-isolation");
    write(
        &root,
        "src/app.mw",
        "module app\n\
        resource Book\n    \
        title: string\n\
        store ^books(id: int): Book\n\
        surface Books from ^books\n    \
        fields title\n",
    );
    write(
        &root,
        "tests/smoke_test.mw",
        "use app\n\
        surface TestBooks from ^books\n    \
        fields title\n\
        pub fn smoke()\n    \
        return\n",
    );
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "tests": ["tests"], "store": { "backend": "memory" } }"#,
    )
    .expect("config");

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("analyze");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let surfaces: Vec<_> = snapshot
        .surface_read_operations()
        .map(|operation| operation.surface.name.as_str())
        .collect();
    assert_eq!(surfaces, vec!["Books"]);
}

#[test]
fn sites_for_reports_saved_catalog_uses_from_lowered_bodies() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n    \
        shelf: string\n\
        store ^books(id: string): Book\n\
        store ^byShelf(id: int): Book\n    \
        index byShelf(shelf, id)\n\
        fn title(title: string): string\n    \
        return ^books(title).title ?? \"\"\n\
        fn on_shelf(shelf: string): int\n    \
        return count(^byShelf.byShelf(shelf))\n";
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

    assert_eq!(
        site_texts(
            &snapshot,
            &store_catalog_id,
            UseSiteKind::SavedRoot,
            file,
            source
        ),
        vec!["^books"]
    );

    assert_eq!(
        site_texts(
            &snapshot,
            &title_catalog_id,
            UseSiteKind::ResourceMember,
            file,
            source
        ),
        vec!["title"]
    );

    assert_eq!(
        site_texts(
            &snapshot,
            &shelf_index_catalog_id,
            UseSiteKind::StoreIndex,
            file,
            source
        ),
        vec!["byShelf"]
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

    assert_eq!(
        site_texts(
            &snapshot,
            &versions_catalog_id,
            UseSiteKind::ResourceMember,
            file,
            source
        ),
        vec!["versions"]
    );
}

#[test]
fn sites_for_reports_keyed_saved_layer_segment_not_same_named_argument() {
    let source = "module m\n\
        resource Book\n    \
        versions(version: int)\n        \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f(id: Id(^books), versions: int)\n    \
        for n, version in ^books(id).versions(versions)\n        \
        print(version.title)\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-keyed-layer-segment",
        &[("src/m.mw", source)],
    );
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
    let versions_start = source
        .find(".versions(versions)")
        .expect("saved layer method call")
        + 1;

    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &versions_catalog_id,
            UseSiteKind::ResourceMember,
            file,
            source
        ),
        vec![("versions", versions_start)]
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

    assert_eq!(
        site_texts(
            &snapshot,
            &versions_catalog_id,
            UseSiteKind::ResourceMember,
            &file,
            source
        ),
        vec!["versions"]
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

    assert_eq!(
        site_texts(
            &snapshot,
            &active_catalog_id,
            UseSiteKind::EnumMember,
            file,
            source
        ),
        vec!["active"]
    );

    let status_catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "m::Status");
    assert_eq!(
        site_texts(
            &snapshot,
            &status_catalog_id,
            UseSiteKind::Enum,
            file,
            source
        ),
        vec!["Status"]
    );
}

#[test]
fn sites_for_reports_enum_uses_from_function_signature_annotations() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn f(s: Status): Status\n    \
        return Status::active\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-enum-signature-annotations",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];
    let status_catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "m::Status");
    let param_start = source.find("s: Status").expect("param annotation") + "s: ".len();
    let return_start = source.find("): Status").expect("return annotation") + "): ".len();
    let literal_start =
        source.find("return Status::active").expect("enum literal") + "return ".len();

    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &status_catalog_id,
            UseSiteKind::Enum,
            file,
            source
        ),
        vec![
            ("Status", param_start),
            ("Status", return_start),
            ("Status", literal_start)
        ]
    );
}

#[test]
fn sites_for_reports_enum_use_from_sequence_annotation_leaf_only() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn f(items: sequence[Status])\n    \
        print(count(items))\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-enum-sequence-annotation",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];
    let status_catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "m::Status");
    let sequence_start = source
        .find("sequence[Status]")
        .expect("sequence annotation")
        + "sequence[".len();

    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &status_catalog_id,
            UseSiteKind::Enum,
            file,
            source
        ),
        vec![("Status", sequence_start)]
    );
}

#[test]
fn sites_for_reports_enum_use_from_aliased_qualified_annotation_leaf_only() {
    let status_source = "module a::b\npub enum Status\n    active\n    archived\n";
    let source = "module app\n\
        use a::b\n\
        fn f(): b::Status\n    \
        return b::Status::active\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-enum-aliased-annotation",
        &[("src/a/b.mw", status_source), ("src/app.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[1];
    let status_catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "a::b::Status");
    let annotation_start = source.find("b::Status").expect("return annotation") + "b::".len();
    let literal_start = source
        .find("return b::Status::active")
        .expect("enum literal")
        + "return b::".len();

    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &status_catalog_id,
            UseSiteKind::Enum,
            file,
            source
        ),
        vec![("Status", annotation_start), ("Status", literal_start)]
    );
}

#[test]
fn sites_for_reports_enum_uses_from_configured_tests() {
    let root = temp_root("analysis-use-sites-enum-test-annotations");
    write(
        &root,
        "src/status.mw",
        "module status\n\
        pub enum Status\n    \
        active\n    \
        archived\n",
    );
    let test_source = "use status\n\
        const s: status::Status = status::Status::active\n";
    write(&root, "tests/smoke.mw", test_source);
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "tests": ["tests"], "store": { "backend": "memory" } }"#,
    )
    .expect("config");

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("analyze");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let test_file = root.join("tests/smoke.mw");
    assert!(
        snapshot.files.iter().any(|file| file.path == test_file),
        "configured test file should be retained in analysis snapshot"
    );
    let status_catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "status::Status");
    let active_catalog_id = proposal_id(
        &snapshot,
        CatalogEntryKind::EnumMember,
        "status::Status::active",
    );
    let annotation_start = test_source
        .find("status::Status =")
        .expect("test annotation")
        + "status::".len();
    let literal_start = test_source
        .find("status::Status::active")
        .expect("test literal")
        + "status::".len();

    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &status_catalog_id,
            UseSiteKind::Enum,
            &test_file,
            test_source
        ),
        vec![("Status", annotation_start), ("Status", literal_start)]
    );
    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &active_catalog_id,
            UseSiteKind::EnumMember,
            &test_file,
            test_source
        ),
        vec![("active", literal_start + "Status::".len())]
    );
}

#[test]
fn sites_for_ignores_test_local_enum_catalog_uses() {
    let root = temp_root("analysis-use-sites-test-local-enum");
    write(&root, "src/app.mw", "module app\nfn ok()\n    return\n");
    let test_source = "enum Scratch\n    one\nconst s: Scratch = Scratch::one\n";
    write(&root, "tests/smoke.mw", test_source);
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "tests": ["tests"], "store": { "backend": "memory" } }"#,
    )
    .expect("config");

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("analyze");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let test_file = root.join("tests/smoke.mw");
    assert!(
        snapshot
            .use_sites()
            .iter()
            .filter(|site| site.file == test_file)
            .all(|site| !matches!(site.kind, UseSiteKind::Enum | UseSiteKind::EnumMember)),
        "test-local enums must not mint catalog use-sites: {:#?}",
        snapshot.use_sites()
    );
}

#[test]
fn sites_for_fails_closed_for_ambiguous_bare_foreign_enum_annotation() {
    let status_a = "module a\npub enum Status\n    active\n";
    let status_b = "module b\npub enum Status\n    active\n";
    let app = "module app\nfn f(s: Status)\n    return\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-ambiguous-bare-enum-annotation",
        &[
            ("src/a.mw", status_a),
            ("src/b.mw", status_b),
            ("src/app.mw", app),
        ],
    );
    let unknown_type = support::with_code(&snapshot.report, "check.unknown_type");
    assert_eq!(unknown_type.len(), 1, "{:#?}", snapshot.report.diagnostics);
    assert_eq!(
        unknown_type[0].payload,
        DiagnosticPayload::AmbiguousType {
            ty: Type::Named("Status".into()),
            name: "Status".into(),
        }
    );
    let app_file = &paths[2];
    for path in ["a::Status", "b::Status"] {
        let catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, path);
        assert_eq!(
            site_texts(&snapshot, &catalog_id, UseSiteKind::Enum, app_file, app),
            Vec::<&str>::new(),
            "ambiguous bare enum annotation must not pick {path}"
        );
    }
}

#[test]
fn sites_for_fail_closed_for_ambiguous_bare_foreign_enum_member_literal() {
    let status_a = "module a\npub enum Status\n    active\n";
    let status_b = "module b\npub enum Status\n    active\n";
    let app = "module app\nfn f(): a::Status\n    return Status::active\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-ambiguous-bare-enum-member-literal",
        &[
            ("src/a.mw", status_a),
            ("src/b.mw", status_b),
            ("src/app.mw", app),
        ],
    );
    let ambiguous = support::with_code(&snapshot.report, "check.ambiguous_member");
    assert_eq!(ambiguous.len(), 1, "{:#?}", snapshot.report.diagnostics);
    let app_file = &paths[2];
    let status_a = proposal_id(&snapshot, CatalogEntryKind::Enum, "a::Status");
    let annotation_start = app.find("a::Status").expect("return annotation") + "a::".len();
    assert_eq!(
        site_texts_with_start(&snapshot, &status_a, UseSiteKind::Enum, app_file, app),
        vec![("Status", annotation_start)]
    );
    let status_b = proposal_id(&snapshot, CatalogEntryKind::Enum, "b::Status");
    assert_eq!(
        site_texts(&snapshot, &status_b, UseSiteKind::Enum, app_file, app),
        Vec::<&str>::new(),
        "ambiguous enum member literal must not pick b::Status"
    );
    for path in ["a::Status::active", "b::Status::active"] {
        let catalog_id = proposal_id(&snapshot, CatalogEntryKind::EnumMember, path);
        assert_eq!(
            site_texts(
                &snapshot,
                &catalog_id,
                UseSiteKind::EnumMember,
                app_file,
                app
            ),
            Vec::<&str>::new(),
            "ambiguous enum member literal must not pick {path}"
        );
    }
}

#[test]
fn sites_for_fail_closed_for_ambiguous_private_bare_foreign_enum_member_literal() {
    let status_a = "module a\nenum Status\n    active\n";
    let status_b = "module b\nenum Status\n    active\n";
    let app = "module app\nconst x = Status::active\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-ambiguous-private-bare-enum-member-literal",
        &[
            ("src/a.mw", status_a),
            ("src/b.mw", status_b),
            ("src/app.mw", app),
        ],
    );
    let ambiguous = support::with_code(&snapshot.report, "check.ambiguous_member");
    assert_eq!(ambiguous.len(), 1, "{:#?}", snapshot.report.diagnostics);
    let app_file = &paths[2];
    for path in ["a::Status", "b::Status"] {
        let catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, path);
        assert_eq!(
            site_texts(&snapshot, &catalog_id, UseSiteKind::Enum, app_file, app),
            Vec::<&str>::new(),
            "ambiguous private enum member literal must not pick {path}"
        );
    }
    for path in ["a::Status::active", "b::Status::active"] {
        let catalog_id = proposal_id(&snapshot, CatalogEntryKind::EnumMember, path);
        assert_eq!(
            site_texts(
                &snapshot,
                &catalog_id,
                UseSiteKind::EnumMember,
                app_file,
                app
            ),
            Vec::<&str>::new(),
            "ambiguous private enum member literal must not pick {path}"
        );
    }
}

#[test]
fn sites_for_private_enum_member_literal_has_no_catalog_use_sites() {
    let status = "module a\nenum Status\n    active\n";
    let app = "module app\nconst x = a::Status::active\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-private-enum-member-literal",
        &[("src/a.mw", status), ("src/app.mw", app)],
    );
    let private = support::with_code(&snapshot.report, "check.private_enum");
    assert_eq!(private.len(), 1, "{:#?}", snapshot.report.diagnostics);
    let app_file = &paths[1];
    let status_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "a::Status");
    assert_eq!(
        site_texts(&snapshot, &status_id, UseSiteKind::Enum, app_file, app),
        Vec::<&str>::new()
    );
    let active_id = proposal_id(&snapshot, CatalogEntryKind::EnumMember, "a::Status::active");
    assert_eq!(
        site_texts(
            &snapshot,
            &active_id,
            UseSiteKind::EnumMember,
            app_file,
            app
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn sites_for_qualified_enum_owner_before_ambiguous_bare_foreign_owner() {
    let status_a = "module a\npub enum Status\n    active\n";
    let status_b = "module b\npub enum Status\n    active\n";
    let choice = "module Status\npub enum Choice\n    active\n";
    let app = "module app\nfn f(): Status::Choice\n    return Status::Choice::active\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-qualified-owner-before-ambiguous-bare",
        &[
            ("src/a.mw", status_a),
            ("src/b.mw", status_b),
            ("src/Status.mw", choice),
            ("src/app.mw", app),
        ],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let app_file = &paths[3];
    let choice_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "Status::Choice");
    let annotation_start =
        app.find("Status::Choice").expect("qualified annotation") + "Status::".len();
    let literal_start = app
        .find("return Status::Choice::active")
        .expect("qualified literal")
        + "return Status::".len();
    assert_eq!(
        site_texts_with_start(&snapshot, &choice_id, UseSiteKind::Enum, app_file, app),
        vec![("Choice", annotation_start), ("Choice", literal_start)]
    );

    let active_id = proposal_id(
        &snapshot,
        CatalogEntryKind::EnumMember,
        "Status::Choice::active",
    );
    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &active_id,
            UseSiteKind::EnumMember,
            app_file,
            app
        ),
        vec![("active", literal_start + "Choice::".len())]
    );
    for path in ["a::Status", "b::Status"] {
        let status_id = proposal_id(&snapshot, CatalogEntryKind::Enum, path);
        assert_eq!(
            site_texts(&snapshot, &status_id, UseSiteKind::Enum, app_file, app),
            Vec::<&str>::new(),
            "qualified enum member literal must not pick bare owner {path}"
        );
    }
}

#[test]
fn sites_for_does_not_treat_resource_type_annotation_as_foreign_enum_use() {
    let foreign = "module a\npub enum Order\n    active\n";
    let app = "module app\n\
        resource Order\n    \
        title: string\n\
        fn set(o: Order)\n    \
        return\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-resource-shadows-foreign-enum",
        &[("src/a.mw", foreign), ("src/app.mw", app)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let app_file = &paths[1];
    let enum_catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "a::Order");

    assert_eq!(
        site_texts(
            &snapshot,
            &enum_catalog_id,
            UseSiteKind::Enum,
            app_file,
            app
        ),
        Vec::<&str>::new(),
        "resource annotation must not be reported as a foreign enum use"
    );
}

#[test]
fn sites_for_source_enum_annotations_ignore_test_local_public_enums() {
    let root = temp_root("analysis-source-use-sites-ignore-test-enums");
    let source = "module app\n\
        use source_status\n\
        fn f(s: source_status::Status): source_status::Status\n    \
        return source_status::Status::active\n";
    write(
        &root,
        "src/source_status.mw",
        "module source_status\n\
        pub enum Status\n    \
        active\n",
    );
    write(&root, "src/app.mw", source);
    write(
        &root,
        "tests/smoke.mw",
        "pub enum Status\n    active\nfn smoke()\n    return\n",
    );
    let cfg = parse_config(
        r#"{ "sourceRoots": ["src"], "tests": ["tests"], "store": { "backend": "memory" } }"#,
    )
    .expect("config");

    let snapshot = analyze_project(&root, &cfg, &ProjectSources::new(), None).expect("analyze");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot.program.facts.module_id("tests::smoke").is_none(),
        "configured test facts must not remain in the source snapshot"
    );
    let app_file = root.join("src/app.mw");
    let status_catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "source_status::Status");
    let param_start = source
        .find("source_status::Status")
        .expect("param annotation")
        + "source_status::".len();
    let return_start = source
        .find("): source_status::Status")
        .expect("return annotation")
        + "): source_status::".len();
    let literal_start = source
        .find("return source_status::Status::active")
        .expect("literal")
        + "return source_status::".len();

    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &status_catalog_id,
            UseSiteKind::Enum,
            &app_file,
            source
        ),
        vec![
            ("Status", param_start),
            ("Status", return_start),
            ("Status", literal_start)
        ]
    );
}

#[test]
fn sites_for_distinguishes_match_arm_header_from_body_enum_member_use() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        fn f(s: Status): Status\n    \
        match s\n        \
        active\n            \
        return Status::active\n        \
        archived\n            \
        return Status::archived\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-match-arm-header",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];
    let active_catalog_id =
        proposal_id(&snapshot, CatalogEntryKind::EnumMember, "m::Status::active");
    let header_start = source
        .find("\n        active\n")
        .expect("active arm header")
        + 9;
    let body_start = source
        .find("return Status::active")
        .expect("active body return")
        + "return Status::".len();

    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &active_catalog_id,
            UseSiteKind::EnumMember,
            file,
            source
        ),
        vec![("active", header_start), ("active", body_start)]
    );
}

#[test]
fn sites_for_reports_each_nested_enum_member_segment_in_expressions() {
    let source = "module m\n\
        enum Cat\n    \
        category tiger\n        \
        bengal\n\
        fn favorite(): Cat\n    \
        return Cat::tiger::bengal\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-nested-enum-expression",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];
    let cat_catalog_id = proposal_id(&snapshot, CatalogEntryKind::Enum, "m::Cat");
    let tiger_catalog_id = proposal_id(&snapshot, CatalogEntryKind::EnumMember, "m::Cat::tiger");
    let bengal_catalog_id = proposal_id(
        &snapshot,
        CatalogEntryKind::EnumMember,
        "m::Cat::tiger::bengal",
    );
    let literal = source
        .find("return Cat::tiger::bengal")
        .expect("nested enum literal");
    let return_annotation = source.find("): Cat").expect("return annotation") + "): ".len();

    assert_eq!(
        site_texts_with_start(&snapshot, &cat_catalog_id, UseSiteKind::Enum, file, source),
        vec![
            ("Cat", return_annotation),
            ("Cat", literal + "return ".len())
        ]
    );
    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &tiger_catalog_id,
            UseSiteKind::EnumMember,
            file,
            source
        ),
        vec![("tiger", literal + "return Cat::".len())]
    );
    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &bengal_catalog_id,
            UseSiteKind::EnumMember,
            file,
            source
        ),
        vec![("bengal", literal + "return Cat::tiger::".len())]
    );
}

#[test]
fn sites_for_reports_each_nested_enum_member_segment_in_match_arms() {
    let source = "module m\n\
        enum Cat\n    \
        category tiger\n        \
        bengal\n    \
        housecat\n\
        fn rank(c: Cat): int\n    \
        match c\n        \
        tiger::bengal\n            \
        return 1\n        \
        housecat\n            \
        return 2\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-use-sites-nested-enum-match-arm",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];
    let tiger_catalog_id = proposal_id(&snapshot, CatalogEntryKind::EnumMember, "m::Cat::tiger");
    let bengal_catalog_id = proposal_id(
        &snapshot,
        CatalogEntryKind::EnumMember,
        "m::Cat::tiger::bengal",
    );
    let arm = source
        .find("\n        tiger::bengal\n")
        .expect("nested match arm")
        + 9;

    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &tiger_catalog_id,
            UseSiteKind::EnumMember,
            file,
            source
        ),
        vec![("tiger", arm)]
    );
    assert_eq!(
        site_texts_with_start(
            &snapshot,
            &bengal_catalog_id,
            UseSiteKind::EnumMember,
            file,
            source
        ),
        vec![("bengal", arm + "tiger::".len())]
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
    assert_eq!(
        site_texts(
            &snapshot,
            &active_catalog_id,
            UseSiteKind::EnumMember,
            &file,
            evolved
        ),
        vec!["active"]
    );
}

#[test]
fn analysis_snapshot_exposes_catalog_declarations_by_catalog_id() {
    let source = "module m\n\
        enum Status\n    \
        active\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n    \
        index byShelf(title, id)\n";
    let (snapshot, paths) =
        analyze_overlay("analysis-catalog-declarations", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];

    let cases = [
        (
            proposal_id(&snapshot, CatalogEntryKind::Store, "m::^books"),
            CatalogEntryKind::Store,
            "books",
            "^books",
        ),
        (
            proposal_id(&snapshot, CatalogEntryKind::Resource, "m::Book"),
            CatalogEntryKind::Resource,
            "Book",
            "Book",
        ),
        (
            proposal_id(
                &snapshot,
                CatalogEntryKind::ResourceMember,
                "m::Book::title",
            ),
            CatalogEntryKind::ResourceMember,
            "title",
            "title",
        ),
        (
            proposal_id(
                &snapshot,
                CatalogEntryKind::StoreIndex,
                "m::^books::byShelf",
            ),
            CatalogEntryKind::StoreIndex,
            "byShelf",
            "byShelf",
        ),
        (
            proposal_id(&snapshot, CatalogEntryKind::Enum, "m::Status"),
            CatalogEntryKind::Enum,
            "Status",
            "Status",
        ),
        (
            proposal_id(&snapshot, CatalogEntryKind::EnumMember, "m::Status::active"),
            CatalogEntryKind::EnumMember,
            "active",
            "active",
        ),
    ];

    for (catalog_id, kind, name, text) in &cases {
        let declaration = snapshot
            .catalog_declaration(catalog_id)
            .unwrap_or_else(|| panic!("missing declaration for {catalog_id}"));
        assert_eq!(&declaration.catalog_id, catalog_id);
        assert_eq!(declaration.file, *file);
        assert_eq!(declaration.kind, *kind);
        assert_eq!(declaration.name, *name);
        assert_eq!(span_text(source, declaration.span), *text);
    }

    let all: Vec<_> = snapshot
        .catalog_declarations()
        .iter()
        .map(|declaration| {
            (
                declaration.kind,
                declaration.catalog_id.as_str(),
                declaration.name.as_str(),
                span_text(source, declaration.span),
            )
        })
        .collect();
    for (catalog_id, _, _, _) in &cases {
        assert!(
            all.iter().any(|(_, id, _, _)| id == catalog_id),
            "catalog declarations: {all:#?}"
        );
    }
    assert_eq!(
        all.len(),
        snapshot
            .program
            .catalog
            .proposal
            .as_ref()
            .expect("proposal")
            .entries
            .len(),
        "catalog declarations must cover every proposal entry: {all:#?}"
    );
}

#[test]
fn catalog_declarations_fail_closed_for_duplicate_resource_paths() {
    let source = "module m\n\
        resource Book\n    \
        title: string\n\
        resource Book\n    \
        title: string\n";
    let (snapshot, _) = analyze_overlay(
        "analysis-duplicate-resource-catalog-paths",
        &[("src/m.mw", source)],
    );
    assert!(
        snapshot.report.has_errors(),
        "duplicate resources should still be diagnosed"
    );
    let duplicate_ids = proposal_ids(&snapshot, CatalogEntryKind::Resource, "m::Book");
    assert_eq!(
        duplicate_ids.len(),
        2,
        "test fixture must produce duplicate proposal entries: {duplicate_ids:?}"
    );

    for catalog_id in duplicate_ids {
        assert!(
            snapshot.catalog_declaration(&catalog_id).is_none(),
            "ambiguous resource path must not expose collapsed declaration for {catalog_id}"
        );
    }
}

#[test]
fn catalog_use_sites_fail_closed_for_duplicate_resource_member_paths() {
    let source = "module m\n\
        resource Book\n    \
        title: string\n    \
        title: string\n\
        store ^books(id: int): Book\n\
        fn title(id: int): string\n    \
        return ^books(id).title ?? \"\"\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-duplicate-member-catalog-paths",
        &[("src/m.mw", source)],
    );
    assert!(
        snapshot.report.has_errors(),
        "duplicate resource members should still be diagnosed"
    );
    let file = &paths[0];
    let duplicate_ids = proposal_ids(
        &snapshot,
        CatalogEntryKind::ResourceMember,
        "m::Book::title",
    );
    assert_eq!(
        duplicate_ids.len(),
        2,
        "test fixture must produce duplicate proposal entries: {duplicate_ids:?}"
    );

    for catalog_id in duplicate_ids {
        assert!(
            snapshot.catalog_declaration(&catalog_id).is_none(),
            "ambiguous member path must not expose collapsed declaration for {catalog_id}"
        );
        assert_eq!(
            site_texts(
                &snapshot,
                &catalog_id,
                UseSiteKind::ResourceMember,
                file,
                source
            ),
            Vec::<&str>::new(),
            "ambiguous member path must not expose collapsed use sites for {catalog_id}"
        );
    }
}

#[test]
fn catalog_use_sites_fail_closed_for_duplicate_enum_member_paths() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        active\n\
        const s: Status = Status::active\n\
        enum Cat\n    \
        category tiger\n        \
        bengal\n        \
        bengal\n\
        const c: Cat = Cat::tiger::bengal\n";
    let (snapshot, paths) = analyze_overlay(
        "analysis-duplicate-enum-member-catalog-paths",
        &[("src/m.mw", source)],
    );
    assert!(
        snapshot.report.has_errors(),
        "duplicate enum members should still be diagnosed"
    );
    assert_eq!(
        snapshot
            .report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == SCHEMA_DUPLICATE_MEMBER)
            .count(),
        2,
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = &paths[0];
    let duplicate_paths = ["m::Status::active", "m::Cat::tiger::bengal"];

    for path in duplicate_paths {
        let duplicate_ids = proposal_ids(&snapshot, CatalogEntryKind::EnumMember, path);
        assert!(
            !duplicate_ids.is_empty(),
            "fixture should retain at least one proposal entry for {path}"
        );
        for catalog_id in duplicate_ids {
            assert!(
                snapshot.catalog_declaration(&catalog_id).is_none(),
                "ambiguous enum member path must not expose declaration for {catalog_id}"
            );
            assert_eq!(
                site_texts(
                    &snapshot,
                    &catalog_id,
                    UseSiteKind::EnumMember,
                    file,
                    source
                ),
                Vec::<&str>::new(),
                "ambiguous enum member path must not expose use sites for {catalog_id}"
            );
        }
    }
}

#[test]
fn accepted_catalog_use_sites_fail_closed_for_duplicate_current_source_enum_member_paths() {
    let baseline = "module m\n\
        enum Status\n    \
        active\n    \
        archived\n\
        const s: Status = Status::active\n";
    let evolved = "module m\n\
        enum Status\n    \
        active\n    \
        active\n    \
        archived\n\
        const s: Status = Status::active\n";
    let root = temp_root("analysis-accepted-duplicate-enum-member-catalog-paths");
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
    let active_catalog_id = accepted
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::EnumMember && entry.path == "m::Status::active"
        })
        .expect("accepted active member")
        .stable_id
        .clone();

    write(&root, "src/m.mw", evolved);
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze evolved source");
    assert!(
        snapshot.report.has_errors(),
        "duplicate current-source enum members should still be diagnosed"
    );
    assert_eq!(
        snapshot
            .report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == SCHEMA_DUPLICATE_MEMBER)
            .count(),
        1,
        "{:#?}",
        snapshot.report.diagnostics
    );
    let file = root.join("src/m.mw");

    assert!(
        snapshot.catalog_declaration(&active_catalog_id).is_none(),
        "ambiguous current-source enum member path must not expose accepted declaration"
    );
    assert_eq!(
        site_texts(
            &snapshot,
            &active_catalog_id,
            UseSiteKind::EnumMember,
            &file,
            evolved
        ),
        Vec::<&str>::new(),
        "ambiguous current-source enum member path must not expose accepted use sites"
    );
}

#[test]
fn accepted_catalog_use_sites_fail_closed_for_duplicate_current_source_member_paths() {
    let baseline = "module m\n\
        resource Book\n    \
        title: string\n\
        store ^books(id: int): Book\n\
        fn title(id: int): string\n    \
        return ^books(id).title ?? \"\"\n";
    let evolved = "module m\n\
        resource Book\n    \
        title: string\n    \
        title: string\n\
        store ^books(id: int): Book\n\
        fn title(id: int): string\n    \
        return ^books(id).title ?? \"\"\n";
    let root = temp_root("analysis-accepted-duplicate-member-catalog-paths");
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
    let title_catalog_id = accepted
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "m::Book::title"
        })
        .expect("accepted title member")
        .stable_id
        .clone();

    write(&root, "src/m.mw", evolved);
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze evolved source");
    assert!(
        snapshot.report.has_errors(),
        "duplicate current-source members should still be diagnosed"
    );
    let file = root.join("src/m.mw");

    assert!(
        snapshot.catalog_declaration(&title_catalog_id).is_none(),
        "ambiguous current-source member path must not expose accepted declaration"
    );
    assert_eq!(
        site_texts(
            &snapshot,
            &title_catalog_id,
            UseSiteKind::ResourceMember,
            &file,
            evolved
        ),
        Vec::<&str>::new(),
        "ambiguous current-source member path must not expose accepted use sites"
    );
}

#[test]
fn accepted_catalog_fallbacks_fail_closed_for_duplicate_changed_current_source_member_paths() {
    let baseline = "module m\n\
        resource Book\n    \
        title: string\n\
        store ^books(id: int): Book\n\
        fn title(id: int): string\n    \
        return ^books(id).title ?? \"\"\n";
    let evolved = "module m\n\
        resource Book\n    \
        title: int\n    \
        title: bool\n\
        store ^books(id: int): Book\n\
        fn title(id: int): int\n    \
        return ^books(id).title ?? 0\n";
    let root = temp_root("analysis-accepted-duplicate-changed-member-catalog-paths");
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
    let title_catalog_id = accepted
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "m::Book::title"
        })
        .expect("accepted title member")
        .stable_id
        .clone();

    write(&root, "src/m.mw", evolved);
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze evolved source");
    assert!(
        snapshot.report.has_errors(),
        "duplicate current-source members should still be diagnosed"
    );
    assert!(
        snapshot.program.catalog.proposal.is_some(),
        "changed duplicate member structures must force a proposal"
    );
    let file = root.join("src/m.mw");

    assert!(
        snapshot.catalog_declaration(&title_catalog_id).is_none(),
        "ambiguous changed current-source member path must not expose accepted declaration"
    );
    assert_eq!(
        site_texts(
            &snapshot,
            &title_catalog_id,
            UseSiteKind::ResourceMember,
            &file,
            evolved
        ),
        Vec::<&str>::new(),
        "ambiguous changed current-source member path must not expose accepted use sites"
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

fn proposal_ids(
    snapshot: &marrow_check::AnalysisSnapshot,
    kind: CatalogEntryKind,
    path: &str,
) -> Vec<String> {
    snapshot
        .program
        .catalog
        .proposal
        .as_ref()
        .expect("first-run analysis proposes catalog ids")
        .entries
        .iter()
        .filter(|entry| entry.kind == kind && entry.path == path)
        .map(|entry| entry.stable_id.clone())
        .collect()
}

fn span_text(source: &str, span: marrow_syntax::SourceSpan) -> &str {
    &source[span.start_byte..span.end_byte]
}

fn site_texts<'a>(
    snapshot: &marrow_check::AnalysisSnapshot,
    catalog_id: &str,
    kind: UseSiteKind,
    file: &std::path::Path,
    source: &'a str,
) -> Vec<&'a str> {
    site_texts_with_start(snapshot, catalog_id, kind, file, source)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

fn site_texts_with_start<'a>(
    snapshot: &marrow_check::AnalysisSnapshot,
    catalog_id: &str,
    kind: UseSiteKind,
    file: &std::path::Path,
    source: &'a str,
) -> Vec<(&'a str, usize)> {
    let mut sites: Vec<_> = snapshot
        .sites_for(catalog_id)
        .into_iter()
        .filter(|site| site.file == file && site.kind == kind)
        .map(|site| (span_text(source, site.span), site.span.start_byte))
        .collect();
    sites.sort_by_key(|(_, start)| *start);
    sites
}

#[test]
fn evolution_preview_schema_only_records_source_digest_without_backup() {
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
    seed_backup_sample_leaf(&store, "cat_00000000000000000000000000000001", 1);
    write_minimal_backup_archive(&archive, &store);

    let facts = marrow_check::evolution::evolution_preview(&snapshot, Some(&archive))
        .expect("backup evolution preview");
    let backup = facts.backup.expect("backup facts");

    assert_eq!(backup.cell_count, 1);
    assert_eq!(
        backup.sample_catalog_ids,
        vec![
            "cat_00000000000000000000000000000001".to_string(),
            BACKUP_SAMPLE_MEMBER_ID.to_string(),
        ]
    );
    assert!(!backup.samples_truncated);
}

#[test]
fn stamped_data_readers_carry_store_and_checked_snapshot_identity() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n";
    let root = temp_root("analysis-stamped-data-roots");
    write(&root, "src/m.mw", source);
    let (checked, program) = check_project(&root, &config()).expect("check source");
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    let accepted = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes a catalog");
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze accepted source");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let store_fact = snapshot
        .program
        .facts
        .stores()
        .iter()
        .find(|store| store.root == "books")
        .expect("books store");
    let store_id = CatalogId::new(
        snapshot
            .program
            .store_catalog_id(store_fact.id)
            .expect("books store catalog id"),
    )
    .expect("valid store catalog id");
    let title_id = CatalogId::new(
        accepted
            .entries
            .iter()
            .find(|entry| {
                entry.kind == CatalogEntryKind::ResourceMember && entry.path == "m::Book::title"
            })
            .expect("accepted title catalog id")
            .stable_id
            .clone(),
    )
    .expect("valid title catalog id");
    let store = TreeStore::memory();
    store
        .replace_catalog_snapshot(&accepted)
        .expect("write catalog snapshot");
    store
        .write_record_presence(&store_id, &[SavedKey::Int(7)])
        .expect("seed record presence");
    store
        .write_record_presence(&store_id, &[SavedKey::Int(8)])
        .expect("seed second record presence");
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(7)],
            &[StoreDataPathSegment::Member(title_id)],
            b"Dune".to_vec(),
        )
        .expect("seed title");
    let uid = StoreUid::from_entropy_bytes([7; 16]);
    store.write_store_uid(&uid).expect("write store uid");
    let committed_source_digest =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let profile = EngineProfile::new(0);
    store
        .write_commit_metadata(&CommitMetadata {
            commit_id: 11,
            catalog_epoch: 3,
            layout_epoch: profile.layout_epoch(),
            source_digest: committed_source_digest.to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: vec![store_id],
            changed_index_catalog_ids: Vec::new(),
        })
        .expect("write commit metadata");

    let stamped =
        stamped_data_roots_in_store(&snapshot.program, &store).expect("stamped roots read");

    assert_eq!(stamped.data, vec!["books".to_string()]);
    assert_eq!(stamped.stamp.store_uid.as_ref(), Some(&uid));
    assert_eq!(
        stamped.stamp.store_catalog_digest.as_deref(),
        Some(accepted.digest.as_str())
    );
    let store_commit = stamped.stamp.store_commit.as_ref().expect("commit stamp");
    assert_eq!(store_commit.commit_id, 11);
    assert_eq!(store_commit.catalog_epoch, 3);
    assert_eq!(store_commit.source_digest, committed_source_digest);
    assert_eq!(store_commit.layout_epoch, profile.layout_epoch());
    assert_eq!(store_commit.engine_profile_digest, profile.digest_bytes());
    assert_eq!(
        stamped.stamp.checked_source_digest,
        snapshot.program.source_digest()
    );

    let path = resolve_data_path(
        &snapshot.program,
        &[
            DataPathSegment::Root("books".to_string()),
            DataPathSegment::Key(SavedKey::Int(7)),
            DataPathSegment::Field("title".to_string()),
        ],
    )
    .expect("valid data path")
    .expect("accepted data path");
    let stamped_value =
        stamped_read_data_path(&snapshot.program, &store, &path).expect("stamped value read");

    assert_eq!(stamped_value.data.presence, DataPresence::ValueOnly);
    assert_eq!(
        stamped_value
            .data
            .payload
            .as_ref()
            .map(|value| value.as_bytes()),
        Some(&b"Dune"[..])
    );
    assert_eq!(stamped_value.stamp, stamped.stamp);

    let stamped_record_children = stamped_data_children(
        &snapshot.program,
        &store,
        &[DataPathSegment::Root("books".to_string())],
        1,
        None,
    )
    .expect("stamped record children read");

    assert_eq!(
        stamped_record_children.data.children,
        vec![DataChild::Key(SavedKey::Int(7))]
    );
    assert!(stamped_record_children.data.truncated);
    assert_eq!(stamped_record_children.data.cursor, Some(SavedKey::Int(7)));
    assert_eq!(stamped_record_children.stamp, stamped.stamp);

    let stamped_children = stamped_data_children(
        &snapshot.program,
        &store,
        &[
            DataPathSegment::Root("books".to_string()),
            DataPathSegment::Key(SavedKey::Int(7)),
        ],
        10,
        None,
    )
    .expect("stamped children read");

    assert_eq!(
        stamped_children.data.children,
        vec![DataChild::Field("title".to_string())]
    );
    assert!(!stamped_children.data.truncated);
    assert_eq!(stamped_children.data.cursor, None);
    assert_eq!(stamped_children.stamp, stamped.stamp);
}

#[test]
fn stamped_data_preview_is_bounded_and_marks_truncation() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n";
    let root = temp_root("analysis-stamped-data-preview");
    write(&root, "src/m.mw", source);
    let (checked, program) = check_project(&root, &config()).expect("check source");
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    let accepted = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes a catalog");
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze accepted source");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let store_fact = snapshot
        .program
        .facts
        .stores()
        .iter()
        .find(|store| store.root == "books")
        .expect("books store");
    let store_id = CatalogId::new(
        snapshot
            .program
            .store_catalog_id(store_fact.id)
            .expect("books store catalog id"),
    )
    .expect("valid store catalog id");
    let title_id = CatalogId::new(
        accepted
            .entries
            .iter()
            .find(|entry| {
                entry.kind == CatalogEntryKind::ResourceMember && entry.path == "m::Book::title"
            })
            .expect("accepted title catalog id")
            .stable_id
            .clone(),
    )
    .expect("valid title catalog id");

    let store = TreeStore::memory();
    store
        .replace_catalog_snapshot(&accepted)
        .expect("write catalog snapshot");
    store
        .write_record_presence(&store_id, &[SavedKey::Int(7)])
        .expect("seed record presence");
    let long_title = "a".repeat(MAX_VALUE_PREVIEW_LIMIT + 128);
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(7)],
            &[StoreDataPathSegment::Member(title_id)],
            encode_value(&SavedValue::Str(long_title)).expect("encode title"),
        )
        .expect("seed title");

    let stamped_roots =
        stamped_data_roots_in_store(&snapshot.program, &store).expect("stamped roots read");
    let path = resolve_data_path(
        &snapshot.program,
        &[
            DataPathSegment::Root("books".to_string()),
            DataPathSegment::Key(SavedKey::Int(7)),
            DataPathSegment::Field("title".to_string()),
        ],
    )
    .expect("valid data path")
    .expect("accepted data path");
    let stamped = stamped_preview_data_path(&snapshot.program, &store, &path, 24)
        .expect("stamped preview read");
    let preview = stamped.data.preview.expect("value preview");

    assert_eq!(stamped.data.presence, DataPresence::ValueOnly);
    assert!(preview.truncated, "{preview:?}");
    assert!(preview.text.len() <= 24 + "...".len(), "{preview:?}");
    assert!(preview.text.starts_with('"'), "{preview:?}");
    assert!(preview.text.ends_with("..."), "{preview:?}");
    assert_eq!(stamped.stamp, stamped_roots.stamp);

    let clamped = stamped_preview_data_path(&snapshot.program, &store, &path, usize::MAX)
        .expect("clamped preview read");
    let clamped_preview = clamped.data.preview.expect("clamped value preview");

    assert_eq!(clamped.data.presence, DataPresence::ValueOnly);
    assert!(clamped_preview.truncated, "{clamped_preview:?}");
    assert!(
        clamped_preview.text.len() <= MAX_VALUE_PREVIEW_LIMIT + "...".len(),
        "{clamped_preview:?}"
    );
    assert!(clamped_preview.text.ends_with("..."), "{clamped_preview:?}");
    assert_eq!(clamped.stamp, stamped_roots.stamp);
}

#[test]
fn integrity_problem_samples_share_budget_and_carry_snapshot_identity() {
    let source = "module m\n\
        resource Book\n    \
        required pages: int\n\
        store ^books(id: int): Book\n";
    let root = temp_root("analysis-integrity-problem-sample");
    write(&root, "src/m.mw", source);
    let (checked, program) = check_project(&root, &config()).expect("check source");
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    let accepted = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes a catalog");
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze accepted source");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let store_fact = snapshot
        .program
        .facts
        .stores()
        .iter()
        .find(|store| store.root == "books")
        .expect("books store");
    let store_id = CatalogId::new(
        snapshot
            .program
            .store_catalog_id(store_fact.id)
            .expect("books store catalog id"),
    )
    .expect("valid store catalog id");
    let pages_id = CatalogId::new(
        accepted
            .entries
            .iter()
            .find(|entry| {
                entry.kind == CatalogEntryKind::ResourceMember && entry.path == "m::Book::pages"
            })
            .expect("accepted pages catalog id")
            .stable_id
            .clone(),
    )
    .expect("valid pages catalog id");

    let empty_store = TreeStore::memory();
    let empty_count =
        sample_integrity_problems(&empty_store, &snapshot.program, 0).expect("empty count sample");
    let empty_details = sample_integrity_problem_details(&empty_store, &snapshot.program, 0)
        .expect("empty detail sample");
    assert_eq!(empty_count.items_checked, 0);
    assert_eq!(empty_count.problems, 0);
    assert!(!empty_count.truncated);
    assert_eq!(empty_details.items_checked, 0);
    assert!(empty_details.problems.is_empty());
    assert!(!empty_details.truncated);

    let store = TreeStore::memory();
    store
        .replace_catalog_snapshot(&accepted)
        .expect("write catalog snapshot");
    store
        .write_record_presence(&store_id, &[SavedKey::Int(1)])
        .expect("seed record presence");
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(1)],
            &[StoreDataPathSegment::Member(pages_id)],
            b"not-an-int".to_vec(),
        )
        .expect("seed invalid int");
    let uid = StoreUid::from_entropy_bytes([9; 16]);
    store.write_store_uid(&uid).expect("write store uid");
    let profile = EngineProfile::new(0);
    store
        .write_commit_metadata(&CommitMetadata {
            commit_id: 21,
            catalog_epoch: 4,
            layout_epoch: profile.layout_epoch(),
            source_digest:
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                    .to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: vec![store_id],
            changed_index_catalog_ids: Vec::new(),
        })
        .expect("write commit metadata");

    let zero_count =
        sample_integrity_problems(&store, &snapshot.program, 0).expect("zero count sample");
    let zero_details =
        sample_integrity_problem_details(&store, &snapshot.program, 0).expect("zero detail sample");
    assert_eq!(zero_count.items_checked, 0);
    assert_eq!(zero_count.problems, 0);
    assert!(zero_count.truncated);
    assert_eq!(zero_details.items_checked, 0);
    assert!(zero_details.problems.is_empty());
    assert!(zero_details.truncated);

    let details =
        sample_integrity_problem_details(&store, &snapshot.program, 10).expect("detail sample");
    assert_eq!(details.problems.len(), 1);
    assert_eq!(details.problems[0].code, "data.decode");
    assert!(details.items_checked >= details.problems.len());
    assert!(details.items_checked <= 10);
    assert!(!details.truncated);

    let stamped = stamped_integrity_problem_details(&snapshot.program, &store, 10)
        .expect("stamped detail sample");
    assert_eq!(stamped.data.problems.len(), 1);
    assert_eq!(stamped.data.problems[0].code, "data.decode");
    assert_eq!(stamped.stamp.store_uid.as_ref(), Some(&uid));
    assert_eq!(
        stamped.stamp.store_catalog_digest.as_deref(),
        Some(accepted.digest.as_str())
    );
    assert_eq!(
        stamped.stamp.checked_source_digest,
        snapshot.program.source_digest()
    );
    assert_eq!(
        stamped.stamp.store_commit.expect("commit stamp").commit_id,
        21
    );
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
        seed_backup_sample_leaf(&store, format!("cat_{n:032x}"), n);
    }
    write_minimal_backup_archive(&archive, &store);

    let facts = marrow_check::evolution::evolution_preview(&snapshot, Some(&archive))
        .expect("backup evolution preview");
    let backup = facts.backup.expect("backup facts");

    assert_eq!(backup.cell_count, 17);
    let mut expected_samples = vec![
        "cat_00000000000000000000000000000000".to_string(),
        BACKUP_SAMPLE_MEMBER_ID.to_string(),
    ];
    expected_samples.extend((1..=14).map(|n| format!("cat_{n:032x}")));
    assert_eq!(backup.sample_catalog_ids, expected_samples);
    assert!(backup.samples_truncated);
}

fn seed_backup_sample_leaf(store: &TreeStore, catalog_id: impl Into<String>, id: i64) {
    let store_id = CatalogId::new(catalog_id).expect("store id");
    let member_id = CatalogId::new(BACKUP_SAMPLE_MEMBER_ID).expect("member id");
    store
        .write_leaf(
            &store_id,
            &[SavedKey::Int(id)],
            &member_id,
            b"sample".to_vec(),
        )
        .expect("seed backup cell");
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
