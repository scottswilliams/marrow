//! `type_at`/`scope_at`: position→type and visible-bindings for editor tooling,
//! reconstructing the cursor's lexical scope exactly as the checker does and
//! emitting no diagnostics of their own.
use crate::support;
use std::{fs, path::PathBuf};

use marrow_check::program::MarrowType;
use marrow_check::test_support::{member_catalog_id, root_place, store_id_of};
use marrow_check::tooling::{
    ActiveCallableContext, CallableArgumentStyle, CallableCalleeContext, CallableParameter,
    CallableSignature, CallableSignatureKind, CallableValueShape, DataChild, DataChildView,
    DataPathError, DataPathSegment, DataPresence, DeclaredDataChild, DeclaredDataChildKind,
    DeclaredDataKeyParam, IdentityTypeAnnotation, MAX_VALUE_PREVIEW_LIMIT, MemberFlavor,
    ResourceConstructorField, SavedDataPathSegment, SourceDataPathSegment,
    SourceSavedPathCompletionSegment, ToolingError, active_callable_context,
    callable_callee_contexts, declared_data_children, declared_source_data_children,
    identity_type_annotations, intrinsic_callable_signature, intrinsic_callable_signature_for_file,
    intrinsic_completion_callables, resolve_data_path, resolve_saved_data_path,
    resource_constructor_signature, sample_integrity_problem_details, sample_integrity_problems,
    source_saved_path_completion_fact_at, stamped_data_children, stamped_data_roots_in_store,
    stamped_integrity_problem_details, stamped_preview_data_path, stamped_read_data_path,
    stamped_saved_data_child_views, stamped_saved_data_root_views_in_store,
};
use marrow_check::{
    CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT, CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP,
    CHECK_READ_ONLY_EXPRESSION_WRITE, CatalogEntryKind, CatalogLifecycle, CheckedProgram,
    DebugExpressionDataAccess, DiagnosticPayload, EntryStoreOpenMode, ProjectSources,
    StoreLeafKind, SurfaceCatalogBlocker, SurfaceCatalogStatus, SurfaceReadFootprint,
    SurfaceReadOperationKind, UseSiteKind, WorkShapeClass, analyze_project, check_project,
    scope_at, type_at,
};
use marrow_project::parse_config;
use marrow_schema::{SCHEMA_DUPLICATE_MEMBER, ScalarType, Type};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{
    CommitMetadata, DataPathSegment as StoreDataPathSegment, EngineProfile, StoreUid, TreeStore,
    write_tree_backup_archive_chunk, write_tree_backup_archive_header,
};
use marrow_store::value::{SavedValue, ScalarType as StoreScalarType, encode_value};
use marrow_syntax::{ParsedSource, SourceSpan};

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

fn stop_span(source: &str, needle: &str) -> SourceSpan {
    let start_byte = source.find(needle).expect("stop marker is present");
    let before = &source[..start_byte];
    SourceSpan {
        start_byte,
        end_byte: start_byte + needle.len(),
        line: before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1,
        column: before
            .rsplit_once('\n')
            .map_or(before.len(), |(_, line)| line.len()) as u32
            + 1,
    }
}

fn required_string_child(name: &str) -> DeclaredDataChild {
    DeclaredDataChild {
        name: name.to_string(),
        kind: DeclaredDataChildKind::Field { required: true },
        key_params: Vec::new(),
        leaf: Some(StoreLeafKind::Scalar(StoreScalarType::Str)),
    }
}

#[test]
fn entry_run_facts_resolve_single_entry_runtime_shape() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        pub fn title(id: int): string\n    \
        return ^books(id).title ?? \"\"\n";
    let root = temp_root("entry-run-facts");
    write(&root, "src/m.mw", source);
    let (checked, program) = check_project(&root, &config()).expect("check source");
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    let accepted = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes a catalog");
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
    .expect("analyze accepted source");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let facts = snapshot
        .program
        .entry_run_facts("m::title")
        .expect("entry run facts");

    assert_eq!(facts.store_open_mode, EntryStoreOpenMode::ReadOnly);
    let footprint = facts.footprint;
    assert_eq!(footprint.entry, "m::title");
    let stores_read = footprint
        .stores_read
        .iter()
        .map(|store| snapshot.program.store_structural_path(*store))
        .collect::<Option<Vec<_>>>()
        .expect("store paths");
    assert_eq!(stores_read, vec!["m::^books"]);
    assert!(footprint.stores_written.is_empty());
    assert!(footprint.indexes_touched.is_empty());
    assert_eq!(footprint.work_shape, WorkShapeClass::ReadOnly);
    let cost_shape = facts.cost_shape;
    assert_eq!(cost_shape.entry, "m::title");
    assert_eq!(cost_shape.work_shape, WorkShapeClass::ReadOnly);
    assert_eq!(cost_shape.point_reads, 1);
    assert_eq!(cost_shape.range_scans, 0);
    assert_eq!(cost_shape.writes, 0);
    assert_eq!(cost_shape.index_entry_touches, 0);
    assert_eq!(cost_shape.commit_points, 0);
}

fn declared_receiver_children(
    test_name: &str,
    source: &str,
    receiver: &str,
    scope_marker: &str,
) -> Vec<DeclaredDataChild> {
    let source = source.replacen(
        scope_marker,
        &format!("const __completion = {receiver}.|candidate"),
        1,
    );
    saved_path_completion_children_at_cursor(test_name, &source)
}

fn saved_path_completion_children_at_cursor(
    test_name: &str,
    source: &str,
) -> Vec<DeclaredDataChild> {
    let offset = source.find('|').expect("cursor marker");
    let source = source.replacen('|', "", 1);
    let (snapshot, paths) = analyze_overlay(test_name, &[("src/m.mw", &source)]);
    let path = paths.into_iter().next().expect("source path");
    let parsed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("analyzed source")
        .parsed
        .clone();

    let lexed = marrow_syntax::lex_source(&source);
    source_saved_path_completion_fact_at(&snapshot.program, &path, &source, &parsed, &lexed, offset)
        .map(|fact| fact.children)
        .unwrap_or_default()
}

fn saved_path_completion_children_at_error_cursor(
    test_name: &str,
    source: &str,
) -> Vec<DeclaredDataChild> {
    let offset = source.find('|').expect("cursor marker");
    let source = source.replacen('|', "", 1);
    let (snapshot, paths) = analyze_overlay(test_name, &[("src/m.mw", &source)]);
    assert!(
        snapshot.report.has_errors(),
        "source should not type-check cleanly"
    );
    let path = paths.into_iter().next().expect("source path");
    let parsed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("analyzed source")
        .parsed
        .clone();
    let lexed = marrow_syntax::lex_source(&source);
    source_saved_path_completion_fact_at(&snapshot.program, &path, &source, &parsed, &lexed, offset)
        .map(|fact| fact.children)
        .unwrap_or_default()
}

fn path_segments(segments: &[&str]) -> Vec<String> {
    segments.iter().map(|segment| segment.to_string()).collect()
}

fn active_context_at_marker(source: &str) -> Option<ActiveCallableContext> {
    let offset = source.find('|').expect("cursor marker is present");
    let source = source.replacen('|', "", 1);
    let lexed = marrow_syntax::lex_source(&source);
    let parsed = marrow_syntax::parse_source(&source);
    active_callable_context(&source, &lexed, &parsed, offset)
}

fn callable_callees(source: &str) -> Vec<CallableCalleeContext> {
    let lexed = marrow_syntax::lex_source(source);
    let parsed = marrow_syntax::parse_source(source);
    callable_callee_contexts(source, &lexed, &parsed)
}

fn identity_type_annotation_facts(test_name: &str, source: &str) -> Vec<IdentityTypeAnnotation> {
    let (snapshot, paths) = analyze_overlay(test_name, &[("src/m.mw", source)]);
    let path = paths.into_iter().next().expect("source path");
    identity_type_annotations(&snapshot, &path)
}

#[test]
fn callable_callee_contexts_reports_expression_callees() {
    let source = "module app\nuse std::text\nfn run(): int\n    return outer(text::length(\"abc\"), Id(^books, 1))\n";
    let contexts = callable_callees(source);
    let paths = contexts
        .iter()
        .map(|context| context.callee_path_segments.clone())
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![
            path_segments(&["outer"]),
            path_segments(&["text", "length"]),
            path_segments(&["Id"]),
        ]
    );
    assert_eq!(
        &source[contexts[1].callee_span.start_byte..contexts[1].callee_span.end_byte],
        "text::length"
    );
    assert_eq!(
        &source[contexts[1].callee_leaf_span.start_byte..contexts[1].callee_leaf_span.end_byte],
        "length"
    );
}

