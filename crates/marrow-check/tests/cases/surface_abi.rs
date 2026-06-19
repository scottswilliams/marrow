use crate::support::catalog::{catalog, entry, write_catalog};
use crate::support::{assert_clean, check_with_accepted, config, temp_project, write};

use marrow_catalog::{CatalogEntry, CatalogEntryKind};
use marrow_check::{
    AnalysisSnapshot, CheckedProgram, ProjectSources, SurfaceCatalogBlocker, SurfaceCatalogStatus,
    SurfaceReadOperationDescriptorKind, SurfaceReadOperationValueShape, analyze_project,
    check_project,
};
use marrow_schema::ScalarType;
use marrow_store::cell::CatalogId;

fn stable_snapshot(name: &str, source: &str) -> (crate::support::TempProject, AnalysisSnapshot) {
    let root = temp_project(name, |root| write(root, "src/app.mw", source));
    let (baseline_report, baseline_program) =
        check_project(&root, &config()).expect("baseline check");
    assert_clean(&baseline_report);
    let baseline = baseline_program
        .catalog
        .proposal
        .expect("first run proposes catalog ids");
    write_catalog(&root, &baseline);
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&baseline))
        .expect("stable analysis");
    assert_clean(&snapshot.report);
    (root, snapshot)
}

fn read_tags(program: &CheckedProgram) -> Vec<String> {
    program.facts.surfaces()[0]
        .read_operations
        .iter()
        .map(|operation| {
            operation
                .operation_tag
                .clone()
                .expect("stable operation tag")
        })
        .collect()
}

fn catalog_id(raw: &str) -> CatalogId {
    CatalogId::new(raw.to_string()).expect("valid catalog id")
}

#[test]
fn surface_read_operation_tag_keeps_v1_bytes_stable() {
    let root = temp_project("surface-read-abi-tag-stability", |root| {
        write(
            root,
            "src/app.mw",
            "\
module app
resource Book
    required title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
",
        );
        write_catalog(
            root,
            &catalog(vec![
                entry(
                    CatalogEntryKind::Resource,
                    "app::Book",
                    "cat_00000000000000000000000000000001",
                    &[],
                ),
                CatalogEntry {
                    accepted_key_shape: Some("int".to_string()),
                    ..entry(
                        CatalogEntryKind::Store,
                        "app::^books",
                        "cat_00000000000000000000000000000002",
                        &[],
                    )
                },
                CatalogEntry {
                    accepted_struct: Some("leaf:string".to_string()),
                    ..entry(
                        CatalogEntryKind::ResourceMember,
                        "app::Book::title",
                        "cat_00000000000000000000000000000003",
                        &[],
                    )
                },
            ]),
        );
    });
    let (report, program) = check_with_accepted(&root);
    assert_clean(&report);
    assert_eq!(program.catalog.proposal, None);

    let tags = read_tags(&program);

    assert_eq!(
        tags,
        vec!["sha256:7c42c1bd5a21558a53d30e812b039e181d026dc3a4fbabca7d0f8b56148b42e7".to_string()],
        "surface.read.v1 cursor tags are a live runtime/json contract"
    );
}

