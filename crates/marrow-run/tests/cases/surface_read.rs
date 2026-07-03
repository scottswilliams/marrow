use crate::support;
use support::*;

use marrow_check::{
    CheckedProgram, CheckedRuntimeProgram, ProjectConfig, SurfaceId, SurfaceReadOperationKind,
    check_project,
};
use marrow_run::{
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_CURSOR, SURFACE_INVALID_DATA, SURFACE_LIMIT,
    SURFACE_REQUEST, SURFACE_STALE_CURSOR, SURFACE_STORE, SurfaceCollectionPageRequest,
    SurfaceCollectionRead, SurfaceCollectionReadShape, SurfaceEnumValue, SurfaceIndexRangeRequest,
    SurfaceNodeRead, SurfaceNodeReadShape, SurfacePageBoundary, SurfaceReadError,
    SurfaceReadIdentity, SurfaceReadOperationRef, SurfaceReadRecord, SurfaceUpdate,
    SurfaceUpdateField, SurfaceValue, read_surface_point, read_surface_singleton,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{
    DataPathSegment, StoreUid, TreeEnumMember, TreeStore, encode_tree_enum_member,
};
use marrow_store::value::SavedValue;

const BOOK_SURFACE: &str = "\
resource Book
    required title: string
    required privateCode: string
    subtitle: string
store ^books(id: int): Book

surface Books from ^books
    fields title, subtitle
";

const SETTINGS_SURFACE: &str = "\
resource Settings
    required theme: string
    mode: string
store ^settings: Settings

surface SettingsSurface from ^settings
    fields theme, mode
";

const ENROLLMENT_SURFACE: &str = "\
resource Enrollment
    required status: string
store ^enrollments(studentId: string, courseId: string): Enrollment

surface Enrollments from ^enrollments
    fields status
";

const GROUP_BACKED_SURFACE: &str = "\
resource Book
    required title: string
    audit
        required checksum: string
store ^books(id: int): Book

surface Books from ^books
    fields title
";

const OPTIONAL_PRIVATE_SURFACE: &str = "\
resource Book
    required title: string
    internalScore: int
store ^books(id: int): Book

surface Books from ^books
    fields title
";

const ENUM_AND_IDENTITY_SURFACE: &str = "\
enum Status
    draft
    published

resource Author
    required name: string
store ^authors(id: int): Author

resource Book
    required title: string
    required status: Status
    author: Id(^authors)
store ^books(id: int): Book

surface Books from ^books
    fields title, status, author
";

const NESTED_ENUM_SURFACE: &str = "\
enum Cat
    category tiger
        paw
    category lion
        paw

resource Sighting
    required spotted: Cat
store ^sightings(id: int): Sighting

surface Sightings from ^sightings
    fields spotted
";

const COLLECTION_SURFACE: &str = "\
resource Book
    required title: string
    required privateCode: string
    author: string
    isbn: string
store ^books(shelf: string, id: int): Book
    index byAuthor(author, shelf, id)
    index byIsbn(isbn) unique

surface Books from ^books
    fields title, author, isbn
    collection ^books as list
    collection ^books.byAuthor as byAuthor
    collection ^books.byIsbn as byIsbn
";

const COLLECTION_UPDATE_SURFACE: &str = "\
resource Book
    required title: string
    required privateCode: string
    author: string
    isbn: string
store ^books(shelf: string, id: int): Book
    index byAuthor(author, shelf, id)
    index byIsbn(isbn) unique

surface Books from ^books
    fields title, author, isbn
    update author
    collection ^books as list
";

const RANGE_COLLECTION_SURFACE: &str = "\
resource Post
    required title: string
    required category: string
    required publishedOn: date
store ^posts(id: int): Post
    index byCategoryDate(category, publishedOn, id)

surface Posts from ^posts
    fields title, category, publishedOn
    collection ^posts.byCategoryDate range as byCategoryDate
";

const DUPLICATE_NODE_TAG_SURFACES: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title

surface Library from ^books
    fields title
";

#[test]
fn point_read_materializes_required_non_public_fields_before_projection() {
    let (program, runtime) = committed_program_and_runtime(BOOK_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        SavedValue::Str("Dune".into()),
    );

    let surface = surface_id(&program, "Books");
    assert_surface_error(
        read_surface_point(&program, &store, surface, &identity),
        SURFACE_INVALID_DATA,
    );

    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["privateCode"]),
        SavedValue::Str("internal".into()),
    );

    let record = read_surface_point(&program, &store, surface, &identity).expect("surface read");
    assert_identity(&record, &store_catalog_id(&runtime, "books"), &identity);
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "books", &["title"]),
                Some(SurfaceValue::Str("Dune".into())),
            ),
            (field_catalog_id(&runtime, "books", &["subtitle"]), None),
        ]
    );
}

#[test]
fn surface_node_read_admits_checked_point_operation_tag() {
    let (program, runtime) = committed_program_and_runtime(BOOK_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        SavedValue::Str("Dune".into()),
    );
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["privateCode"]),
        SavedValue::Str("internal".into()),
    );

    let surface = surface_id(&program, "Books");
    let tag = operation_tag(&program, node_read_ref(&program, surface));
    let read = SurfaceNodeRead::admit_by_operation_tag(&program, &store, &tag)
        .expect("admit point read by operation tag");

    assert_eq!(read.surface(), surface);
    assert_eq!(read.shape(), SurfaceNodeReadShape::Point);
    let record = read.read_point(&identity).expect("surface point read");
    assert_identity(&record, &store_catalog_id(&runtime, "books"), &identity);
}