#[test]
fn callable_callee_contexts_reports_nested_named_argument_values() {
    let source = "module app\nfn run()\n    return Outer(arg: Book(field: Id(^books, 1)))\n";
    let contexts = callable_callees(source);

    assert_eq!(
        contexts
            .iter()
            .map(|context| context.callee_path_segments.clone())
            .collect::<Vec<_>>(),
        vec![
            path_segments(&["Outer"]),
            path_segments(&["Book"]),
            path_segments(&["Id"]),
        ]
    );
}

#[test]
fn callable_callee_contexts_reports_named_argument_values_under_member_calls() {
    let source = "module app\nfn run(book: Book)\n    return book.items(id: Id(^books, 1))\n";
    let contexts = callable_callees(source);

    assert_eq!(
        contexts
            .iter()
            .map(|context| context.callee_path_segments.clone())
            .collect::<Vec<_>>(),
        vec![path_segments(&["Id"])]
    );
}

#[test]
fn callable_callee_contexts_reports_incomplete_const_initializer_call() {
    let source = "module app\nconst DEFAULT = Book(^books, 1\n";
    let contexts = callable_callees(source);

    assert_eq!(
        contexts
            .iter()
            .map(|context| context.callee_path_segments.clone())
            .collect::<Vec<_>>(),
        vec![path_segments(&["Book"])]
    );
}

#[test]
fn callable_callee_contexts_ignores_type_annotations() {
    let source = "module app\nstore ^authors(id: int): Author\npub fn run(\n    ids: sequence[Id(^authors)],\n): sequence[Id(^authors)]\n    return Id(^authors, 1)\n";
    let contexts = callable_callees(source);

    assert_eq!(
        contexts
            .iter()
            .map(|context| context.callee_path_segments.clone())
            .collect::<Vec<_>>(),
        vec![path_segments(&["Id"])]
    );
    assert_eq!(
        &source[contexts[0].callee_span.start_byte..contexts[0].callee_span.end_byte],
        "Id"
    );
}

#[test]
fn callable_callee_contexts_handles_multiline_nested_callees() {
    let depth = 64;
    let mut source = "module app\nfn run()\n    return ".to_string();
    for index in 0..depth {
        source.push_str(&format!("f{index}(\n        "));
    }
    source.push('0');
    for _ in 0..depth {
        source.push(')');
    }
    source.push('\n');

    let contexts = callable_callees(&source);

    assert_eq!(contexts.len(), depth);
    assert_eq!(contexts[0].callee_path_segments, path_segments(&["f0"]));
    assert_eq!(
        contexts[depth - 1].callee_path_segments,
        path_segments(&[&format!("f{}", depth - 1)])
    );
}

#[test]
fn callable_callee_contexts_handles_deeply_nested_callees() {
    let depth = 64;
    let mut source = "module app\nfn run()\n    return ".to_string();
    for index in 0..depth {
        source.push_str(&format!("f{index}("));
    }
    source.push('0');
    for _ in 0..depth {
        source.push(')');
    }
    source.push('\n');

    let contexts = callable_callees(&source);

    assert_eq!(contexts.len(), depth);
    assert_eq!(contexts[0].callee_path_segments, path_segments(&["f0"]));
    assert_eq!(
        contexts[depth - 1].callee_path_segments,
        path_segments(&[&format!("f{}", depth - 1)])
    );
}

#[test]
fn callable_callee_contexts_does_not_rescan_matching_parens() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../marrow-syntax/src/active_call.rs");
    let source = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", path.display());
    });
    let start = source
        .find("fn looks_like_declaration_syntax_indexed")
        .expect("batch declaration classifier exists");
    let end = source[start..]
        .find("fn callee_segment_indices_before")
        .map(|offset| start + offset)
        .expect("next helper exists");
    let body = &source[start..end];

    assert!(
        !body.contains("key_list_has_type_suffix("),
        "batch callable callee collection must not rescan forward from each open paren"
    );
}

