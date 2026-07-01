use marrow_check::{
    CHECK_SURFACE_ACTION, CHECK_SURFACE_COMPUTED_READ, CHECK_SURFACE_FIELD, CHECK_SURFACE_TARGET,
    DiagnosticPayload, SurfaceActionDiagnostic, SurfaceCatalogBlocker, SurfaceCatalogStatus,
    SurfaceCollectionTarget, SurfaceCollisionNameKind, SurfaceComputedReadDiagnostic,
    SurfaceFieldDiagnostic, SurfaceFieldList, SurfaceFieldProblem, SurfaceReadFootprint,
    SurfaceReadOperationKind, SurfaceTargetDiagnostic, check_project, check_tests_program,
};
use marrow_syntax::SourceSpan;

use crate::support::{assert_clean, check_with_accepted, config, temp_project, with_code, write};

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

fn builtin_collision_lines(report: &marrow_check::CheckReport) -> Vec<u32> {
    with_code(report, "check.builtin_collision")
        .into_iter()
        .map(|diagnostic| diagnostic.span.line)
        .collect()
}

fn surface_targets(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, CHECK_SURFACE_TARGET)
}

fn surface_fields(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, CHECK_SURFACE_FIELD)
}

fn surface_actions(report: &marrow_check::CheckReport) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, CHECK_SURFACE_ACTION)
}