#[test]
fn surface_node_read_admits_checked_singleton_operation_tag() {
    let (program, runtime) = committed_program_and_runtime(SETTINGS_SURFACE);
    let store = admitted_store(&program);
    write_data_value(
        &runtime,
        &store,
        "settings",
        &[],
        &data_path(&runtime, "settings", &["theme"]),
        SavedValue::Str("dark".into()),
    );

    let surface = surface_id(&program, "SettingsSurface");
    let tag = operation_tag(&program, node_read_ref(&program, surface));
    let read = SurfaceNodeRead::admit_by_operation_tag(&program, &store, &tag)
        .expect("admit singleton read by operation tag");

    assert_eq!(read.surface(), surface);
    assert_eq!(read.shape(), SurfaceNodeReadShape::Singleton);
    let record = read.read_singleton().expect("surface singleton read");
    assert_eq!(record.identity, None);
}

#[test]
fn surface_node_read_tag_admission_fails_closed_for_collection_unknown_and_duplicate_tags() {
    let (program, _runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let collection_tag = operation_tag(&program, root_collection_ref(&program, surface));

    assert_surface_error(
        SurfaceNodeRead::admit_by_operation_tag(&program, &store, &collection_tag),
        SURFACE_ABI_MISMATCH,
    );
    assert_surface_error(
        SurfaceNodeRead::admit_by_operation_tag(&program, &store, "sha256:not-a-surface-tag"),
        SURFACE_ABI_MISMATCH,
    );

    let (duplicates, _runtime) = committed_program_and_runtime(DUPLICATE_NODE_TAG_SURFACES);
    let store = admitted_store(&duplicates);
    let tag = operation_tag(
        &duplicates,
        node_read_ref(&duplicates, surface_id(&duplicates, "Books")),
    );
    assert_surface_error(
        SurfaceNodeRead::admit_by_operation_tag(&duplicates, &store, &tag),
        SURFACE_ABI_MISMATCH,
    );
}

#[test]
fn surface_collection_read_admits_checked_operation_tags() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");

    let surface = surface_id(&program, "Books");
    let root_ref = root_collection_ref(&program, surface);
    let root_tag = operation_tag(&program, root_ref);
    let root = SurfaceCollectionRead::admit_by_operation_tag(&program, &store, &root_tag)
        .expect("admit root collection by operation tag");
    assert_eq!(root.operation_ref(), root_ref);
    assert_eq!(root.shape(), SurfaceCollectionReadShape::RootPage);

    let by_author_ref = index_collection_ref(&program, surface, "byAuthor");
    let by_author_tag = operation_tag(&program, by_author_ref);
    let by_author = SurfaceCollectionRead::admit_by_operation_tag(&program, &store, &by_author_tag)
        .expect("admit index collection by operation tag");
    assert_eq!(by_author.operation_ref(), by_author_ref);
    assert_eq!(by_author.shape(), SurfaceCollectionReadShape::IndexPage);
    assert_eq!(
        collect_page_identities(&by_author, &[SavedKey::Str("Frank".into())], 10),
        vec![vec![SavedKey::Str("a".into()), SavedKey::Int(1)]]
    );

    let by_isbn_ref = index_collection_ref(&program, surface, "byIsbn");
    let by_isbn_tag = operation_tag(&program, by_isbn_ref);
    let by_isbn = SurfaceCollectionRead::admit_by_operation_tag(&program, &store, &by_isbn_tag)
        .expect("admit unique lookup by operation tag");
    assert_eq!(by_isbn.operation_ref(), by_isbn_ref);
    assert_eq!(by_isbn.shape(), SurfaceCollectionReadShape::UniqueLookup);
    assert_eq!(
        by_isbn
            .lookup_unique(&[SavedKey::Str("isbn-a1".into())])
            .expect("unique lookup")
            .expect("record found")
            .identity
            .expect("identity")
            .keys,
        vec![SavedKey::Str("a".into()), SavedKey::Int(1)]
    );
}

#[test]
fn surface_collection_read_tag_admission_fails_closed_for_node_and_unknown_tags() {
    let (program, _runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let node_tag = operation_tag(&program, node_read_ref(&program, surface));

    assert_surface_error(
        SurfaceCollectionRead::admit_by_operation_tag(&program, &store, &node_tag),
        SURFACE_ABI_MISMATCH,
    );
    assert_surface_error(
        SurfaceCollectionRead::admit_by_operation_tag(&program, &store, "sha256:not-a-surface-tag"),
        SURFACE_ABI_MISMATCH,
    );
}

#[test]
fn surface_point_read_opens_a_read_snapshot_for_materialization() {
    let (program, runtime) = committed_program_and_runtime(BOOK_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        SavedValue::Str("Dune".into()),
    );
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["privateCode"]),
        SavedValue::Str("internal".into()),
    );
    let read =
        SurfaceNodeRead::admit(&program, &store, surface_id(&program, "Books")).expect("admit");

    let snapshot = store.read_snapshot().expect("pin read snapshot");
    assert_surface_error(read.read_point(&identity), SURFACE_STORE);
    drop(snapshot);

    read.read_point(&identity)
        .expect("point read succeeds after snapshot drops");
}