#[test]
fn identity_type_annotations_report_checked_identity_type_spans() {
    let source = "\
module m

resource Author
    name: string

store ^authors(id: int): Author

pub fn f(
    id: Id(^authors),
    ids: sequence[Id(^authors)],
): sequence[Id(^authors)]
    const direct: Id(^authors) = id
    var later: sequence[Id(^authors)]
    return ids
";
    let facts = identity_type_annotation_facts("analysis-identity-type-annotations", source);
    let observed = facts
        .iter()
        .map(|fact| {
            (
                span_text(source, fact.constructor_span),
                span_text(source, fact.root_span),
                fact.store_root.as_str(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        observed,
        vec![
            ("Id", "authors", "authors"),
            ("Id", "authors", "authors"),
            ("Id", "authors", "authors"),
            ("Id", "authors", "authors"),
            ("Id", "authors", "authors"),
        ]
    );
}

#[test]
fn identity_type_annotations_report_source_spans_with_annotation_whitespace() {
    let source = "\
module m

resource Author
    name: string

store ^authors(id: int): Author

fn f(
    id: Id( ^authors ),
    ids: sequence[ Id( ^authors ) ],
)
    return
";
    let facts =
        identity_type_annotation_facts("analysis-identity-type-annotations-whitespace", source);
    let observed = facts
        .iter()
        .map(|fact| {
            (
                span_text(source, fact.constructor_span),
                span_text(source, fact.root_span),
                fact.store_root.as_str(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        observed,
        vec![("Id", "authors", "authors"), ("Id", "authors", "authors"),]
    );
}

#[test]
fn identity_type_annotations_omit_unresolved_longer_and_expression_id_uses() {
    let source = "\
module m

resource Author
    name: string

store ^authors(id: int): Author

fn bad(
    missing: Id(^missing),
    longer: Id(^authors)::Extra,
)
    const expression = Id(^authors, 1)
    return
";
    let facts = identity_type_annotation_facts("analysis-identity-type-annotations-omit", source);

    assert!(facts.is_empty(), "{facts:?}");
}

#[test]
fn identity_type_annotations_omit_files_without_checked_modules() {
    let schema = "\
module schema

resource Author
    name: string

store ^authors(id: int): Author
";
    let broken = "\
module broken

fn f(id: Id(^authors))
    return

surface MissingBody from ^authors
";
    let (snapshot, paths) = analyze_overlay(
        "analysis-identity-type-annotations-unchecked-file",
        &[("src/schema.mw", schema), ("src/broken.mw", broken)],
    );
    assert!(
        snapshot.report.has_errors(),
        "fixture should leave the active file out of checked modules"
    );
    let broken_path = paths
        .into_iter()
        .find(|path| path.ends_with("broken.mw"))
        .expect("broken file path");

    let facts = identity_type_annotations(&snapshot, &broken_path);

    assert!(facts.is_empty(), "{facts:?}");
}

#[test]
fn active_callable_context_reports_positional_argument_index() {
    let context = active_context_at_marker("module app\nfn run()\n    return add(1, |\n")
        .expect("active call context");

    assert_eq!(context.callee_path_segments, path_segments(&["add"]));
    assert_eq!(context.active_argument, 1);
    assert_eq!(context.named_argument, None);
}

#[test]
fn active_callable_context_reports_named_argument() {
    let context = active_context_at_marker("module app\nfn run()\n    return Book(title: |\n")
        .expect("active call context");

    assert_eq!(context.callee_path_segments, path_segments(&["Book"]));
    assert_eq!(context.active_argument, 0);
    assert_eq!(context.named_argument, Some("title".to_string()));
}

#[test]
fn active_callable_context_recovers_multiline_named_argument() {
    let context =
        active_context_at_marker("module app\nfn run()\n    return Book(\n        title: |\n")
            .expect("active call context");

    assert_eq!(context.callee_path_segments, path_segments(&["Book"]));
    assert_eq!(context.active_argument, 0);
    assert_eq!(context.named_argument, Some("title".to_string()));
}

#[test]
fn active_callable_context_recovers_named_argument_after_trivia() {
    for source in [
        "module app\nfn run()\n    return Book(\n        ; doc\n        title: |\n",
        "module app\nfn run()\n    return Book(\n        ;; doc\n        title: |\n",
    ] {
        let context = active_context_at_marker(source).expect("active call context");

        assert_eq!(context.callee_path_segments, path_segments(&["Book"]));
        assert_eq!(context.active_argument, 0);
        assert_eq!(
            context.named_argument,
            Some("title".to_string()),
            "{source}"
        );
    }
}

#[test]
fn active_callable_context_uses_innermost_nested_call() {
    let context =
        active_context_at_marker("module app\nfn run()\n    return outer(inner(1, |), 3)\n")
            .expect("active call context");

    assert_eq!(context.callee_path_segments, path_segments(&["inner"]));
    assert_eq!(context.active_argument, 1);
    assert_eq!(context.named_argument, None);
}

#[test]
fn active_callable_context_recovers_keyword_headed_qualified_call() {
    let positional = active_context_at_marker(
        "module app\nuse std::bytes\nfn run(data: bytes)\n    return bytes::base64Encode(|\n",
    )
    .expect("active call context");
    assert_eq!(
        positional.callee_path_segments,
        path_segments(&["bytes", "base64Encode"])
    );
    assert_eq!(positional.active_argument, 0);
    assert_eq!(positional.named_argument, None);

    let named = active_context_at_marker(
        "module app\nuse std::bytes\nfn run(data: bytes)\n    return bytes::base64Encode(value: |\n",
    )
    .expect("active call context");
    assert_eq!(
        named.callee_path_segments,
        path_segments(&["bytes", "base64Encode"])
    );
    assert_eq!(named.active_argument, 0);
    assert_eq!(named.named_argument, Some("value".to_string()));

    let nested = active_context_at_marker(
        "module app\nuse std::bytes\nfn run(data: bytes)\n    return outer(bytes::base64Encode(|))\n",
    )
    .expect("active call context");
    assert_eq!(
        nested.callee_path_segments,
        path_segments(&["bytes", "base64Encode"])
    );
    assert_eq!(nested.active_argument, 0);
    assert_eq!(nested.named_argument, None);
}

#[test]
fn active_callable_context_ignores_declarations_types_and_member_keys() {
    for source in [
        "resource Counter\n    count(|): string\n",
        "fn run(value: int(|\n",
        "fn run(|\n",
        "resource Book(|\n",
        "module app(|\n",
        "module app\nuse library(|\n",
        "module app\nuse library::books(|\n",
        "module app\nfn run(): int\n    var count(|\n",
        "module app\nfn run(): int\n    var int(|\n",
        "module app\nstore ^authors(id: int): Author\npub fn run(\n    ids: sequence[Id(|^authors)],\n): sequence[Id(^authors)]\n    return ids\n",
        "module app\nfn run(): int\n    const count(|\n",
        "module app\nfn run(): int\n    const int(|\n",
        "module app\nstore ^authors(id: int): Author\npub fn run(\n    ids: sequence[Id(^authors)],\n): sequence[Id(|^authors)]\n    return ids\n",
        "surface app(|\n",
        "resource Book\n    author: string\nstore ^books(id: int): Book\n    index by author::name(|\n",
    ] {
        assert_eq!(active_context_at_marker(source), None, "{source}");
    }
}

#[test]
fn active_callable_context_requires_adjacent_expression_callee() {
    for source in [
        "module app\nfn run()\n    return foo\n\n    (|\n",
        "module app\nfn run()\n    return foo ;; trailing doc\n    (|\n",
        "module app\nfn run()\n    return (|\n",
        "module app\nfn run()\n    if (|\n",
        "module app\nfn run()\n    return std::if::foo(|\n",
    ] {
        assert_eq!(active_context_at_marker(source), None, "{source}");
    }
}

#[test]
fn active_callable_context_rejects_postfix_member_calls() {
    for source in [
        "module app\nfn run(book: Book)\n    return book.title(|\n",
        "module app\nfn run(book: Book)\n    return book?.title(|\n",
        "module app\nfn run(book: Book)\n    return book.items(id: |\n",
    ] {
        assert_eq!(active_context_at_marker(source), None, "{source}");
    }
}

#[test]
fn active_callable_context_ignores_enum_member_declarations() {
    for source in [
        "enum Status\n    active(|\n",
        "enum Status\n    category active(|\n",
    ] {
        assert_eq!(active_context_at_marker(source), None, "{source}");
    }
}

#[test]
fn active_callable_context_ignores_non_expression_path_prefixes_and_headers() {
    for source in [
        "module app\nfn run()\n    return ^books(id: |\n",
        "surface app from ^books\n    action publish(| as publish\n",
        "module app\nfn run(status: Status)\n    match status\n        active(|\n",
        "module app\nevolve\n    rename Book(| to Volume\n",
        "module app\nevolve\n    rename Book(|) -> Volume\n",
        "module app\nevolve\n    rename Book -> Volume(|)\n",
        "module app\nevolve\n    default Book(|) = 1\n",
        "module app\nevolve\n    retire Book(|)\n",
        "module app\nevolve\n    transform Book(|)\n        return old\n",
        "module app\nevolve\n    transform Book(|\n        return old\n",
    ] {
        assert_eq!(active_context_at_marker(source), None, "{source}");
    }

    let context = active_context_at_marker("module app\nfn run()\n    return add(|\n")
        .expect("expression calls still report an active context");
    assert_eq!(context.callee_path_segments, path_segments(&["add"]));
}

#[test]
fn active_callable_context_reports_evolve_expression_calls() {
    for source in [
        "module app\nevolve\n    default Book.title = Book(|\n",
        "module app\nevolve\n    transform Book.title\n        return Book(|\n",
    ] {
        let context = active_context_at_marker(source).expect("active call context");

        assert_eq!(context.callee_path_segments, path_segments(&["Book"]));
        assert_eq!(context.active_argument, 0);
        assert_eq!(context.named_argument, None);
    }
}

#[test]
fn active_callable_context_ignores_surface_item_name_lists() {
    for source in [
        "module app\nsurface Books from ^books\n    fields title(|\n",
        "module app\nsurface Books from ^books\n    create title(|\n",
        "module app\nsurface Books from ^books\n    update title(|\n",
    ] {
        assert_eq!(active_context_at_marker(source), None, "{source}");
    }
}

#[test]
fn active_callable_context_ignores_surface_collection_alias_syntax() {
    assert_eq!(
        active_context_at_marker(
            "module app\nsurface Books from ^books\n    collection ^books as books(|\n"
        ),
        None
    );
}

#[test]
fn active_callable_context_reports_const_initializer_call() {
    let context = active_context_at_marker("module app\nconst DEFAULT = Book(|\n")
        .expect("active call context");

    assert_eq!(context.callee_path_segments, path_segments(&["Book"]));
    assert_eq!(context.active_argument, 0);
    assert_eq!(context.named_argument, None);
}

#[test]
fn resource_constructor_signature_resolves_imported_qualified_resource_fields() {
    let books = "module library::books\n\
        enum Status\n    \
        active\n\
        ;; Book records.\n\
        resource Book\n    \
        ;; Display title.\n    \
        required title: string\n    \
        ;; Lifecycle state.\n    \
        status: Status\n    \
        tags(pos: int): string\n    \
        editions(version: int)\n        \
        required isbn: string\n\
        store ^books(id: int): Book\n";
    let app = "module app\nuse library::books\nfn make()\n    return\n";
    let (snapshot, paths) = analyze_overlay(
        "resource-constructor-signature-qualified",
        &[("src/library/books.mw", books), ("src/app.mw", app)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let signature = resource_constructor_signature(
        &snapshot.program,
        &paths[1],
        &path_segments(&["books", "Book"]),
    )
    .expect("imported resource constructor signature");

    assert_eq!(signature.name, "Book");
    assert_eq!(
        signature.ty,
        MarrowType::Resource(support::resource_id(
            &snapshot.program,
            "library::books",
            "Book"
        ))
    );
    assert_eq!(signature.docs, ["Book records."]);
    assert_eq!(
        signature.fields,
        [
            ResourceConstructorField {
                name: "title".to_string(),
                required: true,
                ty: MarrowType::Primitive(ScalarType::Str),
                docs: vec!["Display title.".to_string()],
            },
            ResourceConstructorField {
                name: "status".to_string(),
                required: false,
                ty: MarrowType::Enum(support::enum_id(
                    &snapshot.program,
                    "library::books",
                    "Status",
                )),
                docs: vec!["Lifecycle state.".to_string()],
            },
        ]
    );

    let bare =
        resource_constructor_signature(&snapshot.program, &paths[0], &path_segments(&["Book"]))
            .expect("same-module bare resource constructor signature");
    assert_eq!(bare, signature);

    let fully_qualified = resource_constructor_signature(
        &snapshot.program,
        &paths[1],
        &path_segments(&["library", "books", "Book"]),
    )
    .expect("fully qualified resource constructor signature");
    assert_eq!(fully_qualified, signature);
}

#[test]
fn resource_constructor_signature_returns_none_for_non_resource_or_unresolved_path() {
    let source = "module app\npub fn make(): int\n    return 1\n";
    let (snapshot, paths) = analyze_overlay(
        "resource-constructor-signature-none",
        &[("src/app.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    assert_eq!(
        resource_constructor_signature(&snapshot.program, &paths[0], &path_segments(&["make"])),
        None
    );
    assert_eq!(
        resource_constructor_signature(&snapshot.program, &paths[0], &path_segments(&["Missing"])),
        None
    );
}

#[test]
fn resource_constructor_signature_fails_closed_for_ambiguous_bare_foreign_resource() {
    let (snapshot, paths) = analyze_overlay(
        "resource-constructor-signature-ambiguous-bare",
        &[
            ("src/a.mw", "module a\nresource Book\n    title: string\n"),
            ("src/b.mw", "module b\nresource Book\n    pages: int\n"),
            ("src/app.mw", "module app\nfn make()\n    return\n"),
        ],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    assert_eq!(
        resource_constructor_signature(&snapshot.program, &paths[2], &path_segments(&["Book"])),
        None
    );
}

#[test]
fn intrinsic_callable_signature_returns_builtin_shapes_without_stale_removed_builtins() {
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["count"])),
        Some(CallableSignature {
            path: path_segments(&["count"]),
            kind: CallableSignatureKind::Builtin,
            argument_style: CallableArgumentStyle::Positional,
            docs: vec![
                "Returns child count for a saved path, 1 for a scalar, or 0 when absent."
                    .to_string()
            ],
            params: vec![CallableParameter {
                label: "collection".to_string(),
                required: true,
                repeat: false,
                shape: CallableValueShape::Collection,
                docs: Vec::new(),
            }],
            return_shape: Some(CallableValueShape::Type(MarrowType::Primitive(
                ScalarType::Int
            ))),
        })
    );

    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["nextId"]))
            .expect("nextId signature")
            .return_shape,
        Some(CallableValueShape::Identity)
    );
    let append =
        intrinsic_callable_signature(&path_segments(&["append"])).expect("append signature");
    assert_eq!(
        append
            .params
            .iter()
            .map(|param| &param.shape)
            .collect::<Vec<_>>(),
        [&CallableValueShape::SavedLayer, &CallableValueShape::Value,]
    );
    assert_eq!(append.argument_style, CallableArgumentStyle::Positional);
    assert_eq!(
        append.return_shape,
        Some(CallableValueShape::Type(MarrowType::Primitive(
            ScalarType::Int
        )))
    );
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["print"]))
            .expect("print signature")
            .return_shape,
        None
    );
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["write"])),
        None
    );
}

