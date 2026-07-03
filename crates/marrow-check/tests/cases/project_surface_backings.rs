use marrow_check::{
    CHECK_SURFACE_FIELD, CHECK_SURFACE_TARGET, DiagnosticPayload, SurfaceFieldDiagnostic,
    SurfaceFieldList, SurfaceFieldProblem, SurfaceRootOrigin, SurfaceTargetDiagnostic,
    check_project,
};

use crate::support::{config, temp_project, with_code, write};

enum ExpectedSurfaceDiagnostic {
    Field {
        name: &'static str,
        problem: SurfaceFieldProblem,
    },
    Target(SurfaceTargetDiagnostic),
}

struct InvalidSurfaceCase {
    name: &'static str,
    source: &'static str,
    required_code: &'static str,
    expected: ExpectedSurfaceDiagnostic,
}

fn assert_invalid_surface_case(case: InvalidSurfaceCase) {
    let root = temp_project(case.name, |root| {
        write(root, "src/app.mw", case.source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(
        with_code(&report, case.required_code).len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert_expected_surface_diagnostic(&report, case.expected);
    assert!(
        program.facts.surfaces().is_empty(),
        "invalid backing case `{}` must not produce checked surface facts",
        case.name
    );
}

fn assert_expected_surface_diagnostic(
    report: &marrow_check::CheckReport,
    expected: ExpectedSurfaceDiagnostic,
) {
    match expected {
        ExpectedSurfaceDiagnostic::Field { name, problem } => {
            assert!(
                surface_targets(report).is_empty(),
                "field diagnostic case should not also emit surface target diagnostics: {:#?}",
                report.diagnostics
            );
            assert_eq!(
                surface_fields(report)
                    .iter()
                    .map(|diagnostic| &diagnostic.payload)
                    .collect::<Vec<_>>(),
                vec![&DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                    list: SurfaceFieldList::Fields,
                    name: name.into(),
                    problem,
                })]
            );
        }
        ExpectedSurfaceDiagnostic::Target(expected) => {
            assert!(
                surface_fields(report).is_empty(),
                "target diagnostic case should not also emit surface field diagnostics: {:#?}",
                report.diagnostics
            );
            assert_eq!(
                surface_targets(report)
                    .iter()
                    .map(|diagnostic| &diagnostic.payload)
                    .collect::<Vec<_>>(),
                vec![&DiagnosticPayload::SurfaceTarget(expected)]
            );
        }
    }
}

fn surface_targets(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, CHECK_SURFACE_TARGET)
}

fn surface_fields(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, CHECK_SURFACE_FIELD)
}

fn duplicate_declarations(
    report: &marrow_check::CheckReport,
) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, "check.duplicate_declaration")
}

fn surface_collisions(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, "check.surface_collision")
}

#[test]
fn ambiguous_backing_targets_do_not_produce_surface_facts() {
    let cases = [
        InvalidSurfaceCase {
            name: "surface-ambiguous-fields",
            required_code: "schema.duplicate_member",
            expected: ExpectedSurfaceDiagnostic::Field {
                name: "title",
                problem: SurfaceFieldProblem::Ambiguous,
            },
            source: "\
module app
resource Book
    title: string
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-ambiguous-store-resource",
            required_code: "check.duplicate_declaration",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::AmbiguousStoreResource {
                    surface: "Books".into(),
                    root: "books".into(),
                    resource: "Book".into(),
                },
            ),
            source: "\
module app
resource Book
    title: string
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-ambiguous-indexes",
            required_code: "schema.duplicate_member",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::AmbiguousCollectionIndex {
                    root: "books".into(),
                    index: "byTitle".into(),
                },
            ),
            source: "\
module app
resource Book
    title: string
store ^books(id: int): Book
    index byTitle(title, id)
    index byTitle(title, id)
surface Books from ^books
    fields title
    collection ^books.byTitle as byTitle
",
        },
    ];

    for case in cases {
        assert_invalid_surface_case(case);
    }
}