#[test]
fn surface_singleton_read_opens_a_read_snapshot_for_materialization() {
    let (program, runtime) = committed_program_and_runtime(SETTINGS_SURFACE);
    let store = admitted_store(&program);
    write_data_value(
        &runtime,
        &store,
        "settings",
        &[],
        &data_path(&runtime, "settings", &["theme"]),
        SavedValue::Str("dark".into()),
    );
    let read = SurfaceNodeRead::admit(&program, &store, surface_id(&program, "SettingsSurface"))
        .expect("admit");

    let snapshot = store.read_snapshot().expect("pin read snapshot");
    assert_surface_error(read.read_singleton(), SURFACE_STORE);
    drop(snapshot);

    read.read_singleton()
        .expect("singleton read succeeds after snapshot drops");
}

#[test]
fn point_read_enforces_stored_value_byte_budget() {
    let (program, runtime) = committed_program_and_runtime(BOOK_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        SavedValue::Str("x".repeat(marrow_run::SURFACE_MAX_VALUE_BYTES + 1)),
    );
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["privateCode"]),
        SavedValue::Str("internal".into()),
    );

    assert_surface_error(
        read_surface_point(&program, &store, surface_id(&program, "Books"), &identity),
        SURFACE_LIMIT,
    );
}

#[test]
fn singleton_read_returns_projection_without_identity() {
    let (program, runtime) = committed_program_and_runtime(SETTINGS_SURFACE);
    let store = admitted_store(&program);
    write_data_value(
        &runtime,
        &store,
        "settings",
        &[],
        &data_path(&runtime, "settings", &["theme"]),
        SavedValue::Str("dark".into()),
    );

    let record = read_surface_singleton(&program, &store, surface_id(&program, "SettingsSurface"))
        .expect("surface singleton read");
    assert_eq!(record.identity, None);
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "settings", &["theme"]),
                Some(SurfaceValue::Str("dark".into())),
            ),
            (field_catalog_id(&runtime, "settings", &["mode"]), None),
        ]
    );
}

#[test]
fn point_read_separates_absent_records_from_invalid_backing_data() {
    let (program, _runtime) = committed_program_and_runtime(BOOK_SURFACE);
    let store = admitted_store(&program);

    assert_surface_error(
        read_surface_point(
            &program,
            &store,
            surface_id(&program, "Books"),
            &[SavedKey::Int(404)],
        ),
        SURFACE_ABSENT,
    );
}

#[test]
fn point_read_rejects_identity_keys_that_do_not_match_the_checked_store() {
    let (program, _runtime) = committed_program_and_runtime(BOOK_SURFACE);
    let store = admitted_store(&program);

    assert_surface_error(
        read_surface_point(
            &program,
            &store,
            surface_id(&program, "Books"),
            &[SavedKey::Str("not-an-int".into())],
        ),
        SURFACE_REQUEST,
    );
}

#[test]
fn point_read_reports_projected_decode_failures_as_invalid_data() {
    let (program, runtime) = committed_program_and_runtime(BOOK_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    write_data_bytes(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        vec![0xff],
    );
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["privateCode"]),
        SavedValue::Str("internal".into()),
    );

    assert_surface_error(
        read_surface_point(&program, &store, surface_id(&program, "Books"), &identity),
        SURFACE_INVALID_DATA,
    );
}

#[test]
fn point_read_validates_present_private_optional_unkeyed_fields_before_projection() {
    let (program, runtime) = committed_program_and_runtime(OPTIONAL_PRIVATE_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        SavedValue::Str("Dune".into()),
    );
    write_data_bytes(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["internalScore"]),
        vec![0xff],
    );

    assert_surface_error(
        read_surface_point(&program, &store, surface_id(&program, "Books"), &identity),
        SURFACE_INVALID_DATA,
    );
}

#[test]
fn point_read_preserves_composite_identity_keys() {
    let (program, runtime) = committed_program_and_runtime(ENROLLMENT_SURFACE);
    let store = admitted_store(&program);
    let identity = [
        SavedKey::Str("student-1".into()),
        SavedKey::Str("course-9".into()),
    ];
    write_data_value(
        &runtime,
        &store,
        "enrollments",
        &identity,
        &data_path(&runtime, "enrollments", &["status"]),
        SavedValue::Str("active".into()),
    );

    let record = read_surface_point(
        &program,
        &store,
        surface_id(&program, "Enrollments"),
        &identity,
    )
    .expect("surface point read");
    assert_identity(
        &record,
        &store_catalog_id(&runtime, "enrollments"),
        &identity,
    );
    assert_eq!(
        field_values(&record),
        vec![(
            field_catalog_id(&runtime, "enrollments", &["status"]),
            Some(SurfaceValue::Str("active".into())),
        )]
    );
}

#[test]
fn point_read_validates_required_fields_inside_unkeyed_groups() {
    let (program, runtime) = committed_program_and_runtime(GROUP_BACKED_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        SavedValue::Str("Dune".into()),
    );

    assert_surface_error(
        read_surface_point(&program, &store, surface_id(&program, "Books"), &identity),
        SURFACE_INVALID_DATA,
    );

    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["audit", "checksum"]),
        SavedValue::Str("ok".into()),
    );

    let record = read_surface_point(&program, &store, surface_id(&program, "Books"), &identity)
        .expect("surface point read");
    assert_identity(&record, &store_catalog_id(&runtime, "books"), &identity);
    assert_eq!(
        field_values(&record),
        vec![(
            field_catalog_id(&runtime, "books", &["title"]),
            Some(SurfaceValue::Str("Dune".into())),
        )]
    );
}