#[test]
fn intrinsic_completion_callables_enumerate_bare_callable_facts() {
    let callables = intrinsic_completion_callables();
    let labels = callables
        .iter()
        .map(|signature| signature.path.join("::"))
        .collect::<Vec<_>>();
    let expected_labels = [
        "print",
        "exists",
        "nextId",
        "append",
        "keys",
        "count",
        "values",
        "next",
        "prev",
        "key",
        "bool",
        "int",
        "string",
        "ErrorCode",
        "bytes",
        "date",
        "instant",
        "duration",
        "decimal",
        "Id",
        "Error",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();

    assert_eq!(labels, expected_labels);
    assert_eq!(
        labels
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        labels.len(),
        "completion callable labels must be unique: {labels:?}"
    );
    assert!(
        !labels.contains(&"write".to_string()),
        "completion callables must not resurrect removed builtins: {labels:?}"
    );
    assert!(
        !labels.iter().any(|label| label.starts_with("std::")),
        "bare completion callables must not enumerate namespace-qualified std operations: {labels:?}"
    );
    for signature in callables {
        assert_eq!(
            intrinsic_callable_signature(&signature.path),
            Some(signature.clone()),
            "enumerated callable must round-trip through lookup"
        );
    }
}

#[test]
fn intrinsic_callable_signature_returns_identity_constructor_shape() {
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["Id"])),
        Some(CallableSignature {
            path: path_segments(&["Id"]),
            kind: CallableSignatureKind::IdentityConstructor,
            argument_style: CallableArgumentStyle::Positional,
            docs: Vec::new(),
            params: vec![
                CallableParameter {
                    label: "root".to_string(),
                    required: true,
                    repeat: false,
                    shape: CallableValueShape::SavedRoot,
                    docs: Vec::new(),
                },
                CallableParameter {
                    label: "key".to_string(),
                    required: true,
                    repeat: true,
                    shape: CallableValueShape::Value,
                    docs: Vec::new(),
                },
            ],
            return_shape: Some(CallableValueShape::Identity),
        })
    );
}

#[test]
fn intrinsic_callable_signature_returns_conversion_shapes_with_error_code_identity() {
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["int"])),
        Some(CallableSignature {
            path: path_segments(&["int"]),
            kind: CallableSignatureKind::ScalarConversion,
            argument_style: CallableArgumentStyle::Positional,
            docs: Vec::new(),
            params: vec![CallableParameter {
                label: "value".to_string(),
                required: true,
                repeat: false,
                shape: CallableValueShape::Value,
                docs: Vec::new(),
            }],
            return_shape: Some(CallableValueShape::Type(MarrowType::Primitive(
                ScalarType::Int
            ))),
        })
    );

    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["ErrorCode"]))
            .expect("ErrorCode signature")
            .return_shape,
        Some(CallableValueShape::ErrorCode)
    );
}

#[test]
fn intrinsic_callable_signature_returns_standard_library_shapes() {
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["std", "text", "contains"])),
        Some(CallableSignature {
            path: path_segments(&["std", "text", "contains"]),
            kind: CallableSignatureKind::StandardLibrary,
            argument_style: CallableArgumentStyle::Positional,
            docs: Vec::new(),
            params: vec![
                CallableParameter {
                    label: "string".to_string(),
                    required: true,
                    repeat: false,
                    shape: CallableValueShape::Type(MarrowType::Primitive(ScalarType::Str)),
                    docs: Vec::new(),
                },
                CallableParameter {
                    label: "string".to_string(),
                    required: true,
                    repeat: false,
                    shape: CallableValueShape::Type(MarrowType::Primitive(ScalarType::Str)),
                    docs: Vec::new(),
                },
            ],
            return_shape: Some(CallableValueShape::Type(MarrowType::Primitive(
                ScalarType::Bool
            ))),
        })
    );

    let absent = intrinsic_callable_signature(&path_segments(&["std", "assert", "isAbsent"]))
        .expect("std::assert::isAbsent signature");
    assert_eq!(absent.params[0].shape, CallableValueShape::SavedPath);
    assert_eq!(absent.return_shape, None);
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["std", "text", "missing"])),
        None
    );
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["foo", "std", "text", "contains"])),
        None
    );
}