#[test]
fn duplicate_root_owner_suppresses_root_and_collection_surface_facts() {
    let source = "\
module app
resource Book
    title: string
resource Shelf
    title: string
store ^books(id: int): Book
store ^books(id: int): Missing
store ^shelves(id: int): Shelf
surface Books from ^books
    fields title
surface Shelves from ^shelves
    fields title
    collection ^books as books
";
    let root = temp_project("surface-duplicate-root-owner-diagnostic", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(
        program
            .facts
            .stores()
            .iter()
            .filter(|store| store.root == "books")
            .count(),
        1,
        "duplicate source roots do not require duplicate store facts"
    );
    assert_eq!(
        surface_targets(&report)
            .iter()
            .map(|diagnostic| &diagnostic.payload)
            .collect::<Vec<_>>(),
        vec![
            &DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::AmbiguousStore {
                origin: SurfaceRootOrigin::Surface {
                    name: "Books".into(),
                },
                root: "books".into(),
            }),
            &DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::AmbiguousStore {
                origin: SurfaceRootOrigin::Collection,
                root: "books".into(),
            }),
        ]
    );
    assert!(program.facts.surfaces().is_empty());
}

#[test]
fn schema_invalid_backings_reject_fields_indexes_and_stores() {
    let cases = [
        InvalidSurfaceCase {
            name: "surface-schema-invalid-field",
            required_code: "schema.unknown_in_saved",
            expected: ExpectedSurfaceDiagnostic::Field {
                name: "title",
                problem: SurfaceFieldProblem::Invalid,
            },
            source: "\
module app
resource Book
    title: unknown
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-schema-invalid-index",
            required_code: "schema.index_missing_identity_keys",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidCollectionIndex {
                    root: "books".into(),
                    index: "byTitle".into(),
                },
            ),
            source: "\
module app
resource Book
    title: string
store ^books(id: int): Book
    index byTitle(title)
surface Books from ^books
    fields title
    collection ^books.byTitle as byTitle
",
        },
        InvalidSurfaceCase {
            name: "surface-unprojected-invalid-member",
            required_code: "schema.unorderable_key",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidStoreResource {
                    surface: "Books".into(),
                    root: "books".into(),
                    resource: "Book".into(),
                },
            ),
            source: "\
module app
resource Book
    title: string
    tags(pos: decimal): string
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-checker-invalid-typed-keyed-entry",
            required_code: "check.unknown_type",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidStoreResource {
                    surface: "Books".into(),
                    root: "books".into(),
                    resource: "Book".into(),
                },
            ),
            source: "\
module app
resource Book
    title: string
    rows(pos: int): Missing
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-unprojected-unknown-identity",
            required_code: "check.unknown_type",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidStoreResource {
                    surface: "Books".into(),
                    root: "books".into(),
                    resource: "Book".into(),
                },
            ),
            source: "\
module app
resource Book
    title: string
    author: Id(^missing)
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-schema-invalid-store",
            required_code: "schema.unorderable_key",
            expected: ExpectedSurfaceDiagnostic::Target(SurfaceTargetDiagnostic::InvalidStore {
                surface: "Books".into(),
                root: "books".into(),
            }),
            source: "\
module app
resource Book
    title: string
store ^books(id: decimal): Book
surface Books from ^books
    fields title
",
        },
    ];

    for case in cases {
        assert_invalid_surface_case(case);
    }
}

