use crate::support::catalog::{catalog, entry, write_catalog};
use crate::support::{assert_clean, check_with_accepted, config, temp_project, write};

use marrow_catalog::{CatalogEntry, CatalogEntryKind};
use marrow_check::{
    AnalysisSnapshot, CheckedProgram, ENTRY_PROTOCOL_TAG_VERSION, EntryDescriptor,
    EntryFunctionSurfaceDescriptor, EntrySurfaceProfile, EntrySurfaceValueShape, ProjectSources,
    SurfaceActionOperationDescriptor, SurfaceCatalogBlocker, SurfaceCatalogStatus,
    SurfaceComputedReadOperationDescriptor, SurfaceCreateBodySemantics,
    SurfaceCreateExistenceSemantics, SurfaceCreateIdentityPolicy,
    SurfaceCreateOperationDescriptorKind, SurfaceDeleteOperationDescriptorKind,
    SurfaceDeleteSemantics, SurfaceOperationValueShape, SurfaceReadOperationDescriptor,
    SurfaceReadOperationDescriptorKind, SurfaceUpdateOperationDescriptor,
    SurfaceUpdateOperationDescriptorKind, WorkShapeClass, analyze_project, check_project,
};
use marrow_schema::ReturnPresence;
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&baseline),
        None,
    )
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

fn create_tag(snapshot: &AnalysisSnapshot) -> String {
    snapshot
        .surface_create_operations()
        .next()
        .and_then(|operation| operation.stable_descriptor())
        .map(|descriptor| descriptor.operation_tag)
        .expect("stable create operation tag")
}

fn delete_tag(snapshot: &AnalysisSnapshot) -> String {
    snapshot
        .surface_delete_operations()
        .next()
        .and_then(|operation| operation.stable_descriptor())
        .map(|descriptor| descriptor.operation_tag)
        .expect("stable delete operation tag")
}