#[test]
fn intrinsic_callable_signature_for_file_expands_import_aliases() {
    let text =
        "module app\nuse std::text\nfn run(): bool\n    return text::contains(\"abc\", \"b\")\n";
    let clock = "module clock_user\nuse std::clock\nfn run(): instant\n    return clock::now()\n";
    let (snapshot, paths) = analyze_overlay(
        "intrinsic-callable-signature-import-aliases",
        &[("src/app.mw", text), ("src/clock_user.mw", clock)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let imported_text = intrinsic_callable_signature_for_file(
        &snapshot,
        &paths[0],
        &path_segments(&["text", "contains"]),
    )
    .expect("imported std text operation");
    let canonical_text = intrinsic_callable_signature(&path_segments(&["std", "text", "contains"]))
        .expect("canonical std text operation");
    assert_eq!(imported_text, canonical_text);

    let imported_clock = intrinsic_callable_signature_for_file(
        &snapshot,
        &paths[1],
        &path_segments(&["clock", "now"]),
    )
    .expect("imported std clock operation");
    assert_eq!(
        imported_clock,
        intrinsic_callable_signature(&path_segments(&["std", "clock", "now"]))
            .expect("canonical std clock operation")
    );

    assert_eq!(
        intrinsic_callable_signature_for_file(
            &snapshot,
            &paths[0],
            &path_segments(&["clock", "now"]),
        ),
        None
    );
}

#[test]
fn intrinsic_callable_signature_for_file_fails_closed_for_ambiguous_alias_heads() {
    let duplicate_alias = "module app\nuse text\nuse std::text\nfn run(): bool\n    return true\n";
    let (duplicate_snapshot, duplicate_paths) = analyze_overlay(
        "intrinsic-callable-signature-duplicate-alias",
        &[
            ("src/app.mw", duplicate_alias),
            ("src/text.mw", "module text\n"),
        ],
    );
    assert!(
        duplicate_snapshot.report.has_errors(),
        "the duplicate import fixture should be rejected"
    );
    assert_eq!(
        intrinsic_callable_signature_for_file(
            &duplicate_snapshot,
            &duplicate_paths[0],
            &path_segments(&["text", "contains"]),
        ),
        None
    );

    let declaration_collision = "module app\nuse std::text\nfn text(): bool\n    return true\n\nfn run(): bool\n    return true\n";
    let (collision_snapshot, collision_paths) = analyze_overlay(
        "intrinsic-callable-signature-declaration-collision",
        &[("src/app.mw", declaration_collision)],
    );
    assert!(
        collision_snapshot.report.has_errors(),
        "the import and function collision fixture should be rejected"
    );
    assert_eq!(
        intrinsic_callable_signature_for_file(
            &collision_snapshot,
            &collision_paths[0],
            &path_segments(&["text", "contains"]),
        ),
        None
    );

    let surface_collision = "module app\n\
        use std::text\n\
        resource Book\n    \
        title: string\n\
        store ^books(id: int): Book\n\
        surface text from ^books\n    \
        fields title\n\
        fn run(): bool\n    \
        return true\n";
    let (surface_snapshot, surface_paths) = analyze_overlay(
        "intrinsic-callable-signature-surface-collision",
        &[("src/app.mw", surface_collision)],
    );
    assert!(
        surface_snapshot.report.has_errors(),
        "the import and surface collision fixture should be rejected"
    );
    assert_eq!(
        intrinsic_callable_signature_for_file(
            &surface_snapshot,
            &surface_paths[0],
            &path_segments(&["text", "contains"]),
        ),
        None
    );

    let clean_import =
        "module app\nuse std::text\nfn run(): bool\n    return text::contains(\"abc\", \"b\")\n";
    let (clean_snapshot, clean_paths) = analyze_overlay(
        "intrinsic-callable-signature-clean-import",
        &[("src/app.mw", clean_import)],
    );
    assert!(
        !clean_snapshot.report.has_errors(),
        "{:#?}",
        clean_snapshot.report.diagnostics
    );
    assert_eq!(
        intrinsic_callable_signature_for_file(
            &clean_snapshot,
            &clean_paths[0],
            &path_segments(&["text", "contains"]),
        ),
        intrinsic_callable_signature(&path_segments(&["std", "text", "contains"]))
    );
}

#[test]
fn intrinsic_callable_signature_returns_error_constructor_shape() {
    assert_eq!(
        intrinsic_callable_signature(&path_segments(&["Error"])),
        Some(CallableSignature {
            path: path_segments(&["Error"]),
            kind: CallableSignatureKind::ErrorConstructor,
            argument_style: CallableArgumentStyle::NamedFields,
            docs: Vec::new(),
            params: vec![
                CallableParameter {
                    label: "code".to_string(),
                    required: true,
                    repeat: false,
                    shape: CallableValueShape::ErrorCode,
                    docs: Vec::new(),
                },
                CallableParameter {
                    label: "message".to_string(),
                    required: true,
                    repeat: false,
                    shape: CallableValueShape::Type(MarrowType::Primitive(ScalarType::Str)),
                    docs: Vec::new(),
                },
                CallableParameter {
                    label: "help".to_string(),
                    required: false,
                    repeat: false,
                    shape: CallableValueShape::Type(MarrowType::Primitive(ScalarType::Str)),
                    docs: Vec::new(),
                },
                CallableParameter {
                    label: "data".to_string(),
                    required: false,
                    repeat: false,
                    shape: CallableValueShape::Type(MarrowType::Unknown),
                    docs: Vec::new(),
                },
            ],
            return_shape: Some(CallableValueShape::Type(MarrowType::Error)),
        })
    );
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
fn type_at_a_saved_field_read_is_the_optional_leaf_type() {
    // `^books(id).title` reads a `string` field through a maybe-present record, so
    // the read types as `string?`. Typing it requires both the saved-data machinery
    // and the `id` parameter in scope.
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn peek(id: int): string\n    \
        return ^books(id).title ?? \"\"\n";
    let (program, parsed, path) = analyze("type-at-saved-field", source);
    // Point at the `.title` leaf, so the smallest covering expression is the whole
    // field read `^books(id).title` rather than the inner `^books` root.
    let offset = source.rfind("title").expect("the .title field read") + 1;

    let ty = type_at(&program, &path, &parsed, offset);
    assert_eq!(
        ty,
        Some(MarrowType::Optional(Box::new(MarrowType::Primitive(
            ScalarType::Str
        )))),
        "{ty:?}"
    );
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
        Some(MarrowType::Identity(support::identity_root_id(
            &program, "books"
        ))),
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
fn scope_at_includes_a_loop_binding_typed_to_the_position() {
    // A `for` binding is in scope only within the loop body. A bare sequence loop
    // binds the 1-based `int` position — the same rule the checker applies.
    // Reconstructing the cursor scope must push that frame.
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
            resource: support::resource_id(&program, "m", "Book"),
            layers: support::group_entry_layers(&program, "m", "Book", &["versions"]),
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
fn checked_debug_expression_sees_prior_locals_and_excludes_the_current_statement_binding() {
    let source = "module m\n\
        fn f(input: int)\n    \
        const before = input + 1\n    \
        const current = before + input\n    \
        print(current)\n";
    let (snapshot, paths) =
        analyze_overlay("debug-expression-before-statement", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let path = paths.into_iter().next().expect("source path");
    let span = stop_span(source, "const current");

    let checked = snapshot
        .checked_debug_expression(&path, span, "before + input > 0")
        .expect("params and prior locals are visible before the current statement");
    assert_eq!(
        checked.ty(),
        &MarrowType::Primitive(ScalarType::Bool),
        "conditional debuggers can require bool without reinferring"
    );

    let diagnostics = snapshot
        .checked_debug_expression(&path, span, "current")
        .expect_err("the current statement binding is not visible before it runs");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == marrow_syntax::Severity::Error),
        "{diagnostics:#?}"
    );
}

#[test]
fn checked_debug_expression_replays_loop_shadowing_for_its_result_type() {
    let source = "module m\n\
        fn f(items: sequence[int])\n    \
        const item: string = \"outer\"\n    \
        for item in items\n        \
        print(item)\n";
    let (snapshot, paths) =
        analyze_overlay("debug-expression-loop-shadow", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let path = paths.into_iter().next().expect("source path");

    let checked = snapshot
        .checked_debug_expression(&path, stop_span(source, "print(item)"), "item > 1")
        .expect("loop binding shadows the outer local in the shared scope replay");
    assert_eq!(checked.ty(), &MarrowType::Primitive(ScalarType::Bool));
}

#[test]
fn checked_debug_expression_replays_if_const_binding() {
    let source = "module m\n\
        resource Book\n    \
        title: string\n\
        store ^books(id: int): Book\n\
        fn f(id: int)\n    \
        if const title = ^books(id).title\n        \
        print(title)\n";
    let (snapshot, paths) = analyze_overlay("debug-expression-if-const", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let path = paths.into_iter().next().expect("source path");

    let checked = snapshot
        .checked_debug_expression(
            &path,
            stop_span(source, "print(title)"),
            "title == \"Dune\"",
        )
        .expect("if const binding is visible inside the then block");
    assert_eq!(checked.ty(), &MarrowType::Primitive(ScalarType::Bool));
}

#[test]
fn checked_debug_expression_replays_match_arm_shadowing() {
    let source = "module m\n\
        enum Status\n    \
        active\n    \
        retired\n\
        fn f(state: Status)\n    \
        const value: string = \"outer\"\n    \
        match state\n        \
        active\n            \
        const value: int = 1\n            \
        print(value)\n        \
        retired\n            \
        print(0)\n";
    let (snapshot, paths) = analyze_overlay("debug-expression-match-arm", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let path = paths.into_iter().next().expect("source path");

    let checked = snapshot
        .checked_debug_expression(&path, stop_span(source, "print(value)"), "value > 0")
        .expect("match arm local shadows the outer binding");
    assert_eq!(checked.ty(), &MarrowType::Primitive(ScalarType::Bool));
}

#[test]
fn checked_debug_expression_reuses_read_only_effect_diagnostics() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n    \
        required shelf: string\n\
        store ^books(id: int): Book\n\
        fn f(id: int)\n    \
        const before = id\n    \
        print(before)\n";
    let (snapshot, paths) = analyze_overlay(
        "debug-expression-read-only-effects",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let path = paths.into_iter().next().expect("source path");
    let span = stop_span(source, "print(before)");

    let write = snapshot
        .checked_debug_expression(
            &path,
            span,
            "append(^books, Book(title: \"Dune\", shelf: \"sf\"))",
        )
        .expect_err("debug expressions cannot write saved data");
    assert!(
        write
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_WRITE),
        "{write:#?}"
    );

    let host = snapshot
        .checked_debug_expression(&path, span, "print(\"hello\")")
        .expect_err("debug expressions cannot call host-effecting operations");
    assert!(
        host.iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_HOST_EFFECT),
        "{host:#?}"
    );

    let unindexed = snapshot
        .checked_debug_expression(&path, span, "count(^books)")
        .expect_err("debug expressions cannot perform unindexed saved scans");
    assert!(
        unindexed
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_READ_ONLY_EXPRESSION_UNINDEXED_LOOKUP),
        "{unindexed:#?}"
    );
}

#[test]
fn checked_debug_expression_reports_durable_data_access() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n    \
        shelf: string\n\
        store ^books(id: int): Book\n    \
        index byShelf(shelf, id)\n\
        fn bookTitle(id: int): string\n    \
        return ^books(id).title ?? \"\"\n\
        fn f(id: int)\n    \
        const before = id + 1\n    \
        print(before)\n";
    let (snapshot, paths) =
        analyze_overlay("debug-expression-data-access", &[("src/m.mw", source)]);
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let path = paths.into_iter().next().expect("source path");
    let span = stop_span(source, "print(before)");

    let local = snapshot
        .checked_debug_expression(&path, span, "before > 0")
        .expect("local-only debug expression is admitted");
    assert_eq!(local.data_access(), DebugExpressionDataAccess::LocalOnly);

    let durable = snapshot
        .checked_debug_expression(&path, span, "^books(id).title")
        .expect("read-only durable debug expression is admitted with data-access fact");
    assert_eq!(
        durable.data_access(),
        DebugExpressionDataAccess::RequiresDurableData
    );

    let transitive = snapshot
        .checked_debug_expression(&path, span, "bookTitle(id) == \"Dune\"")
        .expect("helper-mediated durable debug expression is admitted with data-access fact");
    assert_eq!(
        transitive.data_access(),
        DebugExpressionDataAccess::RequiresDurableData
    );

    let indexed = snapshot
        .checked_debug_expression(&path, span, "count(^books.byShelf(\"fiction\")) > 0")
        .expect("indexed durable debug expression is admitted with data-access fact");
    assert_eq!(
        indexed.data_access(),
        DebugExpressionDataAccess::RequiresDurableData
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
    let shelf = "module shelf\nresource Thing\n    required name: string\n";
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

    let source_only = analyze_project(&root, &config(), &ProjectSources::new(), None, None)
        .expect("source-only analysis");
    let stable = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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

/// Project a committed lock from a proposal catalog: every Active entry becomes a lock entry and
/// `source_digest` records the shape the lock was produced under, exactly as the run path writes
/// it. A test threads this lock through `analyze_project` with no accepted store to drive the
/// store-less first-run adoption path.
fn clean_lock_for(
    catalog: &marrow_catalog::CatalogMetadata,
    source_digest: &str,
) -> marrow_catalog::CatalogLock {
    let entries = catalog
        .entries
        .iter()
        .filter(|entry| entry.lifecycle == marrow_catalog::CatalogLifecycle::Active)
        .map(marrow_catalog::LockEntry::from_catalog_entry)
        .collect();
    marrow_catalog::CatalogLock::new(
        entries,
        Vec::new(),
        catalog.epoch,
        source_digest.to_string(),
    )
    .expect("lock builds from a valid proposal catalog")
}

#[test]
fn store_less_clean_lock_adoption_binds_accepted_not_a_proposal() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        surface Books from ^books\n    \
        fields title\n";
    let root = temp_root("analysis-store-less-clean-lock-adoption");
    write(&root, "src/m.mw", source);

    // The first check with no store and no lock proposes catalog ids at epoch 1. The committed
    // lock that the run path would project from that proposal carries those ids and the shape
    // digest the source was checked under.
    let (report, program) = check_project(&root, &config()).expect("baseline check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes catalog ids");
    let source_digest = program.source_digest();
    let lock = clean_lock_for(&proposal, &source_digest);

    // Re-analyzing the unchanged source with no accepted store but the clean committed lock must
    // bind the lock as the accepted reference: the lock's epoch is the accepted epoch and there is
    // no pending proposal, so the surface ABI is stable exactly as a live store would make it.
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("clean lock-only analysis");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    assert_eq!(
        snapshot.program.catalog.accepted_epoch,
        Some(lock.epoch_high_water),
        "a clean lock-only bind adopts the lock epoch as accepted"
    );
    assert!(
        snapshot.program.catalog.proposal.is_none(),
        "a clean lock-only bind carries no pending proposal: {:#?}",
        snapshot.program.catalog.proposal
    );
    let status = snapshot
        .surface_read_operations()
        .next()
        .expect("a surface operation")
        .surface
        .catalog_status
        .clone();
    assert_eq!(
        status,
        SurfaceCatalogStatus::Stable,
        "a clean lock-only bind yields a stable surface ABI"
    );
}

#[test]
fn store_less_drifted_source_against_lock_stays_a_proposal() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        surface Books from ^books\n    \
        fields title\n";
    let root = temp_root("analysis-store-less-drifted-lock");
    write(&root, "src/m.mw", source);

    let (report, program) = check_project(&root, &config()).expect("baseline check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes catalog ids");
    let source_digest = program.source_digest();
    let lock = clean_lock_for(&proposal, &source_digest);

    // Add a member the committed lock never recorded. The source now carries an entity with no
    // committed identity, so the lock no longer adopts the source cleanly and the binding must
    // stay a pending proposal rather than falsely report the drifted source as accepted.
    let drifted = "module m\n\
        resource Book\n    \
        required title: string\n    \
        author: string\n\
        store ^books(id: int): Book\n\
        surface Books from ^books\n    \
        fields title\n";
    write(&root, "src/m.mw", drifted);

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("drifted lock analysis");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    assert!(
        snapshot.program.catalog.proposal.is_some(),
        "a drifted source against the lock stays a proposal, not accepted"
    );
    assert_eq!(
        snapshot.program.catalog.accepted_epoch, None,
        "a drifted source binds no accepted epoch from the lock"
    );
}

#[test]
fn store_less_shape_drifted_lock_stays_a_proposal() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        surface Books from ^books\n    \
        fields title\n";
    let root = temp_root("analysis-store-less-shape-drift");
    write(&root, "src/m.mw", source);

    let (report, program) = check_project(&root, &config()).expect("baseline check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes catalog ids");

    // A lock whose committed SHAPE for an entity differs from the current source shape is stale,
    // even when every `(kind, path)` still anchors a source entity: the lock was produced under a
    // different shape. Adoption keys on per-entry shape, not the order-sensitive whole-source
    // digest, so a shape edit the lock predates stays a proposal until the lock is re-projected.
    let mut entries: Vec<marrow_catalog::CatalogEntry> = proposal
        .entries
        .iter()
        .filter(|entry| entry.lifecycle == marrow_catalog::CatalogLifecycle::Active)
        .cloned()
        .collect();
    for entry in &mut entries {
        if entry.kind == marrow_catalog::CatalogEntryKind::ResourceMember
            && entry.path == "m::Book::title"
        {
            entry.accepted_struct = Some("leaf:int".to_string());
        }
    }
    let lock_entries = entries
        .iter()
        .map(marrow_catalog::LockEntry::from_catalog_entry)
        .collect();
    let lock = marrow_catalog::CatalogLock::new(
        lock_entries,
        Vec::new(),
        proposal.epoch,
        program.source_digest(),
    )
    .expect("shape-drifted lock builds");

    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), None, Some(&lock))
        .expect("shape-drift lock analysis");
    assert!(
        snapshot.program.catalog.proposal.is_some(),
        "a lock whose committed shape differs from source stays a proposal, not accepted"
    );
    assert_eq!(
        snapshot.program.catalog.accepted_epoch, None,
        "a shape-drifted lock binds no accepted epoch"
    );
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
        r#"{ "sourceRoots": ["src"], "tests": ["tests"], "store": { "backend": "native", "dataDir": ".marrow/data" } }"#,
    )
    .expect("config");

    let snapshot =
        analyze_project(&root, &cfg, &ProjectSources::new(), None, None).expect("analyze");
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
        versions(version: int): string\n\
        store ^books(id: int): Book\n\
        fn f(id: Id(^books), versions: int)\n    \
        const title: string = ^books(id).versions(versions) ?? \"\"\n    \
        print(title)\n";
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

    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
        r#"{ "sourceRoots": ["src"], "tests": ["tests"], "store": { "backend": "native", "dataDir": ".marrow/data" } }"#,
    )
    .expect("config");

    let snapshot =
        analyze_project(&root, &cfg, &ProjectSources::new(), None, None).expect("analyze");
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
        r#"{ "sourceRoots": ["src"], "tests": ["tests"], "store": { "backend": "native", "dataDir": ".marrow/data" } }"#,
    )
    .expect("config");

    let snapshot =
        analyze_project(&root, &cfg, &ProjectSources::new(), None, None).expect("analyze");
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
        r#"{ "sourceRoots": ["src"], "tests": ["tests"], "store": { "backend": "native", "dataDir": ".marrow/data" } }"#,
    )
    .expect("config");

    let snapshot =
        analyze_project(&root, &cfg, &ProjectSources::new(), None, None).expect("analyze");
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
fn declared_source_receiver_children_single_key_root() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f(id: int)\n    \
        print(0)\n";

    let children = declared_receiver_children(
        "analysis-source-receiver-single-key-root",
        source,
        "^books(id)",
        "print(0)",
    );

    assert_eq!(children, vec![required_string_child("title")]);
}