#[test]
fn point_read_projects_enum_and_identity_fields_as_surface_values() {
    let (program, runtime) = committed_program_and_runtime(ENUM_AND_IDENTITY_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    let author_identity = [SavedKey::Int(42)];
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        SavedValue::Str("Dune".into()),
    );
    let status = TreeEnumMember::new(
        enum_catalog_id(&runtime, "Status"),
        enum_member_catalog_id(&runtime, "Status", "published"),
    );
    write_data_bytes(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["status"]),
        encode_tree_enum_member(&status).expect("surface test enum encodes"),
    );
    write_data_bytes(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["author"]),
        encode_identity_payload(&author_identity),
    );

    let record = read_surface_point(&program, &store, surface_id(&program, "Books"), &identity)
        .expect("surface point read");
    assert_identity(&record, &store_catalog_id(&runtime, "books"), &identity);
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "books", &["title"]),
                Some(SurfaceValue::Str("Dune".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["status"]),
                Some(SurfaceValue::Enum(SurfaceEnumValue {
                    enum_catalog_id: enum_catalog_id(&runtime, "Status"),
                    member_catalog_id: enum_member_catalog_id(&runtime, "Status", "published"),
                    render_label: "published".into(),
                })),
            ),
            (
                field_catalog_id(&runtime, "books", &["author"]),
                Some(SurfaceValue::Identity(SurfaceReadIdentity {
                    store_catalog_id: store_catalog_id(&runtime, "authors"),
                    keys: author_identity.to_vec(),
                })),
            ),
        ]
    );
}

#[test]
fn point_read_renders_nested_enum_labels_as_qualified_paths() {
    let (program, runtime) = committed_program_and_runtime(NESTED_ENUM_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Int(1)];
    let lion_paw = runtime
        .facts()
        .enum_members()
        .iter()
        .find(|member| {
            runtime
                .facts()
                .enum_member_render_path(member.id)
                .as_deref()
                == Some("lion::paw")
        })
        .expect("lion::paw member fact");
    let stored = TreeEnumMember::new(
        enum_catalog_id(&runtime, "Cat"),
        catalog_id(&lion_paw.catalog_id),
    );
    write_data_bytes(
        &runtime,
        &store,
        "sightings",
        &identity,
        &data_path(&runtime, "sightings", &["spotted"]),
        encode_tree_enum_member(&stored).expect("surface test enum encodes"),
    );

    // Duplicate leaves under different categories must render as their qualifying path, so the
    // envelope label stays injective and cannot show the wrong sibling member.
    let record = read_surface_point(
        &program,
        &store,
        surface_id(&program, "Sightings"),
        &identity,
    )
    .expect("surface point read");
    assert_eq!(
        field_values(&record),
        vec![(
            field_catalog_id(&runtime, "sightings", &["spotted"]),
            Some(SurfaceValue::Enum(SurfaceEnumValue {
                enum_catalog_id: enum_catalog_id(&runtime, "Cat"),
                member_catalog_id: catalog_id(&lion_paw.catalog_id),
                render_label: "lion::paw".into(),
            })),
        )]
    );
}

#[test]
fn surface_reads_require_an_admitted_store_catalog() {
    let program = source_only_program(BOOK_SURFACE);
    let store = TreeStore::memory();

    assert_surface_error(
        read_surface_point(
            &program,
            &store,
            surface_id(&program, "Books"),
            &[SavedKey::Int(1)],
        ),
        SURFACE_ABI_MISMATCH,
    );
}

#[test]
fn root_collection_pages_records_in_identity_order_with_typed_cursor() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "b", 1, "Hyperion", "Dan", "isbn-b1");
    write_book(&runtime, &store, "a", 2, "Dune Messiah", "Frank", "isbn-a2");
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");

    let surface = surface_id(&program, "Books");
    let read =
        SurfaceCollectionRead::admit(&program, &store, root_collection_ref(&program, surface))
            .expect("admit root collection");
    let first = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 2,
            cursor: None,
        })
        .expect("first page");

    assert_eq!(
        record_identities(&first.rows),
        vec![
            vec![SavedKey::Str("a".into()), SavedKey::Int(1)],
            vec![SavedKey::Str("a".into()), SavedKey::Int(2)],
        ]
    );
    let next = first.next.as_ref().expect("first page has cursor");

    let second = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 2,
            cursor: Some(next),
        })
        .expect("second page");
    assert_eq!(
        record_identities(&second.rows),
        vec![vec![SavedKey::Str("b".into()), SavedKey::Int(1)]]
    );
    assert_eq!(second.next, None);

    let all = vec![
        vec![SavedKey::Str("a".into()), SavedKey::Int(1)],
        vec![SavedKey::Str("a".into()), SavedKey::Int(2)],
        vec![SavedKey::Str("b".into()), SavedKey::Int(1)],
    ];
    for limit in 1..=3 {
        assert_eq!(collect_page_identities(&read, &[], limit), all);
    }
}

