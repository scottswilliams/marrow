use marrow_check::{DiagnosticPayload, SurfaceCollisionNameKind, check_project};
use marrow_syntax::SourceSpan;

use crate::support::{assert_clean, config, temp_project, with_code, write};

fn source_line_span(source: &str, line: u32) -> SourceSpan {
    let start_byte = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1) as usize)
        .map(str::len)
        .sum();
    let end_byte = source[start_byte..]
        .find('\n')
        .map_or(source.len(), |offset| start_byte + offset);
    SourceSpan {
        start_byte,
        end_byte,
        line,
        column: 1,
    }
}

fn surface_collisions(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, "check.surface_collision")
}

fn duplicate_declarations(
    report: &marrow_check::CheckReport,
) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, "check.duplicate_declaration")
}

fn duplicate_declaration_lines(report: &marrow_check::CheckReport) -> Vec<u32> {
    duplicate_declarations(report)
        .into_iter()
        .map(|diagnostic| diagnostic.span.line)
        .collect()
}

fn assert_surface_collision_payload(
    diagnostic: &marrow_check::CheckDiagnostic,
    name: &str,
    first_kind: SurfaceCollisionNameKind,
    duplicate_kind: SurfaceCollisionNameKind,
    source: &str,
    first_line: u32,
) {
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::SurfaceCollision {
            name: name.into(),
            first_kind,
            first_span: source_line_span(source, first_line),
            duplicate_kind,
        }
    );
}

#[test]
fn surface_name_collision_uses_surface_code_instead_of_duplicate_declaration() {
    let source = "\
module app
resource Books
    title: string
surface Books from ^books
    fields title
";
    let root = temp_project("surface-module-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "Books",
        SurfaceCollisionNameKind::Resource,
        SurfaceCollisionNameKind::Surface,
        source,
        2,
    );
    assert_eq!(
        collisions[0].span.line, 4,
        "the later surface declaration is reported"
    );
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn later_declarations_collide_with_prior_surface_owner() {
    let source = "\
module app
resource Books
    title: string
surface Books from ^books
    fields title
fn Books()
    return
";
    let root = temp_project("surface-prior-owner-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 2, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "Books",
        SurfaceCollisionNameKind::Resource,
        SurfaceCollisionNameKind::Surface,
        source,
        2,
    );
    assert_surface_collision_payload(
        collisions[1],
        "Books",
        SurfaceCollisionNameKind::Surface,
        SurfaceCollisionNameKind::Function,
        source,
        4,
    );
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn import_short_name_collision_with_surface_uses_surface_code() {
    let source = "\
module app
use shelf::Books
surface Books from ^books
    fields title
";
    let root = temp_project("surface-import-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "Books",
        SurfaceCollisionNameKind::Import,
        SurfaceCollisionNameKind::Surface,
        source,
        2,
    );
    assert_eq!(collisions[0].span.line, 3, "{:#?}", collisions[0]);
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn surface_builtin_name_uses_surface_collision_without_duplicate_declaration() {
    let source = "\
module app
surface exists from ^books
    fields title
";
    let root = temp_project("surface-builtin-name", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "exists",
        SurfaceCollisionNameKind::Builtin,
        SurfaceCollisionNameKind::Surface,
        source,
        2,
    );
    assert_eq!(collisions[0].span.line, 2, "{:#?}", collisions[0]);
    assert!(
        duplicate_declarations(&report).is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn surface_collisions_on_builtin_names_do_not_leak_duplicate_declarations() {
    let cases = [
        (
            "\
module app
fn exists()
    return
surface exists from ^books
    fields title
",
            4,
            vec![2],
        ),
        (
            "\
module app
surface exists from ^books
    fields title
fn exists()
    return
",
            2,
            vec![4],
        ),
        (
            "\
module app
use shelf::exists
surface exists from ^books
    fields title
",
            3,
            vec![],
        ),
    ];

    for (index, (source, surface_line, duplicate_lines)) in cases.into_iter().enumerate() {
        let root = temp_project(&format!("surface-builtin-collision-{index}"), |root| {
            write(root, "src/app.mw", source);
        });
        let (report, _program) = check_project(&root, &config()).expect("check");

        let collisions = surface_collisions(&report);
        assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
        assert_surface_collision_payload(
            collisions[0],
            "exists",
            SurfaceCollisionNameKind::Builtin,
            SurfaceCollisionNameKind::Surface,
            source,
            surface_line,
        );
        assert_eq!(
            collisions[0].span.line, surface_line,
            "{:#?}",
            collisions[0]
        );
        assert_eq!(
            duplicate_declaration_lines(&report),
            duplicate_lines,
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn canonical_surface_example_allows_payload_overlap() {
    let source = "\
module app
surface Books from ^books
    fields title, author, blurb
    collection ^books as list
    collection ^books.byAuthor as byAuthor
    create title, author, blurb
    update title, blurb
";
    let root = temp_project("surface-canonical-example", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
}

#[test]
fn collection_aliases_collide_with_generated_operation_names() {
    let source = "\
module app
surface Books from ^books
    collection ^books as create
";
    let root = temp_project("surface-generated-alias-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "create",
        SurfaceCollisionNameKind::GeneratedOperation,
        SurfaceCollisionNameKind::CollectionAlias,
        source,
        2,
    );
    assert_eq!(collisions[0].span.line, 3, "{:#?}", collisions[0]);
}

#[test]
fn payload_names_do_not_collide_with_generated_operations_or_aliases() {
    let source = "\
module app
surface Books from ^books
    fields id, get, create, update, list
    collection ^books as list
    create create
    update get
";
    let root = temp_project("surface-payload-operation-overlap", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
}

#[test]
fn duplicate_collection_aliases_collide_inside_one_surface() {
    let source = "\
module app
surface Books from ^books
    collection ^books as list
    collection ^books.byAuthor as list
";
    let root = temp_project("surface-collection-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "list",
        SurfaceCollisionNameKind::CollectionAlias,
        SurfaceCollisionNameKind::CollectionAlias,
        source,
        3,
    );
    assert_eq!(collisions[0].span.line, 4, "{:#?}", collisions[0]);
}

#[test]
fn duplicate_payload_names_collide_within_the_same_payload_namespace() {
    let cases = [
        (
            "\
module app
surface Books from ^books
    fields title, title
",
            "title",
            SurfaceCollisionNameKind::FieldItem,
        ),
        (
            "\
module app
surface Books from ^books
    create title, title
",
            "title",
            SurfaceCollisionNameKind::CreateItem,
        ),
        (
            "\
module app
surface Books from ^books
    update title, title
",
            "title",
            SurfaceCollisionNameKind::UpdateItem,
        ),
    ];

    for (index, (source, name, kind)) in cases.into_iter().enumerate() {
        let root = temp_project(&format!("surface-payload-duplicate-{index}"), |root| {
            write(root, "src/app.mw", source);
        });
        let (report, _program) = check_project(&root, &config()).expect("check");

        let collisions = surface_collisions(&report);
        assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
        assert_surface_collision_payload(collisions[0], name, kind, kind, source, 3);
        assert_eq!(collisions[0].span.line, 3, "{:#?}", collisions[0]);
    }
}

#[test]
fn distinct_surfaces_have_independent_local_namespaces() {
    let source = "\
module app
surface Books from ^books
    fields title
    collection ^books as list
surface Authors from ^authors
    fields title
    collection ^authors as list
";
    let root = temp_project("surface-independent-local-namespaces", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
}
