use crate::support;
use support::*;

use marrow_check::{
    CheckedProgram, CheckedRuntimeProgram, ProjectConfig, SurfaceId, check_project,
};
use marrow_run::{
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_INVALID_DATA, SURFACE_REQUEST, SurfaceEnumValue,
    SurfaceReadError, SurfaceReadIdentity, SurfaceReadRecord, SurfaceValue, read_surface_point,
    read_surface_singleton,
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

fn assert_surface_error<T: std::fmt::Debug>(result: Result<T, SurfaceReadError>, code: &str) {
    match result {
        Err(error) => assert_eq!(error.code(), code, "{error:?}"),
        Ok(value) => panic!("expected surface error {code}, got {value:?}"),
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