#[test]
fn collection_cursor_stales_after_committed_surface_update() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_UPDATE_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    write_book(&runtime, &store, "a", 2, "Dune Messiah", "Frank", "isbn-a2");
    write_book(&runtime, &store, "b", 1, "Hyperion", "Dan", "isbn-b1");

    let surface = surface_id(&program, "Books");
    let read =
        SurfaceCollectionRead::admit(&program, &store, root_collection_ref(&program, surface))
            .expect("admit root collection");
    let first = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 1,
            cursor: None,
        })
        .expect("first page");
    let cursor = first.next.as_ref().expect("first page has cursor");
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("baseline commit metadata");
    assert_eq!(cursor.commit_id, baseline.commit_id);

    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    update
        .update_point(
            &[SavedKey::Str("a".into()), SavedKey::Int(2)],
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Ursula".into()),
            }],
        )
        .expect("surface update succeeds");
    let changed = store
        .read_commit_metadata()
        .expect("read changed commit metadata")
        .expect("changed commit metadata");
    assert_eq!(changed.commit_id, baseline.commit_id + 1);

    assert_surface_error(
        read.page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 10,
            cursor: Some(cursor),
        }),
        SURFACE_STALE_CURSOR,
    );
}

#[test]
fn index_collection_pages_exact_tuple_in_identity_suffix_order() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "b", 1, "Book B", "Frank", "isbn-b1");
    write_book(&runtime, &store, "a", 2, "Book A2", "Frank", "isbn-a2");
    write_book(&runtime, &store, "a", 1, "Book A1", "Frank", "isbn-a1");
    write_book(&runtime, &store, "a", 3, "Other", "Octavia", "isbn-a3");

    let surface = surface_id(&program, "Books");
    let read = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byAuthor"),
    )
    .expect("admit index collection");
    let first = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("Frank".into())],
            range: None,
            limit: 2,
            cursor: None,
        })
        .expect("first index page");

    assert_eq!(
        record_identities(&first.rows),
        vec![
            vec![SavedKey::Str("a".into()), SavedKey::Int(1)],
            vec![SavedKey::Str("a".into()), SavedKey::Int(2)],
        ]
    );

    let second = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("Frank".into())],
            range: None,
            limit: 2,
            cursor: first.next.as_ref(),
        })
        .expect("second index page");
    assert_eq!(
        record_identities(&second.rows),
        vec![vec![SavedKey::Str("b".into()), SavedKey::Int(1)]]
    );
    assert_eq!(second.next, None);
}

#[test]
fn index_range_collection_pages_bounds_and_cursor() {
    let (program, runtime) = committed_program_and_runtime(RANGE_COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_post(&runtime, &store, 1, "Old Fantasy", "fantasy", 10);
    write_post(&runtime, &store, 2, "First Match", "fantasy", 20);
    write_post(&runtime, &store, 3, "Second Match", "fantasy", 20);
    write_post(&runtime, &store, 4, "Too New", "fantasy", 30);
    write_post(&runtime, &store, 5, "Other Category", "news", 20);

    let surface = surface_id(&program, "Posts");
    let read = SurfaceCollectionRead::admit(
        &program,
        &store,
        range_collection_ref(&program, surface, "byCategoryDate"),
    )
    .expect("admit range collection");
    assert_eq!(read.shape(), SurfaceCollectionReadShape::IndexRangePage);

    let range = SurfaceIndexRangeRequest {
        lower: Some(SavedKey::Date(10)),
        lower_inclusive: false,
        upper: Some(SavedKey::Date(20)),
        upper_inclusive: true,
    };
    let first = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("fantasy".into())],
            range: Some(&range),
            limit: 1,
            cursor: None,
        })
        .expect("first range page");

    assert_eq!(record_identities(&first.rows), vec![vec![SavedKey::Int(2)]]);
    let cursor = first.next.as_ref().expect("first range page cursor");

    let second = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("fantasy".into())],
            range: Some(&range),
            limit: 10,
            cursor: Some(cursor),
        })
        .expect("second range page");
    assert_eq!(
        record_identities(&second.rows),
        vec![vec![SavedKey::Int(3)]]
    );
    assert_eq!(second.next, None);
}

#[test]
fn index_range_collection_single_sided_ranges_page_with_canonical_cursor_bounds() {
    let (program, runtime) = committed_program_and_runtime(RANGE_COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_post(&runtime, &store, 1, "Old Fantasy", "fantasy", 10);
    write_post(&runtime, &store, 2, "First Match", "fantasy", 20);
    write_post(&runtime, &store, 3, "Second Match", "fantasy", 30);

    let surface = surface_id(&program, "Posts");
    let read = SurfaceCollectionRead::admit(
        &program,
        &store,
        range_collection_ref(&program, surface, "byCategoryDate"),
    )
    .expect("admit range collection");

    let lower_only = SurfaceIndexRangeRequest {
        lower: Some(SavedKey::Date(10)),
        lower_inclusive: false,
        upper: None,
        upper_inclusive: true,
    };
    let first_lower = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("fantasy".into())],
            range: Some(&lower_only),
            limit: 1,
            cursor: None,
        })
        .expect("first lower-only range page");
    assert_eq!(
        record_identities(&first_lower.rows),
        vec![vec![SavedKey::Int(2)]]
    );
    let canonical_lower_only = SurfaceIndexRangeRequest {
        upper_inclusive: false,
        ..lower_only
    };
    let second_lower = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("fantasy".into())],
            range: Some(&canonical_lower_only),
            limit: 10,
            cursor: first_lower.next.as_ref(),
        })
        .expect("second lower-only range page");
    assert_eq!(
        record_identities(&second_lower.rows),
        vec![vec![SavedKey::Int(3)]]
    );

    let upper_only = SurfaceIndexRangeRequest {
        lower: None,
        lower_inclusive: true,
        upper: Some(SavedKey::Date(30)),
        upper_inclusive: false,
    };
    let first_upper = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("fantasy".into())],
            range: Some(&upper_only),
            limit: 1,
            cursor: None,
        })
        .expect("first upper-only range page");
    assert_eq!(
        record_identities(&first_upper.rows),
        vec![vec![SavedKey::Int(1)]]
    );
    let canonical_upper_only = SurfaceIndexRangeRequest {
        lower_inclusive: false,
        ..upper_only
    };
    let second_upper = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("fantasy".into())],
            range: Some(&canonical_upper_only),
            limit: 10,
            cursor: first_upper.next.as_ref(),
        })
        .expect("second upper-only range page");
    assert_eq!(
        record_identities(&second_upper.rows),
        vec![vec![SavedKey::Int(2)]]
    );
}

