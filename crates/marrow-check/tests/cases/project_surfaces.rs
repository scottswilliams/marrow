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
fn surface_local_names_collide_with_generated_operation_names() {
    let source = "\
module app
surface Books from ^books
    create create
";
    let root = temp_project("surface-generated-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "create",
        SurfaceCollisionNameKind::GeneratedOperation,
        SurfaceCollisionNameKind::CreateItem,
        source,
        2,
    );
    assert_eq!(collisions[0].span.line, 3, "{:#?}", collisions[0]);
}

#[test]
fn surface_local_names_collide_across_item_kinds() {
    let source = "\
module app
surface Books from ^books
    fields title
    update title
";
    let root = temp_project("surface-item-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "title",
        SurfaceCollisionNameKind::FieldItem,
        SurfaceCollisionNameKind::UpdateItem,
        source,
        3,
    );
    assert_eq!(collisions[0].span.line, 4, "{:#?}", collisions[0]);
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