fn surface_computed_reads(
    report: &marrow_check::CheckReport,
) -> Vec<&marrow_check::CheckDiagnostic> {
    with_code(report, CHECK_SURFACE_COMPUTED_READ)
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
fn action_alias_collision_rejects_surface() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn addBook()
    return
surface Books from ^books
    fields title
    action addBook as get
";
    let root = temp_project("surface-action-generated-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "get",
        SurfaceCollisionNameKind::GeneratedOperation,
        SurfaceCollisionNameKind::ActionAlias,
        source,
        7,
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a surface with a colliding action alias must not become a fact"
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

    for (index, (source, surface_line, builtin_lines)) in cases.into_iter().enumerate() {
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
        // A non-surface declaration that also shadows the builtin reports its own
        // builtin collision; the surface collision never leaks a redeclaration.
        assert_eq!(
            builtin_collision_lines(&report),
            builtin_lines,
            "{:#?}",
            report.diagnostics
        );
        assert!(
            duplicate_declarations(&report).is_empty(),
            "{:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn canonical_surface_example_allows_payload_overlap() {
    let source = "\
module app
resource Book
    required title: string
    author: string
    blurb: string
store ^books(id: int): Book
    index byAuthor(author, id)
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
fn root_collections_require_a_keyed_store() {
    let source = "\
module app
resource Settings
    theme: string
store ^settings: Settings
surface SettingsSurface from ^settings
    fields theme
    collection ^settings as all
";
    let root = temp_project("surface-root-collection-singleton", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_targets(&report);
    assert_eq!(diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::KeylessCollectionRoot {
            root: "settings".into(),
        })
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a surface with a non-iterable collection target must not become a checked fact"
    );
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
resource Book
    id: string
    get: string
    create: string
    update: string
    list: string
store ^books(bookId: int): Book
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
        // The first occurrence of a duplicated payload name is spanned at the
        // name itself, and the duplicate at the second name's column, so the two
        // names on one line are distinguishable rather than collapsed to column 1.
        let DiagnosticPayload::SurfaceCollision {
            name: payload_name,
            first_kind,
            first_span,
            duplicate_kind,
        } = &collisions[0].payload
        else {
            panic!("expected a surface-collision payload: {:#?}", collisions[0]);
        };
        assert_eq!(payload_name, name);
        assert_eq!(*first_kind, kind);
        assert_eq!(*duplicate_kind, kind);
        assert_eq!((first_span.line, first_span.column), (3, 12));
        assert_eq!(
            (collisions[0].span.line, collisions[0].span.column),
            (3, 19)
        );
    }
}

#[test]
fn duplicate_delete_items_collide_inside_one_surface() {
    let source = "\
module app
surface Books from ^books
    delete
    delete
";
    let root = temp_project("surface-delete-duplicate", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "delete",
        SurfaceCollisionNameKind::DeleteItem,
        SurfaceCollisionNameKind::DeleteItem,
        source,
        3,
    );
    assert_eq!(collisions[0].span.line, 4, "{:#?}", collisions[0]);
}

#[test]
fn distinct_surfaces_have_independent_local_namespaces() {
    let source = "\
module app
resource Book
    title: string
resource Author
    title: string
store ^books(id: int): Book
store ^authors(id: int): Author
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

#[test]
fn rejected_surface_collisions_do_not_produce_surface_facts() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title, title
    collection ^books as create
";
    let root = temp_project("surface-collisions-no-facts", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_eq!(
        surface_collisions(&report).len(),
        2,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a rejected surface must not become a checked fact"
    );
    assert!(
        surface_targets(&report).is_empty(),
        "collision-rejected surfaces should not also run target validation: {:#?}",
        report.diagnostics
    );
}

#[test]
fn checking_tests_preserves_source_surface_facts() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
    collection ^books as list
";
    let root = temp_project("surface-facts-through-tests", |root| {
        write(root, "src/app.mw", source);
        write(root, "tests/smoke_test.mw", "pub fn smoke()\n    return\n");
    });
    let (source_report, source_program) = check_project(&root, &config()).expect("check");
    assert_clean(&source_report);
    assert_eq!(source_program.facts.surfaces().len(), 1);

    let (test_report, combined) =
        check_tests_program(&root, &config(), source_program).expect("check tests");

    assert_clean(&test_report);
    let [surface] = combined.facts.surfaces() else {
        panic!(
            "expected preserved source surface, got {:#?}",
            combined.facts.surfaces()
        );
    };
    assert_eq!(surface.name, "Books");
    let facts = &combined.facts;
    let module = facts.module_id("app").expect("app module");
    let book = facts.resource_id(module, "Book").expect("Book");
    let store = facts.store_id(module, "books").expect("^books");
    let title = facts.resource_member_id(book, &["title"]).expect("title");
    assert_eq!(surface.store, store);
    assert_eq!(
        surface
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.member))
            .collect::<Vec<_>>(),
        vec![("title", title)]
    );
    assert_eq!(
        surface
            .collections
            .iter()
            .map(|collection| (collection.alias.as_str(), collection.target))
            .collect::<Vec<_>>(),
        vec![("list", SurfaceCollectionTarget::StoreRoot(store))]
    );
    assert_eq!(
        surface
            .read_operations
            .iter()
            .map(|operation| operation.kind)
            .collect::<Vec<_>>(),
        vec![
            SurfaceReadOperationKind::PointRead { store },
            SurfaceReadOperationKind::PagedRootCollection { store },
        ]
    );
    assert!(
        surface
            .read_operations
            .iter()
            .all(|operation| operation.operation_tag.is_none()),
        "source-only surface read operations must not claim stable cursor tags"
    );
}

#[test]
fn checking_tests_does_not_mint_surface_facts_from_test_files() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
";
    let test_source = "\
use app
surface TestBooks from ^books
    fields title
pub fn smoke()
    return
";
    let root = temp_project("surface-test-files-source-only", |root| {
        write(root, "src/app.mw", source);
        write(root, "tests/smoke_test.mw", test_source);
    });
    let (source_report, source_program) = check_project(&root, &config()).expect("check");
    assert_clean(&source_report);
    assert_eq!(source_program.facts.surfaces().len(), 1);

    let (test_report, combined) =
        check_tests_program(&root, &config(), source_program).expect("check tests");

    assert_clean(&test_report);
    assert_eq!(
        combined
            .facts
            .surfaces()
            .iter()
            .map(|surface| surface.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Books"],
        "configured test files must not mint application surface facts"
    );
}

#[test]
fn surface_catalog_status_names_pending_catalog_proposal() {
    let root = temp_project("surface-catalog-pending-shape", |root| {
        write(
            root,
            "src/app.mw",
            "\
module app
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
    collection ^books as list
",
        );
    });
    let (baseline_report, baseline_program) =
        check_project(&root, &config()).expect("baseline check");
    assert_clean(&baseline_report);
    let baseline = baseline_program
        .catalog
        .proposal
        .expect("first run proposes accepted baseline");
    crate::support::catalog::write_catalog(&root, &baseline);

    write(
        &root,
        "src/app.mw",
        "\
module app
resource Book
    title: string
store ^books(id: string): Book
surface Books from ^books
    fields title
    collection ^books as list
",
    );
    let (report, program) = check_with_accepted(&root);

    assert_clean(&report);
    assert!(
        program.catalog.proposal.is_some(),
        "store key-shape drift must produce a pending catalog proposal"
    );
    let [surface] = program.facts.surfaces() else {
        panic!("expected one surface, got {:#?}", program.facts.surfaces());
    };
    assert_eq!(
        surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![SurfaceCatalogBlocker::PendingCatalogProposal]),
        "stable surface export must name the pending proposal blocker"
    );
}