#[test]
fn index_range_collection_cursor_is_bound_to_exact_keys_and_range() {
    let (program, runtime) = committed_program_and_runtime(RANGE_COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_post(&runtime, &store, 1, "Old Fantasy", "fantasy", 10);
    write_post(&runtime, &store, 2, "First Match", "fantasy", 20);
    write_post(&runtime, &store, 3, "Other Category", "news", 20);
    write_post(&runtime, &store, 4, "Second Match", "fantasy", 20);

    let surface = surface_id(&program, "Posts");
    let read = SurfaceCollectionRead::admit(
        &program,
        &store,
        range_collection_ref(&program, surface, "byCategoryDate"),
    )
    .expect("admit range collection");
    let range = SurfaceIndexRangeRequest {
        lower: Some(SavedKey::Date(10)),
        lower_inclusive: false,
        upper: Some(SavedKey::Date(20)),
        upper_inclusive: true,
    };
    let page = read
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("fantasy".into())],
            range: Some(&range),
            limit: 1,
            cursor: None,
        })
        .expect("range page");
    let cursor = page.next.as_ref().expect("range cursor");

    let different_range = SurfaceIndexRangeRequest {
        lower: Some(SavedKey::Date(10)),
        lower_inclusive: true,
        upper: Some(SavedKey::Date(20)),
        upper_inclusive: true,
    };
    assert_surface_error(
        read.page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("fantasy".into())],
            range: Some(&different_range),
            limit: 10,
            cursor: Some(cursor),
        }),
        SURFACE_CURSOR,
    );
    assert_surface_error(
        read.page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("news".into())],
            range: Some(&range),
            limit: 10,
            cursor: Some(cursor),
        }),
        SURFACE_CURSOR,
    );
}

#[test]
fn unique_index_lookup_returns_zero_or_one_materialized_record() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    write_book(&runtime, &store, "a", 2, "Dune Messiah", "Frank", "isbn-a2");

    let surface = surface_id(&program, "Books");
    let read = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byIsbn"),
    )
    .expect("admit unique lookup");

    let found = read
        .lookup_unique(&[SavedKey::Str("isbn-a1".into())])
        .expect("unique lookup")
        .expect("record found");
    assert_eq!(
        found.identity.expect("identity").keys,
        vec![SavedKey::Str("a".into()), SavedKey::Int(1)]
    );
    assert_eq!(
        read.lookup_unique(&[SavedKey::Str("missing".into())])
            .expect("absent lookup"),
        None
    );
}

#[test]
fn unique_index_lookup_fails_closed_on_duplicate_or_dangling_entries() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    write_unique_index_entry(
        &runtime,
        &store,
        "byIsbn",
        &[SavedKey::Str("isbn-a1".into())],
        &[SavedKey::Str("a".into()), SavedKey::Int(2)],
    );

    let surface = surface_id(&program, "Books");
    let read = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byIsbn"),
    )
    .expect("admit unique lookup");

    assert_surface_error(
        read.lookup_unique(&[SavedKey::Str("isbn-a1".into())]),
        SURFACE_INVALID_DATA,
    );

    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_unique_index_entry(
        &runtime,
        &store,
        "byIsbn",
        &[SavedKey::Str("dangling".into())],
        &[SavedKey::Str("z".into()), SavedKey::Int(9)],
    );
    let surface = surface_id(&program, "Books");
    let read = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byIsbn"),
    )
    .expect("admit unique lookup");

    assert_surface_error(
        read.lookup_unique(&[SavedKey::Str("dangling".into())]),
        SURFACE_INVALID_DATA,
    );
}

#[test]
fn collection_page_fails_instead_of_skipping_a_corrupt_row() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    write_record_presence(
        &runtime,
        &store,
        "books",
        &[SavedKey::Str("a".into()), SavedKey::Int(2)],
    );
    write_non_unique_index_entry(
        &runtime,
        &store,
        "byAuthor",
        &[
            SavedKey::Str("Frank".into()),
            SavedKey::Str("a".into()),
            SavedKey::Int(2),
        ],
        &[SavedKey::Str("a".into()), SavedKey::Int(2)],
    );

    let surface = surface_id(&program, "Books");
    let read = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byAuthor"),
    )
    .expect("admit index collection");

    assert_surface_error(
        read.page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("Frank".into())],
            range: None,
            limit: 10,
            cursor: None,
        }),
        SURFACE_INVALID_DATA,
    );
}

#[test]
fn collection_page_enforces_total_materialization_byte_budget() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    let large_title = "x".repeat(marrow_run::SURFACE_MAX_MATERIALIZED_BYTES / 9 + 1);
    for id in 0..9 {
        write_book(
            &runtime,
            &store,
            "a",
            id,
            &large_title,
            "Frank",
            &format!("isbn-a{id}"),
        );
    }

    let surface = surface_id(&program, "Books");
    let read =
        SurfaceCollectionRead::admit(&program, &store, root_collection_ref(&program, surface))
            .expect("admit root collection");

    assert_surface_error(
        read.page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 9,
            cursor: None,
        }),
        SURFACE_LIMIT,
    );
}