#[test]
fn invalid_enum_meanings_reject_surface_fields_indexes_and_stores() {
    let cases = [
        InvalidSurfaceCase {
            name: "surface-invalid-enum-field",
            required_code: "schema.category_leaf",
            expected: ExpectedSurfaceDiagnostic::Field {
                name: "status",
                problem: SurfaceFieldProblem::Invalid,
            },
            source: "\
module app
enum Status
    category broken
resource Book
    status: Status
store ^books(id: int): Book
surface Books from ^books
    fields status
",
        },
        InvalidSurfaceCase {
            name: "surface-invalid-enum-index",
            required_code: "schema.category_leaf",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidCollectionIndex {
                    root: "books".into(),
                    index: "byStatus".into(),
                },
            ),
            source: "\
module app
enum Status
    category broken
resource Book
    title: string
    status: Status
store ^books(id: int): Book
    index byStatus(status, id)
surface Books from ^books
    fields title
    collection ^books.byStatus as byStatus
",
        },
        InvalidSurfaceCase {
            name: "surface-unprojected-invalid-enum",
            required_code: "schema.category_leaf",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidStoreResource {
                    surface: "Books".into(),
                    root: "books".into(),
                    resource: "Book".into(),
                },
            ),
            source: "\
module app
enum Status
    category broken
resource Book
    title: string
    status: Status
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-duplicate-enum-field",
            required_code: "check.duplicate_declaration",
            expected: ExpectedSurfaceDiagnostic::Field {
                name: "status",
                problem: SurfaceFieldProblem::Invalid,
            },
            source: "\
module app
enum Status
    draft
enum Status
    published
resource Book
    status: Status
store ^books(id: int): Book
surface Books from ^books
    fields status
",
        },
        InvalidSurfaceCase {
            name: "surface-duplicate-enum-index",
            required_code: "check.duplicate_declaration",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidCollectionIndex {
                    root: "books".into(),
                    index: "byStatus".into(),
                },
            ),
            source: "\
module app
enum Status
    draft
enum Status
    published
resource Book
    title: string
    status: Status
store ^books(id: int): Book
    index byStatus(status, id)
surface Books from ^books
    fields title
    collection ^books.byStatus as byStatus
",
        },
    ];

    for case in cases {
        assert_invalid_surface_case(case);
    }
}

#[test]
fn invalid_identity_meanings_reject_surface_fields_indexes_and_stores() {
    let cases = [
        InvalidSurfaceCase {
            name: "surface-invalid-identity-field",
            required_code: "schema.unorderable_key",
            expected: ExpectedSurfaceDiagnostic::Field {
                name: "author",
                problem: SurfaceFieldProblem::Invalid,
            },
            source: "\
module app
resource Author
    name: string
    tags(pos: decimal): string
store ^authors(id: int): Author
resource Book
    author: Id(^authors)
store ^books(id: int): Book
surface Books from ^books
    fields author
",
        },
        InvalidSurfaceCase {
            name: "surface-invalid-identity-index",
            required_code: "schema.unorderable_key",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidCollectionIndex {
                    root: "books".into(),
                    index: "byAuthor".into(),
                },
            ),
            source: "\
module app
resource Author
    name: string
    tags(pos: decimal): string
store ^authors(id: int): Author
resource Book
    title: string
    author: Id(^authors)
store ^books(id: int): Book
    index byAuthor(author, id)
surface Books from ^books
    fields title
    collection ^books.byAuthor as byAuthor
",
        },
        InvalidSurfaceCase {
            name: "surface-unprojected-invalid-identity",
            required_code: "schema.unorderable_key",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidStoreResource {
                    surface: "Books".into(),
                    root: "books".into(),
                    resource: "Book".into(),
                },
            ),
            source: "\
module app
resource Author
    name: string
    tags(pos: decimal): string
store ^authors(id: int): Author
resource Book
    title: string
    author: Id(^authors)
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-duplicate-identity-root-field",
            required_code: "schema.duplicate_root_owner",
            expected: ExpectedSurfaceDiagnostic::Field {
                name: "author",
                problem: SurfaceFieldProblem::Invalid,
            },
            source: "\
module app
resource Author
    name: string
resource Writer
    name: string
store ^authors(id: int): Author
store ^authors(id: int): Writer
resource Book
    author: Id(^authors)
store ^books(id: int): Book
surface Books from ^books
    fields author
",
        },
        InvalidSurfaceCase {
            name: "surface-duplicate-identity-resource-index",
            required_code: "check.duplicate_declaration",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidCollectionIndex {
                    root: "books".into(),
                    index: "byAuthor".into(),
                },
            ),
            source: "\
module app
resource Author
    name: string