fn computed_read_tag(snapshot: &AnalysisSnapshot) -> String {
    snapshot
        .surface_computed_read_operations()
        .next()
        .and_then(|operation| operation.stable_descriptor())
        .map(|descriptor| descriptor.operation_tag)
        .expect("stable computed-read operation tag")
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

/// Commit `metadata` into a store and read it back, so the surface ABI binds the identity
/// the store holds rather than a source-tree artifact. The store snapshot is the durable
/// authority for accepted identity, so a stable-descriptor assertion that flows through this
/// helper proves the tag came from store-resident ids, not from re-reading source on disk.
fn store_resident(metadata: &marrow_catalog::CatalogMetadata) -> marrow_catalog::CatalogMetadata {
    let store = marrow_store::tree::TreeStore::memory();
    store
        .replace_catalog_snapshot(metadata)
        .expect("commit accepted catalog");
    store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("store holds the committed accepted catalog")
}

fn function_ref(snapshot: &AnalysisSnapshot, name: &str) -> marrow_check::CheckedFunctionRef {
    let module = snapshot.program.facts.modules()[0].id;
    let function_id = snapshot
        .program
        .facts
        .function_id(module, name)
        .expect("function fact");
    let function = snapshot.program.facts.function(function_id);
    marrow_check::CheckedFunctionRef {
        module: function.module.0,
        function: function.source_index,
        presence: function.return_presence,
    }
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
fn surface_update_operation_tag_keeps_v1_bytes_stable_from_store_identity() {
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
    });

    let source_only =
        analyze_project(&root, &config(), &ProjectSources::new(), None, None).expect("analyze");
    assert_clean(&source_only.report);
    assert_eq!(
        source_only.program.facts.surfaces()[0].catalog_status,
        SurfaceCatalogStatus::SourceOnly(vec![
            SurfaceCatalogBlocker::PendingCatalogProposal,
            SurfaceCatalogBlocker::MissingAcceptedCatalogIds,
        ]),
        "without store-resident identity the surface carries no stable update ABI"
    );
    assert!(
        source_only
            .surface_update_operations()
            .next()
            .and_then(|operation| operation.stable_descriptor())
            .is_none()
    );

    let accepted = store_resident(&catalog(vec![
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
    ]));
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
    .expect("stable analysis");
    assert_clean(&snapshot.report);
    assert_eq!(
        snapshot.program.facts.surfaces()[0].catalog_status,
        SurfaceCatalogStatus::Stable,
        "store-resident identity makes the surface part of the stable ABI"
    );

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
        analyze_project(&root, &config(), &ProjectSources::new(), None, None).expect("analyze");
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
fn stable_computed_read_descriptor_wraps_callable_result_and_cost_shape() {
    let source = "\
module app
resource BookPage
    required title: string
resource Book
    title: string
store ^books(id: int): Book
pub fn bookPage(id: Id(^books)): maybe BookPage
    return BookPage(title: ^books(id)?.title ?? \"\")
surface Books from ^books
    fields title
    read bookPage as page
";
    let (_root, snapshot) = stable_snapshot("surface-computed-read-abi-shape", source);
    let descriptors = snapshot
        .surface_computed_read_operations()
        .map(|operation| {
            operation
                .stable_descriptor()
                .expect("stable computed-read descriptor")
        })
        .collect::<Vec<_>>();

    let [descriptor] = descriptors.as_slice() else {
        panic!("expected one computed-read descriptor, got {descriptors:#?}");
    };
    assert_eq!(descriptor.profile_version, "surface.computed_read.v1");
    assert_eq!(descriptor.alias, "page");
    assert!(descriptor.operation_tag.starts_with("sha256:"));
    assert_eq!(descriptor.callable.identity.requested_name, "app::bookPage");
    assert_eq!(descriptor.callable.identity.canonical_name, "app::bookPage");
    assert_eq!(descriptor.callable.parameters.len(), 1);
    assert_eq!(
        descriptor.callable.result.presence,
        ReturnPresence::MaybePresent
    );
    assert!(matches!(
        descriptor.callable.result.value,
        Some(EntrySurfaceValueShape::Resource { .. })
    ));
    assert_eq!(descriptor.cost_shape.work_shape, WorkShapeClass::ReadOnly);
    assert_eq!(descriptor.cost_shape.point_reads, 1);
    assert_eq!(descriptor.cost_shape.range_scans, 0);
    assert_eq!(descriptor.cost_shape.writes, 0);
    assert_eq!(descriptor.cost_shape.commit_points, 0);

    let surface = &snapshot.program.facts.surfaces()[0];
    let computed = &surface.computed_reads[0];
    let rebuilt = SurfaceComputedReadOperationDescriptor::from_computed_read(
        &snapshot.program,
        surface,
        computed,
    )
    .expect("descriptor from fact");
    assert_eq!(rebuilt.operation_tag, descriptor.operation_tag);
}

#[test]
fn stable_computed_read_operation_tag_changes_with_result_shape_and_cost_shape() {
    let string_source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn page(id: Id(^books)): string
    return ^books(id)?.title ?? \"\"
surface Books from ^books
    fields title
    read page
";
    let resource_source = "\
module app
resource BookPage
    required title: string
resource Book
    title: string
store ^books(id: int): Book
pub fn page(id: Id(^books)): BookPage
    return BookPage(title: ^books(id)?.title ?? \"\")
surface Books from ^books
    fields title
    read page
";
    let indexed_source = "\
module app
resource Book
    title: string
    shelf: string
store ^books(id: int): Book
    index byShelf(shelf, id)
pub fn page(shelf: string): int
    return count(^books.byShelf(shelf))
surface Books from ^books
    fields title
    read page
";
    let (_root, string_snapshot) =
        stable_snapshot("surface-computed-read-abi-tag-string", string_source);
    let (_root, resource_snapshot) =
        stable_snapshot("surface-computed-read-abi-tag-resource", resource_source);
    let (_root, indexed_snapshot) =
        stable_snapshot("surface-computed-read-abi-tag-indexed", indexed_source);

    assert_ne!(
        computed_read_tag(&string_snapshot),
        computed_read_tag(&resource_snapshot),
        "computed-read tags include the result descriptor"
    );
    assert_ne!(
        computed_read_tag(&string_snapshot),
        computed_read_tag(&indexed_snapshot),
        "computed-read tags include the cost-shape summary"
    );
}

#[test]
fn stable_computed_read_operation_tag_changes_with_read_only_implementation() {
    let first_source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn page(): string
    return \"first\"
surface Books from ^books
    fields title
    read page
";
    let second_source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn page(): string
    return \"second\"
surface Books from ^books
    fields title
    read page
";
    let (_root, first_snapshot) =
        stable_snapshot("surface-computed-read-tag-body-first", first_source);
    let (_root, second_snapshot) =
        stable_snapshot("surface-computed-read-tag-body-second", second_source);

    assert_ne!(
        computed_read_tag(&first_snapshot),
        computed_read_tag(&second_snapshot),
        "computed-read tags include the read-only implementation identity"
    );
}

#[test]
fn shared_function_surface_descriptor_preserves_maybe_presence() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn titleFor(id: Id(^books)): maybe string
    return absent
surface Books from ^books
    fields title
    action titleFor
";
    let (_root, snapshot) = stable_snapshot("surface-shared-abi-maybe-presence", source);
    let descriptor = EntryFunctionSurfaceDescriptor::from_function_ref(
        &snapshot.program,
        "app::titleFor",
        function_ref(&snapshot, "titleFor"),
        EntrySurfaceProfile::ComputedRead,
    )
    .expect("shared descriptor");

    assert_eq!(descriptor.result.presence, ReturnPresence::MaybePresent);
    assert!(matches!(
        descriptor.result.value,
        Some(EntrySurfaceValueShape::Scalar(ScalarType::Str))
    ));
}

#[test]
fn shared_function_surface_descriptor_renders_resource_results() {
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
";
    let (_root, snapshot) = stable_snapshot("surface-shared-abi-resource-result", source);
    let descriptor = EntryFunctionSurfaceDescriptor::from_function_ref(
        &snapshot.program,
        "app::bookPage",
        function_ref(&snapshot, "bookPage"),
        EntrySurfaceProfile::ComputedRead,
    )
    .expect("shared descriptor");

    let Some(EntrySurfaceValueShape::Resource {
        render_label,
        resource_catalog_id,
        fields,
    }) = descriptor.result.value
    else {
        panic!("expected resource result descriptor: {descriptor:#?}");
    };
    assert_eq!(render_label, "app::BookPage");
    assert!(resource_catalog_id.as_str().starts_with("cat_"));
    assert_eq!(fields.len(), 2);
    let title = fields
        .iter()
        .find(|field| field.render_label == "title")
        .expect("title field");
    assert!(title.required);
    assert!(matches!(
        title.shape,
        EntrySurfaceValueShape::Scalar(ScalarType::Str)
    ));
    let author = fields
        .iter()
        .find(|field| field.render_label == "author")
        .expect("author field");
    assert!(!author.required);
}

#[test]
fn computed_read_shared_descriptor_rejects_no_result_function() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn logBook()
    return
surface Books from ^books
    fields title
    action logBook
";
    let (_root, snapshot) = stable_snapshot("surface-shared-abi-no-result", source);

    assert!(
        EntryFunctionSurfaceDescriptor::from_function_ref(
            &snapshot.program,
            "app::logBook",
            function_ref(&snapshot, "logBook"),
            EntrySurfaceProfile::ComputedRead,
        )
        .is_none(),
        "computed-read descriptors require an explicit result value"
    );
}

#[test]
fn computed_read_shared_descriptor_rejects_unsupported_result_shape() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
pub fn lastError(): Error
    return Error(code: \"app.error\", message: \"hidden\")
surface Books from ^books
    fields title
";
    let (_root, snapshot) = stable_snapshot("surface-shared-abi-unsupported-result", source);

    assert!(
        EntryFunctionSurfaceDescriptor::from_function_ref(
            &snapshot.program,
            "app::lastError",
            function_ref(&snapshot, "lastError"),
            EntrySurfaceProfile::ComputedRead,
        )
        .is_none(),
        "computed-read descriptors must reject unsupported result shapes"
    );
}

#[test]
fn shared_function_surface_descriptor_rejects_private_function_refs() {
    let source = "\
module app
resource Book
    title: string
store ^books(id: int): Book
fn hidden(): maybe string
    return absent
surface Books from ^books
    fields title
";
    let (_root, snapshot) = stable_snapshot("surface-shared-abi-private-ref", source);

    assert!(
        EntryFunctionSurfaceDescriptor::from_function_ref(
            &snapshot.program,
            "app::hidden",
            function_ref(&snapshot, "hidden"),
            EntrySurfaceProfile::ComputedRead,
        )
        .is_none(),
        "raw checked function refs must not bypass public surface admission"
    );
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
        analyze_project(&root, &config(), &ProjectSources::new(), None, None).expect("analyze");
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&baseline),
        None,
    )
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&baseline),
        None,
    )
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
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&baseline),
        None,
    )
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
fn stable_create_and_delete_operation_descriptors_export_catalog_bound_shapes() {
    let source = "\
module app
resource Book
    required title: string
    required author: string
    isbn: string
store ^books(shelf: string, id: int): Book
surface Books from ^books
    fields title, author, isbn
    create title, author
    delete
";
    let (_root, snapshot) = stable_snapshot("surface-create-delete-abi-shapes", source);
    let program = &snapshot.program;
    let creates = snapshot.surface_create_operations().collect::<Vec<_>>();
    let deletes = snapshot.surface_delete_operations().collect::<Vec<_>>();
    let [create] = creates.as_slice() else {
        panic!("expected one create operation, got {creates:#?}");
    };
    let [delete] = deletes.as_slice() else {
        panic!("expected one delete operation, got {deletes:#?}");
    };
    let create = create
        .stable_descriptor()
        .expect("stable create descriptor");
    let delete = delete
        .stable_descriptor()
        .expect("stable delete descriptor");

    assert_eq!(create.profile_version, "surface.create.v1");
    assert_eq!(delete.profile_version, "surface.delete.v1");
    assert!(create.operation_tag.starts_with("sha256:"));
    assert!(delete.operation_tag.starts_with("sha256:"));
    assert_ne!(create.operation_tag, delete.operation_tag);
    assert!(matches!(
        create.kind,
        SurfaceCreateOperationDescriptorKind::PointCreate
    ));
    assert!(matches!(
        delete.kind,
        SurfaceDeleteOperationDescriptorKind::PointDelete
    ));
    assert_eq!(
        create.body_semantics,
        SurfaceCreateBodySemantics::ExactDeclaredBody
    );
    assert_eq!(
        create.identity_policy,
        SurfaceCreateIdentityPolicy::ClientSuppliedIdentity
    );
    assert_eq!(
        create.existence_semantics,
        SurfaceCreateExistenceSemantics::RejectExistingNoReplace
    );
    assert_eq!(
        delete.semantics,
        SurfaceDeleteSemantics::RejectAbsentFullSubtree
    );

    let facts = &program.facts;
    let module = facts.module_id("app").expect("module");
    let book = facts.resource_id(module, "Book").expect("Book");
    let store = facts.store_id(module, "books").expect("^books");
    let title = facts.resource_member_id(book, &["title"]).expect("title");
    let author = facts.resource_member_id(book, &["author"]).expect("author");
    let isbn = facts.resource_member_id(book, &["isbn"]).expect("isbn");

    assert_eq!(
        create.store_catalog_id,
        catalog_id(facts.store(store).catalog_id.as_deref().expect("store id"))
    );
    assert_eq!(delete.store_catalog_id, create.store_catalog_id);
    assert_eq!(
        create
            .fields
            .iter()
            .map(|field| field.member_catalog_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            facts.resource_members()[title.0 as usize]
                .catalog_id
                .as_deref()
                .expect("title id"),
            facts.resource_members()[author.0 as usize]
                .catalog_id
                .as_deref()
                .expect("author id"),
        ]
    );
    assert_eq!(
        create
            .projection
            .iter()
            .map(|field| field.member_catalog_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            facts.resource_members()[title.0 as usize]
                .catalog_id
                .as_deref()
                .expect("title id"),
            facts.resource_members()[author.0 as usize]
                .catalog_id
                .as_deref()
                .expect("author id"),
            facts.resource_members()[isbn.0 as usize]
                .catalog_id
                .as_deref()
                .expect("isbn id"),
        ]
    );
}