#[test]
fn stable_read_operation_descriptor_exports_catalog_bound_shapes() {
    let source = "\
module app
enum Status
    draft
    published
resource Book
    required title: string
    status: Status
    parent: Id(^books)
store ^books(shelf: string, id: int): Book
    index byStatus(status, shelf, id)
    index byParent(parent) unique
surface Books from ^books
    fields title, status, parent
    collection ^books as list
    collection ^books.byStatus as byStatus
    collection ^books.byParent as byParent
";
    let (_root, snapshot) = stable_snapshot("surface-read-abi-shapes", source);
    let program = &snapshot.program;
    let descriptors = snapshot
        .surface_read_operations()
        .map(|operation| operation.stable_descriptor().expect("stable descriptor"))
        .collect::<Vec<_>>();

    assert_eq!(descriptors.len(), 4, "{descriptors:#?}");
    assert!(descriptors.iter().all(|descriptor| {
        descriptor.profile_version == "surface.read.v1"
            && descriptor.operation_tag.starts_with("sha256:")
    }));

    let facts = &program.facts;
    let module = facts.module_id("app").expect("module");
    let book = facts.resource_id(module, "Book").expect("Book");
    let store = facts.store_id(module, "books").expect("^books");
    let title = facts.resource_member_id(book, &["title"]).expect("title");
    let status = facts.resource_member_id(book, &["status"]).expect("status");
    let parent = facts.resource_member_id(book, &["parent"]).expect("parent");
    let status_enum = facts.enum_id(module, "Status").expect("Status");
    let by_status = facts
        .store_indexes()
        .iter()
        .find(|index| index.store == store && index.name == "byStatus")
        .expect("byStatus");
    let by_parent = facts
        .store_indexes()
        .iter()
        .find(|index| index.store == store && index.name == "byParent")
        .expect("byParent");
    let surface = &program.facts.surfaces()[0];
    assert_eq!(
        descriptors
            .iter()
            .map(|descriptor| descriptor.operation_tag.as_str())
            .collect::<Vec<_>>(),
        surface
            .read_operations
            .iter()
            .map(|operation| operation.operation_tag.as_deref().expect("tag"))
            .collect::<Vec<_>>()
    );

    let point = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.kind,
                SurfaceReadOperationDescriptorKind::PointRead
            )
        })
        .expect("point read");
    assert_eq!(
        point.store_catalog_id,
        catalog_id(facts.store(store).catalog_id.as_deref().expect("store id"))
    );
    assert_eq!(
        point.resource_catalog_id,
        catalog_id(
            facts
                .resource(book)
                .catalog_id
                .as_deref()
                .expect("resource id")
        )
    );
    assert_eq!(
        point
            .identity_keys
            .iter()
            .map(|key| &key.value)
            .collect::<Vec<_>>(),
        vec![
            &SurfaceReadOperationValueShape::Scalar(ScalarType::Str),
            &SurfaceReadOperationValueShape::Scalar(ScalarType::Int),
        ]
    );
    assert_eq!(
        point
            .projection
            .iter()
            .map(|field| field.member_catalog_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            facts.resource_members()[title.0 as usize]
                .catalog_id
                .as_deref()
                .expect("title id"),
            facts.resource_members()[status.0 as usize]
                .catalog_id
                .as_deref()
                .expect("status id"),
            facts.resource_members()[parent.0 as usize]
                .catalog_id
                .as_deref()
                .expect("parent id"),
        ]
    );

    let by_status_descriptor = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.kind,
                SurfaceReadOperationDescriptorKind::PagedIndexCollection { .. }
            )
        })
        .expect("paged index operation");
    let SurfaceReadOperationDescriptorKind::PagedIndexCollection {
        index_catalog_id,
        exact_key_count,
        identity_key_count,
    } = &by_status_descriptor.kind
    else {
        unreachable!();
    };
    assert_eq!(
        index_catalog_id,
        &catalog_id(by_status.catalog_id.as_deref().expect("byStatus id"))
    );
    assert_eq!((*exact_key_count, *identity_key_count), (1, 2));
    assert!(by_status_descriptor.index_keys.iter().any(
        |key| matches!(&key.value, SurfaceReadOperationValueShape::Enum {
                enum_catalog_id,
                member_catalog_ids,
            } if enum_catalog_id.as_str() == facts.enum_(status_enum).unwrap().catalog_id.as_deref().unwrap()
                && member_catalog_ids.len() == 2)
    ));

    let unique = descriptors
        .iter()
        .find(|descriptor| {
            matches!(
                descriptor.kind,
                SurfaceReadOperationDescriptorKind::UniqueIndexLookup { .. }
            )
        })
        .expect("unique lookup operation");
    let SurfaceReadOperationDescriptorKind::UniqueIndexLookup {
        index_catalog_id, ..
    } = &unique.kind
    else {
        unreachable!();
    };
    assert_eq!(
        index_catalog_id,
        &catalog_id(by_parent.catalog_id.as_deref().expect("byParent id"))
    );
    assert!(unique.index_keys.iter().any(
        |key| matches!(&key.value, SurfaceReadOperationValueShape::Identity {
                store_catalog_id,
                key_scalars,
                ..
            } if store_catalog_id == &point.store_catalog_id && key_scalars.len() == 2)
    ));
}

#[test]
fn source_only_surface_analysis_has_no_stable_descriptor() {
    let root = temp_project("surface-read-abi-source-only", |root| {
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
",
        );
    });
    let snapshot =
        analyze_project(&root, &config(), &ProjectSources::new(), None).expect("analyze");
    assert_clean(&snapshot.report);

    let operations = snapshot.surface_read_operations().collect::<Vec<_>>();
    let [operation] = operations.as_slice() else {
        panic!("expected one operation, got {operations:#?}");
    };
    assert!(operation.stable_descriptor().is_none());
    assert!(operation.operation.operation_tag.is_none());
    assert_eq!(
        operation.surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![
            SurfaceCatalogBlocker::PendingCatalogProposal,
            SurfaceCatalogBlocker::MissingAcceptedCatalogIds,
        ])
    );
}

#[test]
fn pending_catalog_proposal_clears_operation_tags_even_when_ids_exist() {
    let root = temp_project("surface-read-abi-pending-proposal-tags", |root| {
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
",
        );
    });
    let (baseline_report, baseline_program) =
        check_project(&root, &config()).expect("baseline check");
    assert_clean(&baseline_report);
    let baseline = baseline_program
        .catalog
        .proposal
        .expect("first run proposes catalog ids");
    write_catalog(&root, &baseline);
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
",
    );
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&baseline))
        .expect("analyze pending proposal");
    assert_clean(&snapshot.report);

    let operations = snapshot.surface_read_operations().collect::<Vec<_>>();
    let [operation] = operations.as_slice() else {
        panic!("expected one operation, got {operations:#?}");
    };
    assert_eq!(
        operation.surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![SurfaceCatalogBlocker::PendingCatalogProposal])
    );
    assert!(operation.operation.operation_tag.is_none());
    assert!(operation.stable_descriptor().is_none());
}

#[test]
fn render_labels_do_not_change_operation_tags_but_projection_order_does() {
    let source = "\
module app
resource Book
    title: string
    author: string
store ^books(id: int): Book
    index byAuthor(author, id)
surface Books from ^books
    fields title, author
    collection ^books.byAuthor as byAuthor
";
    let (root, snapshot) = stable_snapshot("surface-read-abi-labels", source);
    let original = read_tags(&snapshot.program);

    write(
        &root,
        "src/app.mw",
        "\
module app
resource Book
    title: string
    author: string
store ^books(id: int): Book
    index byAuthor(author, id)
surface Library from ^books
    fields title, author
    collection ^books.byAuthor as writers
",
    );
    let (renamed_report, renamed) = check_with_accepted(&root);
    assert_clean(&renamed_report);
    assert_eq!(read_tags(&renamed), original);

    write(
        &root,
        "src/app.mw",
        "\
module app
resource Book
    title: string
    author: string
store ^books(id: int): Book
    index byAuthor(author, id)
surface Library from ^books
    fields author, title
    collection ^books.byAuthor as writers
",
    );
    let (reordered_report, reordered) = check_with_accepted(&root);
    assert_clean(&reordered_report);
    assert_ne!(read_tags(&reordered), original);
}