resource Author
    pen_name: string
store ^authors(id: int): Author
resource Book
    title: string
    author: Id(^authors)
store ^books(id: int): Book
    index byAuthor(author, id)
surface Books from ^books
    fields title
    collection ^books.byAuthor as byAuthor
",
        },
    ];

    for case in cases {
        assert_invalid_surface_case(case);
    }
}

#[test]
fn partial_index_meanings_reject_collection_surface_facts() {
    let cases = [
        InvalidSurfaceCase {
            name: "surface-partially-resolved-index",
            required_code: "schema.non_enum_named_field",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidCollectionIndex {
                    root: "books".into(),
                    index: "byTitle".into(),
                },
            ),
            source: "\
module app
resource Book
    author: string
    title: MissingEnum
store ^books(id: int): Book
    index byTitle(title, id)
surface Books from ^books
    fields author
    collection ^books.byTitle as byTitle
",
        },
        InvalidSurfaceCase {
            name: "surface-index-over-duplicate-member",
            required_code: "schema.duplicate_member",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidStoreResource {
                    surface: "Books".into(),
                    root: "books".into(),
                    resource: "Book".into(),
                },
            ),
            source: "\
module app
resource Book
    title: string
    title: string
    other: string
store ^books(id: int): Book
    index byTitle(title, id)
surface Books from ^books
    fields other
    collection ^books.byTitle as byTitle
",
        },
    ];

    for case in cases {
        assert_invalid_surface_case(case);
    }
}