#[test]
fn declared_source_receiver_children_composite_scalar_keys() {
    let source = "module m\n\
        resource Pair\n    \
        required label: string\n\
        store ^pairs(left: int, right: int): Pair\n\
        fn f(left: int, right: int)\n    \
        print(0)\n";

    let children = declared_receiver_children(
        "analysis-source-receiver-composite-scalar-keys",
        source,
        "^pairs(left, right)",
        "print(0)",
    );

    assert_eq!(children, vec![required_string_child("label")]);
}

#[test]
fn declared_source_receiver_children_composite_identity_arg() {
    let source = "module m\n\
        resource Pair\n    \
        required label: string\n\
        store ^pairs(left: int, right: int): Pair\n\
        fn f(left: int, right: int)\n    \
        const pair: Id(^pairs) = Id(^pairs, left, right)\n    \
        print(0)\n";

    let children = declared_receiver_children(
        "analysis-source-receiver-composite-identity-arg",
        source,
        "^pairs(pair)",
        "print(0)",
    );

    assert_eq!(children, vec![required_string_child("label")]);
}

#[test]
fn declared_source_receiver_children_keyed_layer_entry() {
    let source = "module m\n\
        resource Book\n    \
        notes(noteId: string)\n        \
        required text: string\n\
        store ^books(id: int): Book\n\
        fn f(id: int, noteId: string)\n    \
        print(0)\n";

    let children = declared_receiver_children(
        "analysis-source-receiver-keyed-layer-entry",
        source,
        "^books(id).notes(noteId)",
        "print(0)",
    );

    assert_eq!(children, vec![required_string_child("text")]);
}

