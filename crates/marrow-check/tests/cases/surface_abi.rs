use crate::support::catalog::{catalog, entry, write_catalog};
use crate::support::{assert_clean, check_with_accepted, config, temp_project, write};

use marrow_catalog::{CatalogEntry, CatalogEntryKind};
use marrow_check::{
    AnalysisSnapshot, CheckedProgram, ENTRY_PROTOCOL_TAG_VERSION, EntryDescriptor, ProjectSources,
    SurfaceActionOperationDescriptor, SurfaceCatalogBlocker, SurfaceCatalogStatus,
    SurfaceOperationValueShape, SurfaceReadOperationDescriptor, SurfaceReadOperationDescriptorKind,
    SurfaceUpdateOperationDescriptor, SurfaceUpdateOperationDescriptorKind, analyze_project,
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

fn update_tag(snapshot: &AnalysisSnapshot) -> String {
    snapshot
        .surface_update_operations()
        .next()
        .and_then(|operation| operation.stable_descriptor())
        .map(|descriptor| descriptor.operation_tag)
        .expect("stable update operation tag")
}

fn update_tags_by_surface(snapshot: &AnalysisSnapshot) -> Vec<(String, String)> {
    snapshot
        .surface_update_operations()
        .map(|operation| {
            (
                operation.surface.name.clone(),
                operation
                    .stable_descriptor()
                    .expect("stable update descriptor")
                    .operation_tag,
            )
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
fn surface_update_operation_tag_keeps_v1_bytes_stable() {
    let root = temp_project("surface-update-abi-tag-stability", |root| {
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
    update title
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(
            &marrow_catalog::CatalogMetadata::from_json(
                &std::fs::read_to_string(root.join("marrow.catalog.json")).expect("read catalog"),
            )
            .expect("catalog parses"),
        ),
    )
    .expect("stable analysis");
    assert_clean(&snapshot.report);

    assert_eq!(
        update_tag(&snapshot),
        "sha256:edd696ee0d6ef59e2619f9684d21e755ecd5ceca28301039c53d038d0aeaa453",
        "surface.update.v1 tags are a live runtime/json contract"
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
    assert_eq!(
        descriptors
            .iter()
            .map(|descriptor| descriptor.alias.as_str())
            .collect::<Vec<_>>(),
        vec!["get", "list", "byStatus", "byParent"]
    );

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
            &SurfaceOperationValueShape::Scalar(ScalarType::Str),
            &SurfaceOperationValueShape::Scalar(ScalarType::Int),
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
        |key| matches!(&key.value, SurfaceOperationValueShape::Enum {
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
        |key| matches!(&key.value, SurfaceOperationValueShape::Identity {
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

    let updates = snapshot.surface_update_operations().collect::<Vec<_>>();
    assert!(
        updates.is_empty(),
        "source-only surface without update fields has no update operation"
    );

    let actions = snapshot.surface_action_operations().collect::<Vec<_>>();
    assert!(
        actions.is_empty(),
        "source-only surface without action items has no action operation"
    );
}

#[test]
fn stable_action_descriptor_reuses_entry_invoke_identity() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn addBook(title: string)
    return
surface Books from ^books
    fields title
    action addBook
";
    let (_root, snapshot) = stable_snapshot("surface-action-abi-entry-identity", source);
    let descriptors = snapshot
        .surface_action_operations()
        .map(|operation| {
            operation
                .stable_descriptor()
                .expect("stable action descriptor")
        })
        .collect::<Vec<_>>();

    let [descriptor] = descriptors.as_slice() else {
        panic!("expected one action descriptor, got {descriptors:#?}");
    };
    let entry = EntryDescriptor::resolve(&snapshot.program.runtime(), "app::addBook")
        .expect("entry descriptor");
    assert_eq!(descriptor.profile_version, ENTRY_PROTOCOL_TAG_VERSION);
    assert_eq!(descriptor.alias, "addBook");
    assert_eq!(descriptor.operation_tag, entry.identity.entry_tag);
    assert_eq!(descriptor.identity, entry.identity);
    assert_eq!(descriptor.parameters, entry.parameters);
    assert_eq!(descriptor.return_value, entry.return_value);
}

#[test]
fn stable_action_operation_tag_changes_when_return_type_changes() {
    let int_source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn addBook(): int
    return 1
surface Books from ^books
    fields title
    action addBook
";
    let string_source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn addBook(): string
    return \"one\"
surface Books from ^books
    fields title
    action addBook
";
    let (_root, int_snapshot) = stable_snapshot("surface-action-abi-return-int", int_source);
    let (_root, string_snapshot) =
        stable_snapshot("surface-action-abi-return-string", string_source);
    let int_tag = int_snapshot
        .surface_action_operations()
        .next()
        .and_then(|operation| operation.stable_descriptor())
        .map(|descriptor| descriptor.operation_tag)
        .expect("int action tag");
    let string_tag = string_snapshot
        .surface_action_operations()
        .next()
        .and_then(|operation| operation.stable_descriptor())
        .map(|descriptor| descriptor.operation_tag)
        .expect("string action tag");

    assert_ne!(
        int_tag, string_tag,
        "action operation identity must cover response shape"
    );
}

#[test]
fn source_only_surface_action_analysis_has_no_stable_descriptor() {
    let root = temp_project("surface-action-abi-source-only", |root| {
        write(
            root,
            "src/app.mw",
            "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn addBook()
    return
surface Books from ^books
    fields title
    action addBook
",
        );
    });
    let snapshot =
        analyze_project(&root, &config(), &ProjectSources::new(), None).expect("analyze");
    assert_clean(&snapshot.report);

    let actions = snapshot.surface_action_operations().collect::<Vec<_>>();
    let [action] = actions.as_slice() else {
        panic!("expected one action operation, got {actions:#?}");
    };
    assert!(action.stable_descriptor().is_none());
    assert!(
        SurfaceActionOperationDescriptor::from_action(
            &snapshot.program,
            action.surface,
            action.action
        )
        .is_none(),
        "the descriptor owner must enforce the source-only gate"
    );
}

#[test]
fn action_catalog_dependencies_can_make_a_surface_source_only() {
    let root = temp_project("surface-action-abi-action-catalog-dependency", |root| {
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
enum Status
    active
resource Book
    title: string
store ^books(id: int): Book
pub fn currentStatus(): Status
    return Status::active
surface Books from ^books
    fields title
    action currentStatus
",
    );
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&baseline))
        .expect("analyze changed action");
    assert_clean(&snapshot.report);

    let actions = snapshot.surface_action_operations().collect::<Vec<_>>();
    let [action] = actions.as_slice() else {
        panic!("expected one action operation, got {actions:#?}");
    };
    assert_eq!(
        action.surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![
            SurfaceCatalogBlocker::PendingCatalogProposal,
            SurfaceCatalogBlocker::MissingAcceptedCatalogIds,
        ])
    );
    assert!(action.stable_descriptor().is_none());
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
    assert!(
        SurfaceReadOperationDescriptor::from_operation(
            &snapshot.program,
            operation.surface,
            operation.operation
        )
        .is_none(),
        "the descriptor owner must enforce the source-only gate"
    );
}

#[test]
fn pending_catalog_proposal_suppresses_update_descriptor_even_when_ids_exist() {
    let root = temp_project("surface-update-abi-pending-proposal-tags", |root| {
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
    update title
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
    update title
",
    );
    let snapshot = analyze_project(&root, &config(), &ProjectSources::new(), Some(&baseline))
        .expect("analyze pending proposal");
    assert_clean(&snapshot.report);

    let updates = snapshot.surface_update_operations().collect::<Vec<_>>();
    let [update] = updates.as_slice() else {
        panic!("expected one update operation, got {updates:#?}");
    };
    assert_eq!(
        update.surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![SurfaceCatalogBlocker::PendingCatalogProposal])
    );
    assert!(update.stable_descriptor().is_none());
    assert!(
        SurfaceUpdateOperationDescriptor::from_surface(&snapshot.program, update.surface).is_none(),
        "the descriptor owner must enforce the source-only gate"
    );
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

#[test]
fn read_and_update_tags_do_not_collide_for_the_same_surface_fields() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
    update title
";
    let (_root, snapshot) = stable_snapshot("surface-update-abi-read-update-domain", source);
    let read = read_tags(&snapshot.program);
    let update = update_tag(&snapshot);

    assert_eq!(read.len(), 1);
    assert_ne!(read[0], update);
}

#[test]
fn update_operation_tag_includes_surface_shape_and_store_identity() {
    let source = "\
module app
resource Settings
    mode: string
store ^settings: Settings
surface SettingsSurface from ^settings
    fields mode
    update mode

resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
    update title

resource Note
    title: string
store ^notes(id: int): Note
surface Notes from ^notes
    fields title
    update title
";
    let (_root, snapshot) = stable_snapshot("surface-update-abi-shape-identity", source);
    let mut tags = update_tags_by_surface(&snapshot);
    tags.sort_by(|left, right| left.0.cmp(&right.0));

    let books = tags
        .iter()
        .find(|(surface, _)| surface == "Books")
        .expect("Books tag");
    let notes = tags
        .iter()
        .find(|(surface, _)| surface == "Notes")
        .expect("Notes tag");
    let settings = tags
        .iter()
        .find(|(surface, _)| surface == "SettingsSurface")
        .expect("Settings tag");
    assert_ne!(
        settings.1, books.1,
        "singleton and point update shapes are distinct ABI domains"
    );
    assert_ne!(
        books.1, notes.1,
        "store and resource catalog identity are part of update ABI"
    );
}

#[test]
fn stable_update_operation_descriptor_exports_catalog_bound_shapes() {
    let source = "\
module app
enum Status
    draft
    published
resource Author
    required name: string
store ^authors(id: int): Author
resource Book
    required title: string
    status: Status
    author: Id(^authors)
store ^books(shelf: string, id: int): Book
surface Books from ^books
    fields title, status, author
    update status, author
";
    let (_root, snapshot) = stable_snapshot("surface-update-abi-shapes", source);
    let program = &snapshot.program;
    let updates = snapshot.surface_update_operations().collect::<Vec<_>>();
    let [update] = updates.as_slice() else {
        panic!("expected one update operation, got {updates:#?}");
    };
    let descriptor = update
        .stable_descriptor()
        .expect("stable update descriptor");

    assert_eq!(descriptor.profile_version, "surface.update.v1");
    assert!(descriptor.operation_tag.starts_with("sha256:"));
    assert!(matches!(
        descriptor.kind,
        SurfaceUpdateOperationDescriptorKind::PointUpdate
    ));

    let facts = &program.facts;
    let module = facts.module_id("app").expect("module");
    let book = facts.resource_id(module, "Book").expect("Book");
    let store = facts.store_id(module, "books").expect("^books");
    let status = facts.resource_member_id(book, &["status"]).expect("status");
    let author = facts.resource_member_id(book, &["author"]).expect("author");
    let authors = facts.store_id(module, "authors").expect("^authors");
    let status_enum = facts.enum_id(module, "Status").expect("Status");

    assert_eq!(
        descriptor.store_catalog_id,
        catalog_id(facts.store(store).catalog_id.as_deref().expect("store id"))
    );
    assert_eq!(
        descriptor.resource_catalog_id,
        catalog_id(
            facts
                .resource(book)
                .catalog_id
                .as_deref()
                .expect("resource id")
        )
    );
    assert_eq!(
        descriptor
            .identity_keys
            .iter()
            .map(|key| &key.value)
            .collect::<Vec<_>>(),
        vec![
            &SurfaceOperationValueShape::Scalar(ScalarType::Str),
            &SurfaceOperationValueShape::Scalar(ScalarType::Int),
        ]
    );
    assert_eq!(
        descriptor
            .fields
            .iter()
            .map(|field| field.member_catalog_id.as_str())
            .collect::<Vec<_>>(),
        {
            let mut expected = vec![
                facts.resource_members()[status.0 as usize]
                    .catalog_id
                    .as_deref()
                    .expect("status id"),
                facts.resource_members()[author.0 as usize]
                    .catalog_id
                    .as_deref()
                    .expect("author id"),
            ];
            expected.sort();
            expected
        }
    );
    assert_eq!(
        descriptor
            .fields
            .iter()
            .map(|field| field.backing_required)
            .collect::<Vec<_>>(),
        vec![false, false]
    );
    let status_catalog_id = facts.resource_members()[status.0 as usize]
        .catalog_id
        .as_deref()
        .expect("status id");
    let author_catalog_id = facts.resource_members()[author.0 as usize]
        .catalog_id
        .as_deref()
        .expect("author id");
    let status_field = descriptor
        .fields
        .iter()
        .find(|field| field.member_catalog_id.as_str() == status_catalog_id)
        .expect("status update field");
    let author_field = descriptor
        .fields
        .iter()
        .find(|field| field.member_catalog_id.as_str() == author_catalog_id)
        .expect("author update field");
    assert!(matches!(
        &status_field.value,
        SurfaceOperationValueShape::Enum {
            enum_catalog_id,
            member_catalog_ids,
        } if enum_catalog_id.as_str() == facts.enum_(status_enum).unwrap().catalog_id.as_deref().unwrap()
            && member_catalog_ids.len() == 2
    ));
    assert!(matches!(
        &author_field.value,
        SurfaceOperationValueShape::Identity {
            store_catalog_id,
            key_scalars,
            ..
        } if store_catalog_id.as_str() == facts.store(authors).catalog_id.as_deref().unwrap()
            && key_scalars.as_slice() == [ScalarType::Int]
    ));
}

#[test]
fn source_only_surface_update_analysis_has_no_stable_descriptor() {
    let root = temp_project("surface-update-abi-source-only", |root| {
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
    update title
",
        );
    });
    let snapshot =
        analyze_project(&root, &config(), &ProjectSources::new(), None).expect("analyze");
    assert_clean(&snapshot.report);

    let updates = snapshot.surface_update_operations().collect::<Vec<_>>();
    let [update] = updates.as_slice() else {
        panic!("expected one update operation, got {updates:#?}");
    };
    assert!(update.stable_descriptor().is_none());
    assert_eq!(
        update.surface.catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![
            SurfaceCatalogBlocker::PendingCatalogProposal,
            SurfaceCatalogBlocker::MissingAcceptedCatalogIds,
        ])
    );
}

#[test]
fn stable_surface_without_update_fields_has_no_update_descriptor() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
surface Books from ^books
    fields title
";
    let (_root, snapshot) = stable_snapshot("surface-update-abi-no-update", source);

    assert!(snapshot.surface_update_operations().next().is_none());
}

#[test]
fn update_operation_tag_ignores_surface_label_and_update_field_order() {
    let source = "\
module app
resource Book
    title: string
    author: string
store ^books(id: int): Book
surface Books from ^books
    fields title, author
    update title, author
";
    let (root, snapshot) = stable_snapshot("surface-update-abi-labels", source);
    let original_descriptor = snapshot
        .surface_update_operations()
        .next()
        .and_then(|operation| operation.stable_descriptor())
        .expect("stable update descriptor");
    let original = original_descriptor.operation_tag.clone();
    let original_fields = original_descriptor
        .fields
        .iter()
        .map(|field| field.member_catalog_id.clone())
        .collect::<Vec<_>>();

    write(
        &root,
        "src/app.mw",
        "\
module app
resource Book
    title: string
    author: string
store ^books(id: int): Book
surface Library from ^books
    fields title, author
    update author, title
",
    );
    let accepted = marrow_catalog::CatalogMetadata::from_json(
        &std::fs::read_to_string(root.join("marrow.catalog.json")).expect("read catalog"),
    )
    .expect("catalog parses");
    let renamed = analyze_project(&root, &config(), &ProjectSources::new(), Some(&accepted))
        .expect("analyze renamed");
    assert_clean(&renamed.report);

    let renamed_descriptor = renamed
        .surface_update_operations()
        .next()
        .and_then(|operation| operation.stable_descriptor())
        .expect("stable renamed update descriptor");

    assert_eq!(renamed_descriptor.operation_tag, original);
    assert_eq!(
        renamed_descriptor
            .fields
            .iter()
            .map(|field| field.member_catalog_id.clone())
            .collect::<Vec<_>>(),
        original_fields,
        "update descriptor fields are canonicalized with the same identity as the tag"
    );
}