#[test]
fn duplicate_identity_resource_fields_reject_surface_facts() {
    let source = "\
module app
resource Author
    name: string
resource Author
    pen_name: string
store ^authors(id: int): Author
resource Book
    author: Id(^authors)
store ^books(id: int): Book
surface Books from ^books
    fields author
";
    let root = temp_project("surface-duplicate-identity-resource-field", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(duplicate_declarations(&report).len(), 1);
    assert_expected_surface_diagnostic(
        &report,
        ExpectedSurfaceDiagnostic::Field {
            name: "author",
            problem: SurfaceFieldProblem::Invalid,
        },
    );
    assert!(program.facts.surfaces().is_empty());
}

#[test]
fn surface_name_collisions_do_not_hide_duplicate_durable_owners() {
    let enum_source = "\
module app
surface Status from ^books
    fields status
enum Status
    draft
enum Status
    published
resource Book
    status: Status
store ^books(id: int): Book
surface Books from ^books
    fields status
";
    let root = temp_project("surface-collision-duplicate-enum-owner", |root| {
        write(root, "src/app.mw", enum_source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(
        surface_collisions(&report).len(),
        2,
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        duplicate_declarations(&report).len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert_expected_surface_diagnostic(
        &report,
        ExpectedSurfaceDiagnostic::Field {
            name: "status",
            problem: SurfaceFieldProblem::Invalid,
        },
    );
    assert!(program.facts.surfaces().is_empty());

    let identity_source = "\
module app
surface Author from ^authors
    fields name
resource Author
    name: string
resource Author
    pen_name: string
store ^authors(id: int): Author
resource Book
    author: Id(^authors)
store ^books(id: int): Book
surface Books from ^books
    fields author
";
    let root = temp_project("surface-collision-duplicate-resource-owner", |root| {
        write(root, "src/app.mw", identity_source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(
        surface_collisions(&report).len(),
        2,
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        duplicate_declarations(&report).len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert_expected_surface_diagnostic(
        &report,
        ExpectedSurfaceDiagnostic::Field {
            name: "author",
            problem: SurfaceFieldProblem::Invalid,
        },
    );
    assert!(program.facts.surfaces().is_empty());
}

#[test]
fn surface_name_collision_with_sole_durable_owner_does_not_poison_backing() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
surface Book from ^books
    fields title
surface Catalog from ^books
    fields title
";
    let root = temp_project("surface-collision-sole-durable-owner", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(
        surface_collisions(&report).len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        surface_targets(&report).is_empty(),
        "a surface-name collision should not make the backing resource invalid: {:#?}",
        report.diagnostics
    );
    assert!(
        surface_fields(&report).is_empty(),
        "a surface-name collision should not make the backing fields invalid: {:#?}",
        report.diagnostics
    );
    assert_eq!(
        program
            .facts
            .surfaces()
            .iter()
            .map(|surface| surface.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Catalog"]
    );
}

#[test]
fn surface_name_collision_with_sole_enum_owner_does_not_poison_enum_meanings() {
    let source = "\
module app
enum Status
    draft
resource Book
    status: Status
store ^books(id: int): Book
surface Status from ^books
    fields status
surface Books from ^books
    fields status
";
    let root = temp_project("surface-collision-sole-enum-owner", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(
        surface_collisions(&report).len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        surface_targets(&report).is_empty(),
        "a surface-name collision should not make enum-backed targets invalid: {:#?}",
        report.diagnostics
    );
    assert!(
        surface_fields(&report).is_empty(),
        "a surface-name collision should not make enum-backed fields invalid: {:#?}",
        report.diagnostics
    );
    assert_eq!(
        program
            .facts
            .surfaces()
            .iter()
            .map(|surface| surface.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Books"]
    );
}

#[test]
fn unreferenced_invalid_index_does_not_poison_surface_facts() {
    let source = "\
module app
resource Book
    title: string
    author: string
store ^books(id: int): Book
    index byAuthor(author)
surface Books from ^books
    fields title
";
    let root = temp_project("surface-unreferenced-invalid-index", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(
        with_code(&report, "schema.index_missing_identity_keys").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        surface_targets(&report).is_empty(),
        "an unreferenced invalid index should not become a surface target error: {:#?}",
        report.diagnostics
    );
    let [surface] = program.facts.surfaces() else {
        panic!("expected one surface, got {:#?}", program.facts.surfaces());
    };
    assert_eq!(surface.name, "Books");
}

#[test]
fn builtin_colliding_backing_owners_do_not_produce_surface_facts() {
    let cases = [
        InvalidSurfaceCase {
            name: "surface-builtin-resource-backing",
            required_code: "check.builtin_collision",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidStoreResource {
                    surface: "Items".into(),
                    root: "items".into(),
                    resource: "exists".into(),
                },
            ),
            source: "\
module app
resource exists
    title: string
store ^items(id: int): exists
surface Items from ^items
    fields title
",
        },
        InvalidSurfaceCase {
            name: "surface-builtin-enum-field",
            required_code: "check.builtin_collision",
            expected: ExpectedSurfaceDiagnostic::Field {
                name: "kind",
                problem: SurfaceFieldProblem::Invalid,
            },
            source: "\
module app
enum exists
    active
resource Item
    kind: exists
store ^items(id: int): Item
surface Items from ^items
    fields kind
",
        },
        InvalidSurfaceCase {
            name: "surface-builtin-enum-index",
            required_code: "check.builtin_collision",
            expected: ExpectedSurfaceDiagnostic::Target(
                SurfaceTargetDiagnostic::InvalidCollectionIndex {
                    root: "items".into(),
                    index: "byKind".into(),
                },
            ),
            source: "\
module app
enum exists
    active
resource Item
    title: string
    kind: exists
store ^items(id: int): Item
    index byKind(kind, id)
surface Items from ^items
    fields title
    collection ^items.byKind as byKind
",
        },
    ];

    for case in cases {
        assert_invalid_surface_case(case);
    }
}