#[test]
fn collection_requests_validate_limits_exact_keys_and_cursor_binding() {
    let (program, runtime) = committed_program_and_runtime(COLLECTION_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    write_book(&runtime, &store, "a", 2, "Dune Messiah", "Frank", "isbn-a2");

    let surface = surface_id(&program, "Books");
    let root =
        SurfaceCollectionRead::admit(&program, &store, root_collection_ref(&program, surface))
            .expect("admit root collection");
    let by_author = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byAuthor"),
    )
    .expect("admit index collection");

    assert_surface_error(
        root.page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 0,
            cursor: None,
        }),
        SURFACE_REQUEST,
    );
    assert_surface_error(
        root.page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: marrow_run::SURFACE_MAX_PAGE_LIMIT + 1,
            cursor: None,
        }),
        SURFACE_LIMIT,
    );
    assert_surface_error(
        by_author.page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Int(1)],
            range: None,
            limit: 1,
            cursor: None,
        }),
        SURFACE_REQUEST,
    );

    let root_page = root
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 1,
            cursor: None,
        })
        .expect("root page");
    let cursor = root_page.next.as_ref().expect("root cursor");
    assert_surface_error(
        by_author.page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("Frank".into())],
            range: None,
            limit: 1,
            cursor: Some(cursor),
        }),
        SURFACE_STALE_CURSOR,
    );

    let index_page = by_author
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("Frank".into())],
            range: None,
            limit: 1,
            cursor: None,
        })
        .expect("index page");
    let cursor = index_page.next.as_ref().expect("index cursor");
    assert_surface_error(
        by_author.page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("Octavia".into())],
            range: None,
            limit: 1,
            cursor: Some(cursor),
        }),
        SURFACE_CURSOR,
    );

    let mut forged = cursor.clone();
    match &mut forged.boundary {
        SurfacePageBoundary::IndexIdentity { exact_keys, .. } => {
            exact_keys[0] = SavedKey::Str("Octavia".into());
        }
        boundary => panic!("expected index cursor boundary, got {boundary:?}"),
    }
    assert_surface_error(
        by_author.page(SurfaceCollectionPageRequest {
            exact_keys: &[SavedKey::Str("Frank".into())],
            range: None,
            limit: 1,
            cursor: Some(&forged),
        }),
        SURFACE_CURSOR,
    );
}

fn surface_id(program: &CheckedProgram, name: &str) -> SurfaceId {
    program
        .facts
        .surfaces()
        .iter()
        .find(|surface| surface.name == name)
        .unwrap_or_else(|| panic!("surface `{name}` is present in checked facts"))
        .id
}

fn admitted_store(program: &CheckedProgram) -> TreeStore {
    let store = TreeStore::memory();
    marrow_run::evolution::commit_catalog_baseline(&store, program)
        .expect("commit surface test catalog baseline");
    store
        .write_store_uid(&StoreUid::from_entropy_bytes([7; 16]))
        .expect("write surface test store uid");
    store
}

fn root_collection_ref(program: &CheckedProgram, surface: SurfaceId) -> SurfaceReadOperationRef {
    operation_ref(program, surface, |kind| {
        matches!(kind, SurfaceReadOperationKind::PagedRootCollection { .. })
    })
}

fn node_read_ref(program: &CheckedProgram, surface: SurfaceId) -> SurfaceReadOperationRef {
    operation_ref(program, surface, |kind| {
        matches!(
            kind,
            SurfaceReadOperationKind::SingletonRead { .. }
                | SurfaceReadOperationKind::PointRead { .. }
        )
    })
}

fn index_collection_ref(
    program: &CheckedProgram,
    surface: SurfaceId,
    index_name: &str,
) -> SurfaceReadOperationRef {
    operation_ref(program, surface, |kind| match *kind {
        SurfaceReadOperationKind::PagedIndexCollection { index, .. }
        | SurfaceReadOperationKind::UniqueIndexLookup { index, .. } => {
            program.facts.store_index(index).name == index_name
        }
        _ => false,
    })
}

fn range_collection_ref(
    program: &CheckedProgram,
    surface: SurfaceId,
    index_name: &str,
) -> SurfaceReadOperationRef {
    operation_ref(program, surface, |kind| match *kind {
        SurfaceReadOperationKind::PagedIndexRangeCollection { index, .. } => {
            program.facts.store_index(index).name == index_name
        }
        _ => false,
    })
}

fn operation_ref(
    program: &CheckedProgram,
    surface: SurfaceId,
    matches_kind: impl Fn(&SurfaceReadOperationKind) -> bool,
) -> SurfaceReadOperationRef {
    let surface_fact = program.facts.surface(surface);
    let ordinal = surface_fact
        .read_operations
        .iter()
        .position(|operation| matches_kind(&operation.kind))
        .expect("surface operation is present");
    SurfaceReadOperationRef { surface, ordinal }
}

fn operation_tag(program: &CheckedProgram, operation_ref: SurfaceReadOperationRef) -> String {
    program.facts.surface(operation_ref.surface).read_operations[operation_ref.ordinal]
        .operation_tag
        .clone()
        .expect("stable surface operation tag")
}