#[test]
fn saved_path_completion_context_segments_are_public_tooling_types() {
    let source = "module m\n\
        resource Book\n    \
        notes(noteId: string)\n        \
        required text: string\n\
        store ^books(id: int): Book\n\
        fn f(id: int, noteId: string)\n    \
        const value = ^books(id).notes(noteId).|candidate\n";
    let offset = source.find('|').expect("cursor marker");
    let source = source.replacen('|', "", 1);
    let (snapshot, paths) = analyze_overlay(
        "analysis-saved-path-completion-context-public-segments",
        &[("src/m.mw", &source)],
    );
    let path = paths.into_iter().next().expect("source path");
    let parsed = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .expect("analyzed source")
        .parsed
        .clone();
    let lexed = marrow_syntax::lex_source(&source);
    let fact = source_saved_path_completion_fact_at(
        &snapshot.program,
        &path,
        &source,
        &parsed,
        &lexed,
        offset,
    )
    .expect("saved-path completion fact");

    let labels = fact
        .context
        .segments
        .iter()
        .map(|segment| match segment {
            SourceSavedPathCompletionSegment::Root { name, .. } => format!("root:{name}"),
            SourceSavedPathCompletionSegment::KeySlot { name, .. } => format!("key:{name}"),
            SourceSavedPathCompletionSegment::Layer { name, .. } => format!("layer:{name}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        ["root:books", "key:id", "layer:notes", "key:noteId"]
    );
}

#[test]
fn declared_source_receiver_children_empty_for_partial_identity() {
    let source = "module m\n\
        resource Pair\n    \
        required label: string\n\
        store ^pairs(left: int, right: int): Pair\n\
        fn f(left: int)\n    \
        print(0)\n";

    let children = declared_receiver_children(
        "analysis-source-receiver-partial-identity",
        source,
        "^pairs(left)",
        "print(0)",
    );

    assert_eq!(children, Vec::<DeclaredDataChild>::new());
}

#[test]
fn declared_source_receiver_children_empty_for_partial_keyed_layer() {
    let source = "module m\n\
        resource Book\n    \
        notes(noteId: string)\n        \
        required text: string\n\
        store ^books(id: int): Book\n\
        fn f(id: int)\n    \
        print(0)\n";

    let children = declared_receiver_children(
        "analysis-source-receiver-partial-keyed-layer",
        source,
        "^books(id).notes",
        "print(0)",
    );

    assert_eq!(children, Vec::<DeclaredDataChild>::new());
}

#[test]
fn declared_source_receiver_children_empty_for_wrong_identity() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        resource Pair\n    \
        required label: string\n\
        store ^pairs(left: int, right: int): Pair\n\
        fn f(other: Id(^books))\n    \
        print(0)\n";

    let children = declared_receiver_children(
        "analysis-source-receiver-wrong-identity",
        source,
        "^pairs(other)",
        "print(0)",
    );

    assert_eq!(children, Vec::<DeclaredDataChild>::new());
}

#[test]
fn declared_source_receiver_children_empty_for_self_referential_initializer() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        fn f()\n    \
        const id: Id(^books) = ^books(id).|title\n";
    let children = saved_path_completion_children_at_error_cursor(
        "analysis-source-receiver-self-referential-initializer",
        source,
    );

    assert_eq!(children, Vec::<DeclaredDataChild>::new());
}

#[test]
fn declared_source_receiver_children_empty_for_module_const_self_reference() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n\
        const id: Id(^books) = ^books(id).|title\n";
    let children = saved_path_completion_children_at_error_cursor(
        "analysis-source-receiver-module-const-self-reference",
        source,
    );

    assert_eq!(children, Vec::<DeclaredDataChild>::new());
}

#[test]
fn declared_source_data_children_returns_schema_members_after_expression_identity_key() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n    \
        shelf: string\n    \
        tags(pos: int): string\n    \
        notes(noteId: string)\n        \
        required text: string\n\
        store ^books(id: int): Book\n";
    let (snapshot, _) = analyze_overlay(
        "analysis-declared-source-children-expression-key",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let children = declared_source_data_children(
        &snapshot.program,
        &[
            SourceDataPathSegment::Root("books".to_string()),
            SourceDataPathSegment::KeySlot,
        ],
    )
    .expect("declared children for complete source-shaped identity");

    assert_eq!(
        children,
        vec![
            DeclaredDataChild {
                name: "title".to_string(),
                kind: DeclaredDataChildKind::Field { required: true },
                key_params: Vec::new(),
                leaf: Some(StoreLeafKind::Scalar(StoreScalarType::Str)),
            },
            DeclaredDataChild {
                name: "shelf".to_string(),
                kind: DeclaredDataChildKind::Field { required: false },
                key_params: Vec::new(),
                leaf: Some(StoreLeafKind::Scalar(StoreScalarType::Str)),
            },
            DeclaredDataChild {
                name: "tags".to_string(),
                kind: DeclaredDataChildKind::Field { required: false },
                key_params: vec![DeclaredDataKeyParam {
                    name: "pos".to_string(),
                    scalar: Some(ScalarType::Int),
                }],
                leaf: Some(StoreLeafKind::Scalar(StoreScalarType::Str)),
            },
            DeclaredDataChild {
                name: "notes".to_string(),
                kind: DeclaredDataChildKind::Layer,
                key_params: vec![DeclaredDataKeyParam {
                    name: "noteId".to_string(),
                    scalar: Some(ScalarType::Str),
                }],
                leaf: None,
            },
        ]
    );
}

#[test]
fn declared_source_data_children_returns_empty_for_partial_key_prefixes() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n    \
        notes(noteId: string)\n        \
        required text: string\n\
        store ^books(id: int): Book\n";
    let (snapshot, _) = analyze_overlay(
        "analysis-declared-source-children-partial-key",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let root_children = declared_source_data_children(
        &snapshot.program,
        &[SourceDataPathSegment::Root("books".to_string())],
    )
    .expect("root prefix is a valid source-shaped path");
    assert_eq!(root_children, Vec::<DeclaredDataChild>::new());

    let layer_prefix_children = declared_source_data_children(
        &snapshot.program,
        &[
            SourceDataPathSegment::Root("books".to_string()),
            SourceDataPathSegment::KeySlot,
            SourceDataPathSegment::Member("notes".to_string()),
        ],
    )
    .expect("keyed layer prefix is a valid source-shaped path");
    assert_eq!(layer_prefix_children, Vec::<DeclaredDataChild>::new());

    let layer_entry_children = declared_source_data_children(
        &snapshot.program,
        &[
            SourceDataPathSegment::Root("books".to_string()),
            SourceDataPathSegment::KeySlot,
            SourceDataPathSegment::Member("notes".to_string()),
            SourceDataPathSegment::KeySlot,
        ],
    )
    .expect("complete keyed layer entry has declared children");
    assert_eq!(
        layer_entry_children,
        vec![DeclaredDataChild {
            name: "text".to_string(),
            kind: DeclaredDataChildKind::Field { required: true },
            key_params: Vec::new(),
            leaf: Some(StoreLeafKind::Scalar(StoreScalarType::Str)),
        }]
    );
}