#[test]
fn surface_facts_resolve_store_fields_and_collections() {
    let source = "\
module app
resource Book
    required title: string
    author: string
    tags(pos: int): string
store ^books(id: int): Book
    index byAuthor(author, id)
surface Books from ^books
    fields title, author
    collection ^books as list
    collection ^books.byAuthor as byAuthor
    create title, author
    update title
    delete
";
    let root = temp_project("surface-facts", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
    let facts = &program.facts;
    let module = facts.module_id("app").expect("app module");
    let book = facts.resource_id(module, "Book").expect("Book");
    let store = facts.store_id(module, "books").expect("^books");
    let title = facts.resource_member_id(book, &["title"]).expect("title");
    let author = facts.resource_member_id(book, &["author"]).expect("author");
    let by_author = facts
        .store_indexes()
        .iter()
        .find(|index| index.store == store && index.name == "byAuthor")
        .expect("byAuthor")
        .id;

    let [surface] = facts.surfaces() else {
        panic!("expected one surface, got {:#?}", facts.surfaces());
    };
    assert_eq!(facts.surface(surface.id), surface);
    assert_eq!(surface.module, module);
    assert_eq!(surface.name, "Books");
    assert_eq!(surface.store, store);
    assert_eq!(
        surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![
            SurfaceCatalogBlocker::PendingCatalogProposal,
            SurfaceCatalogBlocker::MissingAcceptedCatalogIds,
        ])
    );
    assert_eq!(
        surface
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.member))
            .collect::<Vec<_>>(),
        vec![("title", title), ("author", author)]
    );
    assert_eq!(
        surface
            .create
            .iter()
            .map(|field| (field.name.as_str(), field.member))
            .collect::<Vec<_>>(),
        vec![("title", title), ("author", author)]
    );
    assert_eq!(
        surface
            .update
            .iter()
            .map(|field| (field.name.as_str(), field.member))
            .collect::<Vec<_>>(),
        vec![("title", title)]
    );
    assert!(
        surface.delete.is_some(),
        "delete item resolves to a surface fact"
    );
    assert_eq!(
        surface
            .collections
            .iter()
            .map(|collection| (collection.alias.as_str(), collection.target))
            .collect::<Vec<_>>(),
        vec![
            ("list", SurfaceCollectionTarget::StoreRoot(store)),
            ("byAuthor", SurfaceCollectionTarget::StoreIndex(by_author)),
        ]
    );
}

#[test]
fn active_surface_create_rejects_required_unaddressable_backing_fields() {
    let source = "\
module app
resource Book
    title: string
    details
        required subtitle: string
store ^books(id: int): Book
surface Books from ^books
    fields title
    create title
";
    let root = temp_project("surface-create-required-unaddressable", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let fields = surface_fields(&report);
    assert_eq!(fields.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        fields[0].payload,
        DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
            list: SurfaceFieldList::Create,
            name: "details.subtitle".into(),
            problem: SurfaceFieldProblem::RequiredNotCreateAddressable,
        })
    );
}

#[test]
fn active_surface_create_rejects_missing_required_top_level_fields() {
    let source = "\
module app
resource Book
    required title: string
    required author: string
store ^books(id: int): Book
surface Books from ^books
    fields title, author
    create title
";
    let root = temp_project("surface-create-required-missing", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let fields = surface_fields(&report);
    assert_eq!(fields.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        fields[0].payload,
        DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
            list: SurfaceFieldList::Create,
            name: "author".into(),
            problem: SurfaceFieldProblem::RequiredNotCreateAddressable,
        })
    );
}

#[test]
fn surface_target_diagnostics_span_the_offending_target_token() {
    // A missing `from ^target` and a foreign `collection ^target` each point at
    // the `^target` token rather than collapsing to column 1 of the item line.
    let source = "\
module app
resource Book
    title: string
resource Author
    name: string
store ^books(id: int): Book
store ^authors(id: int): Author
surface Missing from ^nonexistent
    fields title
surface Foreign from ^books
    fields title
    collection ^authors as authors
";
    let root = temp_project("surface-target-spans", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_targets(&report);
    assert_eq!(diagnostics.len(), 2, "{:#?}", report.diagnostics);
    // `from ^nonexistent` on line 8: the caret sits at column 22.
    assert_eq!(diagnostics[0].span.line, 8, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].span.column, 22, "{diagnostics:#?}");
    // `collection ^authors` on line 12: the caret sits at column 16.
    assert_eq!(diagnostics[1].span.line, 12, "{diagnostics:#?}");
    assert_eq!(diagnostics[1].span.column, 16, "{diagnostics:#?}");
}

#[test]
fn ambiguous_resource_surface_target_spans_the_offending_target_token() {
    // An ambiguous backing resource points the diagnostic at the `^target` token
    // in the surface header rather than collapsing to column 1.
    let source = "\
module app
resource Book
    title: string
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
";
    let root = temp_project("surface-ambiguous-resource-span", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_targets(&report);
    assert_eq!(diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        matches!(
            diagnostics[0].payload,
            DiagnosticPayload::SurfaceTarget(
                SurfaceTargetDiagnostic::AmbiguousStoreResource { .. }
            )
        ),
        "{diagnostics:#?}"
    );
    // `from ^books` on line 7: the caret sits at column 20.
    assert_eq!(diagnostics[0].span.line, 7, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].span.column, 20, "{diagnostics:#?}");
}