fn write_book(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    shelf: &str,
    id: i64,
    title: &str,
    author: &str,
    isbn: &str,
) {
    let identity = [SavedKey::Str(shelf.into()), SavedKey::Int(id)];
    write_data_value(
        program,
        store,
        "books",
        &identity,
        &data_path(program, "books", &["title"]),
        SavedValue::Str(title.into()),
    );
    write_data_value(
        program,
        store,
        "books",
        &identity,
        &data_path(program, "books", &["privateCode"]),
        SavedValue::Str("internal".into()),
    );
    write_data_value(
        program,
        store,
        "books",
        &identity,
        &data_path(program, "books", &["author"]),
        SavedValue::Str(author.into()),
    );
    write_data_value(
        program,
        store,
        "books",
        &identity,
        &data_path(program, "books", &["isbn"]),
        SavedValue::Str(isbn.into()),
    );
    write_non_unique_index_entry(
        program,
        store,
        "byAuthor",
        &[
            SavedKey::Str(author.into()),
            SavedKey::Str(shelf.into()),
            SavedKey::Int(id),
        ],
        &identity,
    );
    write_unique_index_entry(
        program,
        store,
        "byIsbn",
        &[SavedKey::Str(isbn.into())],
        &identity,
    );
}

fn write_post(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    id: i64,
    title: &str,
    category: &str,
    published_on: i32,
) {
    let identity = [SavedKey::Int(id)];
    write_data_value(
        program,
        store,
        "posts",
        &identity,
        &data_path(program, "posts", &["title"]),
        SavedValue::Str(title.into()),
    );
    write_data_value(
        program,
        store,
        "posts",
        &identity,
        &data_path(program, "posts", &["category"]),
        SavedValue::Str(category.into()),
    );
    write_data_value(
        program,
        store,
        "posts",
        &identity,
        &data_path(program, "posts", &["publishedOn"]),
        SavedValue::Date(published_on),
    );
    store
        .write_index_entry(
            &index_catalog_id(program, "posts", "byCategoryDate"),
            &[
                SavedKey::Str(category.into()),
                SavedKey::Date(published_on),
                SavedKey::Int(id),
            ],
            &identity,
            Vec::new(),
        )
        .expect("range index entry write succeeds");
}

fn write_non_unique_index_entry(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    index: &str,
    index_keys: &[SavedKey],
    identity: &[SavedKey],
) {
    store
        .write_index_entry(
            &index_catalog_id(program, "books", index),
            index_keys,
            identity,
            Vec::new(),
        )
        .expect("non-unique index entry write succeeds");
}

fn write_unique_index_entry(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    index: &str,
    index_keys: &[SavedKey],
    identity: &[SavedKey],
) {
    store
        .write_index_entry(
            &index_catalog_id(program, "books", index),
            index_keys,
            identity,
            encode_identity_payload(identity),
        )
        .expect("unique index entry write succeeds");
}

fn record_identities(records: &[SurfaceReadRecord]) -> Vec<Vec<SavedKey>> {
    records
        .iter()
        .map(|record| record.identity.as_ref().expect("row identity").keys.clone())
        .collect()
}

fn collect_page_identities(
    read: &SurfaceCollectionRead<'_>,
    exact_keys: &[SavedKey],
    limit: usize,
) -> Vec<Vec<SavedKey>> {
    let mut cursor = None;
    let mut identities = Vec::new();
    loop {
        let page = read
            .page(SurfaceCollectionPageRequest {
                exact_keys,
                range: None,
                limit,
                cursor: cursor.as_ref(),
            })
            .expect("surface page");
        identities.extend(record_identities(&page.rows));
        let Some(next) = page.next else {
            return identities;
        };
        cursor = Some(next);
    }
}

fn source_only_program(source: &str) -> CheckedProgram {
    let root = TempDir::new("marrow-surface-read-test").expect("create surface test project");
    let (path, text) = checked_source_file(source, &[]);
    write_temp_source(root.path(), &path, &text);
    let config = ProjectConfig {
        tests: Vec::new(),
        ..test_project_config()
    };
    let (report, program) =
        check_project(root.path(), &config).expect("check surface test project");
    assert!(
        !report.has_errors(),
        "surface test program must check cleanly: {:#?}",
        report.diagnostics
    );
    program
}

fn assert_surface_error<T>(result: Result<T, SurfaceReadError>, code: &str) {
    match result {
        Err(error) => assert_eq!(error.code(), code, "{error:?}"),
        Ok(_) => panic!("expected surface error {code}"),
    }
}

fn assert_identity(record: &SurfaceReadRecord, store_catalog_id: &CatalogId, keys: &[SavedKey]) {
    let Some(identity) = &record.identity else {
        panic!("expected surface read identity: {record:?}");
    };
    assert_eq!(&identity.store_catalog_id, store_catalog_id);
    assert_eq!(identity.keys.as_slice(), keys);
}

fn field_values(record: &SurfaceReadRecord) -> Vec<(CatalogId, Option<SurfaceValue>)> {
    record
        .fields
        .iter()
        .map(|field| (field.catalog_id.clone(), field.value.clone()))
        .collect()
}

fn field_catalog_id(program: &CheckedRuntimeProgram, root: &str, members: &[&str]) -> CatalogId {
    let path = data_path(program, root, members);
    let Some(DataPathSegment::Member(catalog_id)) = path.last() else {
        panic!("expected `{root}` field path for {members:?}");
    };
    catalog_id.clone()
}