#[test]
fn declared_data_children_accepts_concrete_data_path_segments() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n";
    let (snapshot, _) = analyze_overlay(
        "analysis-declared-data-children-concrete-path",
        &[("src/m.mw", source)],
    );
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );

    let children = declared_data_children(
        &snapshot.program,
        &[
            DataPathSegment::Root("books".to_string()),
            DataPathSegment::Key(SavedKey::Int(7)),
        ],
    )
    .expect("declared children for complete concrete identity");
    assert_eq!(
        children,
        vec![DeclaredDataChild {
            name: "title".to_string(),
            kind: DeclaredDataChildKind::Field { required: true },
            key_params: Vec::new(),
            leaf: Some(StoreLeafKind::Scalar(StoreScalarType::Str)),
        }]
    );

    let error = declared_data_children(
        &snapshot.program,
        &[
            DataPathSegment::Root("books".to_string()),
            DataPathSegment::Key(SavedKey::Str("not-an-int".to_string())),
        ],
    )
    .expect_err("concrete keys still validate declared key types");
    let ToolingError::Path(DataPathError::IdentityKeyType {
        root,
        expected,
        found,
    }) = error
    else {
        panic!("expected identity key type error, got {error:?}");
    };
    assert_eq!(root, "books");
    assert_eq!(expected, ScalarType::Int);
    assert_eq!(found, ScalarType::Str);
}

#[test]
fn catalog_bound_saved_data_segments_issue_and_resolve_accepted_identity() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n";
    let root = temp_root("analysis-catalog-bound-saved-data-segments");
    write(&root, "src/m.mw", source);
    let (checked, program) = check_project(&root, &config()).expect("check source");
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    let accepted = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes a catalog");
    let proposal_store_catalog_id = accepted
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::Store && entry.path == "m::^books")
        .expect("proposal store catalog id")
        .stable_id
        .clone();

    let proposal_only = resolve_saved_data_path(
        &program,
        &[SavedDataPathSegment::Root {
            store_catalog_id: proposal_store_catalog_id.clone(),
        }],
    )
    .expect_err("proposal-only ids are not accepted saved-data authority");
    let ToolingError::Path(DataPathError::UnknownRootCatalogId {
        store_catalog_id: rejected_proposal_root,
    }) = proposal_only
    else {
        panic!("expected unknown root catalog id, got {proposal_only:?}");
    };
    assert_eq!(rejected_proposal_root, proposal_store_catalog_id);

    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
    .expect("analyze accepted source");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let place = root_place(&snapshot.program, "books").expect("books place");
    let store_id = store_id_of(&place).expect("books store id");
    let store_catalog_id = store_id.as_str().to_string();
    let title_member_catalog_id = member_catalog_id(&place, "title").expect("title id");
    let title_id = CatalogId::new(title_member_catalog_id.clone()).expect("valid title id");
    let root_segment = SavedDataPathSegment::Root {
        store_catalog_id: store_catalog_id.clone(),
    };
    let title_segment = SavedDataPathSegment::Field {
        member_catalog_id: title_member_catalog_id.clone(),
    };

    let store = TreeStore::memory();
    store
        .replace_catalog_snapshot(&accepted)
        .expect("write catalog snapshot");
    store
        .write_record_presence(&store_id, &[SavedKey::Int(7)])
        .expect("seed record presence");
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(7)],
            &[StoreDataPathSegment::Member(title_id)],
            encode_value(&SavedValue::Str("Dune".to_string())).expect("encode title"),
        )
        .expect("seed title");

    let roots = stamped_saved_data_root_views_in_store(&snapshot.program, &store)
        .expect("catalog-bound root views");
    assert_eq!(
        roots.data,
        vec![DataChildView {
            segment: root_segment.clone(),
            label: "books".to_string(),
        }]
    );

    let keys = stamped_saved_data_child_views(
        &snapshot.program,
        &store,
        std::slice::from_ref(&root_segment),
        10,
        None,
    )
    .expect("catalog-bound key views");
    assert_eq!(
        keys.data.children,
        vec![DataChildView {
            segment: SavedDataPathSegment::Key(SavedKey::Int(7)),
            label: "(7)".to_string(),
        }]
    );

    let members = stamped_saved_data_child_views(
        &snapshot.program,
        &store,
        &[
            root_segment.clone(),
            SavedDataPathSegment::Key(SavedKey::Int(7)),
        ],
        10,
        None,
    )
    .expect("catalog-bound member views");
    assert_eq!(
        members.data.children,
        vec![DataChildView {
            segment: title_segment.clone(),
            label: "title".to_string(),
        }]
    );

    let resolved = resolve_saved_data_path(
        &snapshot.program,
        &[
            root_segment.clone(),
            SavedDataPathSegment::Key(SavedKey::Int(7)),
            title_segment,
        ],
    )
    .expect("accepted ids resolve")
    .expect("accepted saved path");
    assert_eq!(resolved.path(), "^books(7).title");
    assert_eq!(
        resolved.segments(),
        &[
            DataPathSegment::Root("books".to_string()),
            DataPathSegment::Key(SavedKey::Int(7)),
            DataPathSegment::Field("title".to_string()),
        ]
    );

    let display_root = resolve_saved_data_path(
        &snapshot.program,
        &[SavedDataPathSegment::Root {
            store_catalog_id: "books".to_string(),
        }],
    )
    .expect_err("display root text is not catalog-bound authority");
    let ToolingError::Path(DataPathError::UnknownRootCatalogId {
        store_catalog_id: rejected_display_root,
    }) = display_root
    else {
        panic!("expected unknown root catalog id, got {display_root:?}");
    };
    assert_eq!(rejected_display_root, "books");

    let display_field = resolve_saved_data_path(
        &snapshot.program,
        &[
            root_segment,
            SavedDataPathSegment::Key(SavedKey::Int(7)),
            SavedDataPathSegment::Field {
                member_catalog_id: "title".to_string(),
            },
        ],
    )
    .expect_err("display field text is not catalog-bound authority");
    let ToolingError::Path(DataPathError::UnknownMemberCatalogId {
        flavor,
        member_catalog_id: rejected_display_member,
    }) = display_field
    else {
        panic!("expected unknown member catalog id, got {display_field:?}");
    };
    assert_eq!(flavor, MemberFlavor::Field);
    assert_eq!(rejected_display_member, "title");
}

#[test]
fn checked_runtime_program_accepts_only_active_catalog_ids() {
    let source = "module m\n\
        resource Book\n    \
        required title: string\n\
        store ^books(id: int): Book\n";
    let root = temp_root("analysis-runtime-active-catalog-ids");
    write(&root, "src/m.mw", source);
    let (checked, program) = check_project(&root, &config()).expect("check source");
    assert!(!checked.has_errors(), "{:#?}", checked.diagnostics);
    let accepted = program
        .catalog
        .proposal
        .clone()
        .expect("first check proposes a catalog");
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
    .expect("analyze accepted source");
    assert!(
        !snapshot.report.has_errors(),
        "{:#?}",
        snapshot.report.diagnostics
    );
    let mut program = snapshot.program;
    let active_title = program
        .catalog
        .accepted_entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "m::Book::title"
        })
        .expect("accepted title entry")
        .clone();
    let active_title_id = active_title.stable_id.clone();
    let mut reserved_title = active_title;
    reserved_title.stable_id = "cat_fffffffffffffffffffffffffffffffe".to_string();
    reserved_title.path = "m::Book::retired".to_string();
    reserved_title.lifecycle = CatalogLifecycle::Reserved;
    let reserved_title_id = reserved_title.stable_id.clone();
    program.catalog.accepted_entries.push(reserved_title);

    let runtime = program.runtime();

    assert!(runtime.has_accepted_catalog_id(&active_title_id));
    assert!(
        !runtime.has_accepted_catalog_id(&reserved_title_id),
        "runtime saved-data identity must not admit reserved accepted catalog ids"
    );
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