#[test]
fn invalid_store_surface_target_spans_the_offending_target_token() {
    // An invalid backing store points the diagnostic at the `^target` token in the
    // surface header rather than collapsing to column 1.
    let source = "\
module app
resource Book
    title: string
store ^books(id: decimal): Book
surface Books from ^books
    fields title
";
    let root = temp_project("surface-invalid-store-span", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_targets(&report);
    assert_eq!(diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        matches!(
            diagnostics[0].payload,
            DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::InvalidStore { .. })
        ),
        "{diagnostics:#?}"
    );
    // `from ^books` on line 5: the caret sits at column 20.
    assert_eq!(diagnostics[0].span.line, 5, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].span.column, 20, "{diagnostics:#?}");
}

#[test]
fn surface_field_items_span_each_offending_name() {
    // Two bad field names on one `fields` line each earn a diagnostic at their own
    // column, so they are distinguishable rather than collapsed to column 1.
    let source = "\
module app
resource Book
    required title: string
store ^books(id: int): Book
surface Books from ^books
    fields nopeA, nopeB
";
    let root = temp_project("surface-field-item-spans", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let fields = surface_fields(&report);
    assert_eq!(fields.len(), 2, "{:#?}", report.diagnostics);
    assert_eq!(fields[0].span.line, 6, "{fields:#?}");
    assert_eq!(fields[0].span.column, 12, "{fields:#?}");
    assert_eq!(fields[1].span.column, 19, "{fields:#?}");
}

#[test]
fn surface_action_and_read_diagnostics_span_the_function_target() {
    // An unresolved action and computed-read target each anchor at the
    // function-target token, like field items, rather than column 1 of the line.
    let source = "\
module app
resource Book
    required title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
    action privateAction
    read ghostFn
";
    let root = temp_project("surface-function-target-spans", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let actions = surface_actions(&report);
    assert_eq!(actions.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!((actions[0].span.line, actions[0].span.column), (7, 12));

    let reads = surface_computed_reads(&report);
    assert_eq!(reads.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!((reads[0].span.line, reads[0].span.column), (8, 10));
}

#[test]
fn surface_facts_resolve_public_action_targets() {
    let app_source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn addBook()
    return
surface Books from ^books
    fields title
    action addBook
    action shelf::loanBook as loan
";
    let shelf_source = "\
module shelf
pub fn loanBook()
    return
";
    let root = temp_project("surface-action-facts", |root| {
        write(root, "src/app.mw", app_source);
        write(root, "src/shelf.mw", shelf_source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
    let facts = &program.facts;
    let app = facts.module_id("app").expect("app module");
    let shelf = facts.module_id("shelf").expect("shelf module");
    let add_book = facts.function_id(app, "addBook").expect("addBook");
    let loan_book = facts.function_id(shelf, "loanBook").expect("loanBook");
    let [surface] = facts.surfaces() else {
        panic!("expected one surface, got {:#?}", facts.surfaces());
    };
    assert_eq!(
        surface
            .actions
            .iter()
            .map(|action| {
                (
                    action.alias.as_str(),
                    facts
                        .function_id_for_ref(action.function)
                        .expect("action function"),
                )
            })
            .collect::<Vec<_>>(),
        vec![("addBook", add_book), ("loan", loan_book)]
    );
}

#[test]
fn surface_action_targets_expand_import_aliases() {
    let app_source = "\
module app
use library::shelf
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
    action shelf::loanBook as loan
";
    let shelf_source = "\
module library::shelf
pub fn loanBook()
    return
";
    let root = temp_project("surface-action-target-import-alias", |root| {
        write(root, "src/app.mw", app_source);
        write(root, "src/library/shelf.mw", shelf_source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
    let facts = &program.facts;
    let shelf = facts
        .module_id("library::shelf")
        .expect("library::shelf module");
    let loan_book = facts.function_id(shelf, "loanBook").expect("loanBook");
    let [surface] = facts.surfaces() else {
        panic!("expected one surface, got {:#?}", facts.surfaces());
    };
    let [action] = surface.actions.as_slice() else {
        panic!("expected one action, got {:#?}", surface.actions);
    };
    assert_eq!(action.alias, "loan");
    assert_eq!(
        facts
            .function_id_for_ref(action.function)
            .expect("action function"),
        loan_book
    );
}

#[test]
fn surface_action_target_in_incomplete_module_suppresses_unknown_function_noise() {
    let app_source = "\
module app
use library::shelf
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
    action shelf::loanBook as loan
";
    let shelf_source = "\
module library::shelf
\tpub fn loanBook()
    return
";
    let root = temp_project("surface-action-incomplete-target-module", |root| {
        write(root, "src/app.mw", app_source);
        write(root, "src/library/shelf.mw", shelf_source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        surface_actions(&report).is_empty(),
        "incomplete target module should not produce surface action noise: {:#?}",
        report.diagnostics
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "suppressed unresolved action target must not mint a partial surface fact"
    );
}

#[test]
fn surface_action_diagnostics_reject_unknown_and_private_targets() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
fn hidden()
    return
surface Books from ^books
    fields title
    action missing
    action hidden
";
    let root = temp_project("surface-action-target-diagnostics", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_actions(&report);
    assert_eq!(diagnostics.len(), 2, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceAction(SurfaceActionDiagnostic::UnknownFunction {
            path: "missing".into(),
        })
    );
    assert_eq!(
        diagnostics[1].payload,
        DiagnosticPayload::SurfaceAction(SurfaceActionDiagnostic::PrivateFunction {
            path: "hidden".into(),
        })
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a surface with invalid actions must not become a fact"
    );
}

#[test]
fn surface_action_diagnostics_reject_unsupported_signature_shapes() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn takesResource(book: Book)
    return
pub fn returnsResource(): Book
    var book: Book
    return book
pub fn takesIdentitySequence(ids: sequence[Id(^books)])
    return
surface Books from ^books
    fields title
    action takesResource
    action returnsResource
    action takesIdentitySequence
";
    let root = temp_project("surface-action-signature-diagnostics", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_actions(&report);
    assert_eq!(diagnostics.len(), 3, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceAction(SurfaceActionDiagnostic::UnsupportedParameter {
            path: "takesResource".into(),
            parameter: "book".into(),
        })
    );
    assert_eq!(
        diagnostics[1].payload,
        DiagnosticPayload::SurfaceAction(SurfaceActionDiagnostic::UnsupportedReturn {
            path: "returnsResource".into(),
        })
    );
    assert_eq!(
        diagnostics[2].payload,
        DiagnosticPayload::SurfaceAction(SurfaceActionDiagnostic::UnsupportedParameter {
            path: "takesIdentitySequence".into(),
            parameter: "ids".into(),
        })
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a surface with unsupported action signatures must not become a fact"
    );
}

#[test]
fn surface_facts_resolve_computed_read_targets() {
    let source = "\
module app
resource BookPage
    required title: string
    author: string
resource Book
    title: string
store ^books(id: int): Book
pub fn bookPage(id: Id(^books)): maybe BookPage
    return absent
surface Books from ^books
    fields title
    read bookPage as page
";
    let root = temp_project("surface-computed-read-facts", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
    let facts = &program.facts;
    let app = facts.module_id("app").expect("app module");
    let book_page = facts.function_id(app, "bookPage").expect("bookPage");
    let [surface] = facts.surfaces() else {
        panic!("expected one surface, got {:#?}", facts.surfaces());
    };
    let [computed] = surface.computed_reads.as_slice() else {
        panic!(
            "expected one computed read, got {:#?}",
            surface.computed_reads
        );
    };
    assert_eq!(computed.alias, "page");
    assert_eq!(
        facts
            .function_id_for_ref(computed.function)
            .expect("computed read function"),
        book_page
    );
}

#[test]
fn surface_computed_read_catalog_dependent_shapes_are_source_only_not_signature_errors() {
    let source = "\
module app
resource BookPage
    required title: string
resource Book
    title: string
store ^books(id: int): Book
pub fn page(id: Id(^books)): BookPage
    return BookPage(title: \"\")
surface Books from ^books
    fields title
    read page
";
    let root = temp_project("surface-computed-read-catalog-dependent-shapes", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
    assert!(
        surface_computed_reads(&report).is_empty(),
        "catalog-dependent shapes should not be signature errors: {:#?}",
        report.diagnostics
    );
    let [surface] = program.facts.surfaces() else {
        panic!("expected one surface, got {:#?}", program.facts.surfaces());
    };
    assert_eq!(
        surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![
            SurfaceCatalogBlocker::PendingCatalogProposal,
            SurfaceCatalogBlocker::MissingAcceptedCatalogIds,
        ])
    );
    assert_eq!(surface.computed_reads.len(), 1);
}

#[test]
fn surface_computed_read_diagnostics_reject_unknown_and_private_targets() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
fn hidden(): maybe string
    return absent
surface Books from ^books
    fields title
    read missing
    read hidden
";
    let root = temp_project("surface-computed-read-target-diagnostics", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_computed_reads(&report);
    assert_eq!(diagnostics.len(), 2, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceComputedRead(SurfaceComputedReadDiagnostic::UnknownFunction {
            path: "missing".into(),
        })
    );
    assert_eq!(
        diagnostics[1].payload,
        DiagnosticPayload::SurfaceComputedRead(SurfaceComputedReadDiagnostic::PrivateFunction {
            path: "hidden".into(),
        })
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a surface with invalid computed reads must not become a fact"
    );
}

#[test]
fn surface_computed_read_diagnostics_reject_unsupported_signature_shapes() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn noResult()
    return
pub fn takesResource(book: Book): string
    return \"\"
pub fn returnsError(): Error
    return Error(code: \"app.error\", message: \"hidden\")
surface Books from ^books
    fields title
    read noResult
    read takesResource
    read returnsError
";
    let root = temp_project("surface-computed-read-signature-diagnostics", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_computed_reads(&report);
    assert_eq!(diagnostics.len(), 3, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceComputedRead(SurfaceComputedReadDiagnostic::UnsupportedReturn {
            path: "noResult".into(),
        })
    );
    assert_eq!(
        diagnostics[1].payload,
        DiagnosticPayload::SurfaceComputedRead(
            SurfaceComputedReadDiagnostic::UnsupportedParameter {
                path: "takesResource".into(),
                parameter: "book".into(),
            },
        )
    );
    assert_eq!(
        diagnostics[2].payload,
        DiagnosticPayload::SurfaceComputedRead(SurfaceComputedReadDiagnostic::UnsupportedReturn {
            path: "returnsError".into(),
        })
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a surface with unsupported computed-read signatures must not become a fact"
    );
}

#[test]
fn surface_computed_read_diagnostics_reject_effects() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn writes(): string
    ^books(1).title = \"x\"
    return \"x\"
pub fn logs(): string
    print(\"x\")
    return \"x\"
pub fn fails(): string
    throw Error(code: \"app.error\", message: \"hidden\")
surface Books from ^books
    fields title
    read writes as writePreview
    read logs
    read fails
";
    let root = temp_project("surface-computed-read-effect-diagnostics", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_computed_reads(&report);
    assert_eq!(diagnostics.len(), 3, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceComputedRead(SurfaceComputedReadDiagnostic::Writes {
            path: "writes".into(),
        })
    );
    assert_eq!(
        diagnostics[1].payload,
        DiagnosticPayload::SurfaceComputedRead(SurfaceComputedReadDiagnostic::HostEffects {
            path: "logs".into(),
        })
    );
    assert_eq!(
        diagnostics[2].payload,
        DiagnosticPayload::SurfaceComputedRead(SurfaceComputedReadDiagnostic::Throws {
            path: "fails".into(),
        })
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a surface with effectful computed reads must not become a fact"
    );
}

#[test]
fn surface_computed_read_effect_diagnostics_anchor_at_the_target_token() {
    // A computed read whose function throws is rejected by the effect-closure pass. The diagnostic
    // must point at the read's target token, matching the resolution and signature variants, not at
    // column 1 of the surface item line.
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn fails(): string
    throw Error(code: \"app.error\", message: \"hidden\")
surface Books from ^books
    fields title
    read fails
";
    let root = temp_project("surface-computed-read-effect-span", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_computed_reads(&report);
    assert_eq!(diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceComputedRead(SurfaceComputedReadDiagnostic::Throws {
            path: "fails".into(),
        })
    );
    // `read fails` is on line 9; the target token `fails` starts at column 10.
    assert_eq!(
        (diagnostics[0].span.line, diagnostics[0].span.column),
        (9, 10),
        "the effect-closure diagnostic must anchor at the computed-read target token: {:#?}",
        diagnostics[0]
    );
}

#[test]
fn surface_computed_read_unindexed_scan_spans_the_traversal_not_the_surface_decl() {
    // The computed read `summary` is clean itself; it calls `tally`, whose `for` loop scans the
    // whole `^books` collection unindexed. The diagnostic must point at that traversal so the dev
    // fixes the loop, not the surface declaration that merely names the read.
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
fn tally(): int
    var total: int = 0
    for id in ^books
        total = total + 1
    return total
pub fn summary(): int
    return tally()
surface Books from ^books
    fields title
    read summary
";
    let root = temp_project("surface-computed-read-unindexed-span", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_computed_reads(&report);
    assert_eq!(diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceComputedRead(
            SurfaceComputedReadDiagnostic::UnindexedCollectionRead {
                path: "summary".into(),
            }
        )
    );
    // The `for id in ^books` traversal is on line 7; the surface declaration is line 12. The span
    // must resolve to the traversal site inside the function body.
    assert_eq!(
        diagnostics[0].span.line, 7,
        "the unindexed-read diagnostic must point at the traversal, not the surface decl: {:#?}",
        diagnostics[0]
    );
}

#[test]
fn surface_computed_read_alias_collides_with_generated_operations() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn page(): maybe string
    return absent
surface Books from ^books
    fields title
    read page as create
";
    let root = temp_project("surface-computed-read-alias-collision", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    let collisions = surface_collisions(&report);
    assert_eq!(collisions.len(), 1, "{:#?}", report.diagnostics);
    assert_surface_collision_payload(
        collisions[0],
        "create",
        SurfaceCollisionNameKind::GeneratedOperation,
        SurfaceCollisionNameKind::ComputedReadAlias,
        source,
        7,
    );
    assert!(
        program.facts.surfaces().is_empty(),
        "a rejected computed-read alias collision must not mint a surface fact"
    );
}

#[test]
fn surface_read_operations_cover_backing_and_collections() {
    let source = "\
module app
resource Book
    required title: string
    author: string
    isbn: string
store ^books(shelf: string, id: int): Book
    index byAuthor(author, shelf, id)
    index byIsbn(isbn) unique
surface Books from ^books
    fields title, author
    collection ^books as list
    collection ^books.byAuthor as byAuthor
    collection ^books.byIsbn as byIsbn
";
    let root = temp_project("surface-read-operations", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
    let facts = &program.facts;
    let module = facts.module_id("app").expect("app module");
    let book = facts.resource_id(module, "Book").expect("Book");
    let store = facts.store_id(module, "books").expect("^books");
    let title = facts.resource_member_id(book, &["title"]).expect("title");
    let author = facts.resource_member_id(book, &["author"]).expect("author");
    let by_author = facts
        .store_indexes()
        .iter()
        .find(|index| index.store == store && index.name == "byAuthor")
        .expect("byAuthor")
        .id;
    let by_isbn = facts
        .store_indexes()
        .iter()
        .find(|index| index.store == store && index.name == "byIsbn")
        .expect("byIsbn")
        .id;

    let [surface] = facts.surfaces() else {
        panic!("expected one surface, got {:#?}", facts.surfaces());
    };
    let projection = vec![title, author];
    assert_eq!(
        surface
            .read_operations
            .iter()
            .map(|operation| (
                operation.kind,
                operation.footprint,
                operation.projection.clone()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                SurfaceReadOperationKind::PointRead { store },
                SurfaceReadFootprint::FullRecord { resource: book },
                projection.clone(),
            ),
            (
                SurfaceReadOperationKind::PagedRootCollection { store },
                SurfaceReadFootprint::FullRecord { resource: book },
                projection.clone(),
            ),
            (
                SurfaceReadOperationKind::PagedIndexCollection {
                    index: by_author,
                    exact_key_count: 1,
                    identity_key_count: 2,
                },
                SurfaceReadFootprint::FullRecord { resource: book },
                projection.clone(),
            ),
            (
                SurfaceReadOperationKind::UniqueIndexLookup {
                    index: by_isbn,
                    key_count: 1,
                },
                SurfaceReadFootprint::FullRecord { resource: book },
                projection,
            ),
        ]
    );
}

#[test]
fn keyless_surface_read_operation_is_singleton() {
    let source = "\
module app
resource Settings
    required maxLoans: int
    theme: string
store ^settings: Settings
surface SettingsSurface from ^settings
    fields theme
";
    let root = temp_project("surface-read-operation-singleton", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, program) = check_project(&root, &config()).expect("check");

    assert_clean(&report);
    let facts = &program.facts;
    let module = facts.module_id("app").expect("app module");
    let settings = facts.resource_id(module, "Settings").expect("Settings");
    let store = facts.store_id(module, "settings").expect("^settings");
    let theme = facts
        .resource_member_id(settings, &["theme"])
        .expect("theme");

    let [surface] = facts.surfaces() else {
        panic!("expected one surface, got {:#?}", facts.surfaces());
    };
    assert_eq!(
        surface
            .read_operations
            .iter()
            .map(|operation| (
                operation.kind,
                operation.footprint,
                operation.projection.clone()
            ))
            .collect::<Vec<_>>(),
        vec![(
            SurfaceReadOperationKind::SingletonRead { store },
            SurfaceReadFootprint::FullRecord { resource: settings },
            vec![theme],
        )]
    );
}

#[test]
fn surface_catalog_status_is_stable_only_with_accepted_ids() {
    let source = "\
module app
resource Book
    required title: string
    author: string
store ^books(id: int): Book
    index byAuthor(author, id)
surface Books from ^books
    fields title, author
    collection ^books.byAuthor as byAuthor
";
    let root = temp_project("surface-catalog-ready", |root| {
        write(root, "src/app.mw", source);
    });
    let (baseline_report, baseline_program) =
        check_project(&root, &config()).expect("baseline check");
    assert_clean(&baseline_report);
    let baseline = baseline_program
        .catalog
        .proposal
        .expect("first run proposes accepted baseline");
    crate::support::catalog::write_catalog(&root, &baseline);
    let (report, program) = check_with_accepted(&root);

    assert_clean(&report);
    assert!(
        program.catalog.proposal.is_none(),
        "accepted baseline should match current source"
    );
    let [surface] = program.facts.surfaces() else {
        panic!("expected one surface, got {:#?}", program.facts.surfaces());
    };
    assert_eq!(surface.catalog_status, SurfaceCatalogStatus::Stable);
    assert!(
        surface.read_operations.iter().all(|operation| operation
            .operation_tag
            .as_deref()
            .is_some_and(|tag| tag.starts_with("sha256:"))),
        "stable surface read operations must carry checked operation tags"
    );
}

#[test]
fn surface_catalog_status_checks_collection_index_key_members() {
    let root = temp_project("surface-index-key-member-catalog-status", |root| {
        write(
            root,
            "src/app.mw",
            "\
module app
resource Book
    title: string
store ^books(id: int): Book
    index byLookup(title, id)
surface Books from ^books
    fields title
    collection ^books.byLookup as byLookup
",
        );
    });
    let (baseline_report, baseline_program) =
        check_project(&root, &config()).expect("baseline check");
    assert_clean(&baseline_report);
    let baseline = baseline_program
        .catalog
        .proposal
        .expect("first run proposes accepted baseline");
    crate::support::catalog::write_catalog(&root, &baseline);

    write(
        &root,
        "src/app.mw",
        "\
module app
resource Book
    title: string
    author: string
store ^books(id: int): Book
    index byLookup(author, id)
surface Books from ^books
    fields title
    collection ^books.byLookup as byLookup
",
    );
    let (report, program) = check_with_accepted(&root);

    assert_clean(&report);
    assert!(
        program.catalog.proposal.is_some(),
        "index shape drift should produce a pending catalog proposal"
    );
    let [surface] = program.facts.surfaces() else {
        panic!("expected one surface, got {:#?}", program.facts.surfaces());
    };
    assert_eq!(
        surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![
            SurfaceCatalogBlocker::PendingCatalogProposal,
            SurfaceCatalogBlocker::MissingAcceptedCatalogIds,
        ])
    );
}

#[test]
fn surface_target_diagnostics_reject_missing_and_foreign_roots_and_indexes() {
    let source = "\
module app
resource Book
    title: string
resource Author
    name: string
store ^books(id: int): Book
    index byTitle(title, id)
store ^authors(id: int): Author
surface Missing from ^missing
    fields title
surface Foreign from ^books
    fields title
    collection ^authors as authors
surface MissingIndex from ^books
    fields title
    collection ^books.byMissing as byMissing
";
    let root = temp_project("surface-target-diagnostics", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_targets(&report);
    assert_eq!(diagnostics.len(), 3, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::UnknownStore {
            root: "missing".into(),
        })
    );
    assert_eq!(
        diagnostics[1].payload,
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::ForeignCollectionRoot {
            surface_root: "books".into(),
            target_root: "authors".into(),
        })
    );
    assert_eq!(
        diagnostics[2].payload,
        DiagnosticPayload::SurfaceTarget(SurfaceTargetDiagnostic::UnknownCollectionIndex {
            root: "books".into(),
            index: "byMissing".into(),
        })
    );
}

#[test]
fn surface_field_diagnostics_reject_unsupported_and_unprojected_payloads() {
    let source = "\
module app
resource Book
    required title: string
    meta
        subtitle: string
    tags(pos: int): string
    author: string
store ^books(id: int): Book
    index byAuthor(author, id)
surface Books from ^books
    fields title, meta, tags, id, byAuthor
    create title, author
    update author
";
    let root = temp_project("surface-field-diagnostics", |root| {
        write(root, "src/app.mw", source);
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    let diagnostics = surface_fields(&report);
    assert_eq!(diagnostics.len(), 6, "{:#?}", report.diagnostics);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| &diagnostic.payload)
            .collect::<Vec<_>>(),
        vec![
            &DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                list: SurfaceFieldList::Fields,
                name: "meta".into(),
                problem: SurfaceFieldProblem::Unsupported,
            }),
            &DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                list: SurfaceFieldList::Fields,
                name: "tags".into(),
                problem: SurfaceFieldProblem::Unsupported,
            }),
            &DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                list: SurfaceFieldList::Fields,
                name: "id".into(),
                problem: SurfaceFieldProblem::IdentityKey,
            }),
            &DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                list: SurfaceFieldList::Fields,
                name: "byAuthor".into(),
                problem: SurfaceFieldProblem::Unknown,
            }),
            &DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                list: SurfaceFieldList::Create,
                name: "author".into(),
                problem: SurfaceFieldProblem::NotProjected,
            }),
            &DiagnosticPayload::SurfaceField(SurfaceFieldDiagnostic {
                list: SurfaceFieldList::Update,
                name: "author".into(),
                problem: SurfaceFieldProblem::NotProjected,
            }),
        ]
    );
    let identity_message = &diagnostics
        .iter()
        .find(|diagnostic| match &diagnostic.payload {
            DiagnosticPayload::SurfaceField(field) => {
                field.problem == SurfaceFieldProblem::IdentityKey
            }
            _ => false,
        })
        .expect("an identity-key surface field diagnostic")
        .message;
    assert!(
        identity_message.contains("identity") && identity_message.contains("automatic"),
        "the identity-key message must explain that identity is returned automatically: {identity_message}"
    );
}