#[test]
fn create_and_delete_operation_tags_have_distinct_domains() {
    let source = "\
module app
resource Settings
    required mode: string
store ^settings: Settings
surface SettingsSurface from ^settings
    fields mode
    create mode
    update mode
    delete
";
    let (_root, snapshot) = stable_snapshot("surface-create-delete-abi-domain", source);
    let read = read_tags(&snapshot.program);
    let update = update_tag(&snapshot);
    let create = create_tag(&snapshot);
    let delete = delete_tag(&snapshot);

    assert_eq!(read.len(), 1);
    assert_ne!(read[0], create);
    assert_ne!(read[0], delete);
    assert_ne!(update, create);
    assert_ne!(update, delete);
    assert_ne!(create, delete);
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
        analyze_project(&root, &config(), &ProjectSources::new(), None, None).expect("analyze");
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
    let root = temp_project("surface-update-abi-labels", |root| {
        write(root, "src/app.mw", source)
    });
    let (baseline_report, baseline_program) =
        check_project(&root, &config()).expect("baseline check");
    assert_clean(&baseline_report);
    let accepted = store_resident(
        &baseline_program
            .catalog
            .proposal
            .expect("first run proposes catalog ids"),
    );
    let snapshot = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
    .expect("stable analysis");
    assert_clean(&snapshot.report);
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
    let renamed = analyze_project(
        &root,
        &config(),
        &ProjectSources::new(),
        Some(&accepted),
        None,
    )
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
