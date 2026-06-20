use crate::support;
use support::*;

use marrow_check::{
    CheckedProgram, CheckedRuntimeProgram, SurfaceCreateOperationDescriptor,
    SurfaceDeleteOperationDescriptor, SurfaceId, SurfaceReadOperationKind,
};
use marrow_run::{
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_CONFLICT, SURFACE_REQUEST,
    SurfaceCollectionPageRequest, SurfaceCollectionRead, SurfaceCreate, SurfaceCreateField,
    SurfaceDelete, SurfaceError, SurfaceReadRecord, SurfaceValue, read_surface_point,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, StoreUid, TreeStore};
use marrow_store::value::SavedValue;

const CREATE_DELETE_SURFACE: &str = "\
resource Book
    required title: string
    required author: string
    isbn: string
    secret: string
store ^books(id: int): Book
    index byAuthor(author, id)
    index byIsbn(isbn) unique

surface Books from ^books
    fields title, author, isbn
    create title, author, isbn
    delete
    collection ^books.byAuthor as byAuthor
    collection ^books.byIsbn as byIsbn
";

const SINGLETON_CREATE_DELETE_SURFACE: &str = "\
resource Settings
    required theme: string
    mode: string
store ^settings: Settings

surface SettingsSurface from ^settings
    fields theme, mode
    create theme
    delete
";

const DUPLICATE_CREATE_DELETE_TAG_SURFACES: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    create title
    delete

surface Library from ^books
    fields title
    create title
    delete
";

#[test]
fn point_create_writes_declared_fields_indexes_and_returns_projection() {
    let (program, runtime) = committed_program_and_runtime(CREATE_DELETE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let create = SurfaceCreate::admit(&program, &store, surface).expect("admit create");

    let record = create
        .create_point(
            &[SavedKey::Int(1)],
            &[
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["title"]),
                    value: SurfaceValue::Str("Dune".into()),
                },
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                    value: SurfaceValue::Str("Frank".into()),
                },
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["isbn"]),
                    value: SurfaceValue::Str("isbn-1".into()),
                },
            ],
        )
        .expect("surface create succeeds");

    assert_eq!(
        record
            .identity
            .as_ref()
            .expect("created record identity")
            .keys,
        vec![SavedKey::Int(1)]
    );
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "books", &["title"]),
                Some(SurfaceValue::Str("Dune".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["author"]),
                Some(SurfaceValue::Str("Frank".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["isbn"]),
                Some(SurfaceValue::Str("isbn-1".into())),
            ),
        ]
    );

    let by_author = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byAuthor"),
    )
    .expect("admit author collection");
    assert_eq!(
        collect_page_identities(&by_author, &[SavedKey::Str("Frank".into())], 10),
        vec![vec![SavedKey::Int(1)]]
    );
}

#[test]
fn singleton_create_targets_the_keyless_record() {
    let (program, runtime) = committed_program_and_runtime(SINGLETON_CREATE_DELETE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "SettingsSurface");
    let create = SurfaceCreate::admit(&program, &store, surface).expect("admit create");

    let record = create
        .create_singleton(&[SurfaceCreateField {
            catalog_id: field_catalog_id(&runtime, "settings", &["theme"]),
            value: SurfaceValue::Str("dark".into()),
        }])
        .expect("singleton create succeeds");

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
fn create_rejects_existing_record_and_non_exact_body() {
    let (program, runtime) = committed_program_and_runtime(CREATE_DELETE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let create = SurfaceCreate::admit(&program, &store, surface).expect("admit create");
    let title = field_catalog_id(&runtime, "books", &["title"]);
    let author = field_catalog_id(&runtime, "books", &["author"]);
    let isbn = field_catalog_id(&runtime, "books", &["isbn"]);
    let secret = field_catalog_id(&runtime, "books", &["secret"]);

    assert_surface_error(
        create.create_point(
            &[SavedKey::Int(1)],
            &[
                SurfaceCreateField {
                    catalog_id: title.clone(),
                    value: SurfaceValue::Str("Dune".into()),
                },
                SurfaceCreateField {
                    catalog_id: author.clone(),
                    value: SurfaceValue::Str("Frank".into()),
                },
            ],
        ),
        SURFACE_REQUEST,
    );
    assert_surface_error(
        create.create_point(
            &[SavedKey::Int(1)],
            &[
                SurfaceCreateField {
                    catalog_id: title.clone(),
                    value: SurfaceValue::Str("Dune".into()),
                },
                SurfaceCreateField {
                    catalog_id: title.clone(),
                    value: SurfaceValue::Str("Dune Again".into()),
                },
                SurfaceCreateField {
                    catalog_id: author.clone(),
                    value: SurfaceValue::Str("Frank".into()),
                },
                SurfaceCreateField {
                    catalog_id: isbn.clone(),
                    value: SurfaceValue::Str("isbn-1".into()),
                },
            ],
        ),
        SURFACE_REQUEST,
    );
    assert_surface_error(
        create.create_point(
            &[SavedKey::Int(1)],
            &[
                SurfaceCreateField {
                    catalog_id: title.clone(),
                    value: SurfaceValue::Str("Dune".into()),
                },
                SurfaceCreateField {
                    catalog_id: author.clone(),
                    value: SurfaceValue::Str("Frank".into()),
                },
                SurfaceCreateField {
                    catalog_id: isbn.clone(),
                    value: SurfaceValue::Str("isbn-1".into()),
                },
                SurfaceCreateField {
                    catalog_id: secret,
                    value: SurfaceValue::Str("private".into()),
                },
            ],
        ),
        SURFACE_REQUEST,
    );

    create
        .create_point(
            &[SavedKey::Int(1)],
            &[
                SurfaceCreateField {
                    catalog_id: title,
                    value: SurfaceValue::Str("Dune".into()),
                },
                SurfaceCreateField {
                    catalog_id: author,
                    value: SurfaceValue::Str("Frank".into()),
                },
                SurfaceCreateField {
                    catalog_id: isbn,
                    value: SurfaceValue::Str("isbn-1".into()),
                },
            ],
        )
        .expect("initial create succeeds");
    assert_surface_error(
        create.create_point(&[SavedKey::Int(1)], &[]),
        SURFACE_CONFLICT,
    );
}

#[test]
fn delete_removes_record_subtree_indexes_and_allows_recreate() {
    let (program, runtime) = committed_program_and_runtime(CREATE_DELETE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let identity = [SavedKey::Int(1)];
    create_book(
        &program, &runtime, &store, &identity, "Dune", "Frank", "isbn-1",
    );
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["secret"]),
        SavedValue::Str("private".into()),
    );

    let delete = SurfaceDelete::admit(&program, &store, surface).expect("admit delete");
    delete
        .delete_point(&identity)
        .expect("surface delete succeeds");
    assert_surface_error(
        read_surface_point(&program, &store, surface, &identity),
        SURFACE_ABSENT,
    );
    assert_eq!(
        read_data_value(
            &runtime,
            &store,
            "books",
            &identity,
            &data_path(&runtime, "books", &["secret"]),
            marrow_store::value::ScalarType::Str,
        ),
        None
    );

    let by_author = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byAuthor"),
    )
    .expect("admit author collection");
    assert_eq!(
        collect_page_identities(&by_author, &[SavedKey::Str("Frank".into())], 10),
        Vec::<Vec<SavedKey>>::new()
    );
    let by_isbn = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byIsbn"),
    )
    .expect("admit isbn lookup");
    assert_eq!(
        by_isbn
            .lookup_unique(&[SavedKey::Str("isbn-1".into())])
            .expect("lookup deleted isbn"),
        None
    );

    create_book(
        &program, &runtime, &store, &identity, "Dune 2", "Frank", "isbn-2",
    );
    let record = read_surface_point(&program, &store, surface, &identity)
        .expect("recreated record is readable");
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "books", &["title"]),
                Some(SurfaceValue::Str("Dune 2".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["author"]),
                Some(SurfaceValue::Str("Frank".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["isbn"]),
                Some(SurfaceValue::Str("isbn-2".into())),
            ),
        ]
    );
}

#[test]
fn create_unique_conflict_rolls_back_record_indexes_and_commit_metadata() {
    let (program, runtime) = committed_program_and_runtime(CREATE_DELETE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    create_book(
        &program,
        &runtime,
        &store,
        &[SavedKey::Int(1)],
        "Dune",
        "Frank",
        "isbn-1",
    );
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");

    let create = SurfaceCreate::admit(&program, &store, surface).expect("admit create");
    assert_surface_error(
        create.create_point(
            &[SavedKey::Int(2)],
            &[
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["title"]),
                    value: SurfaceValue::Str("Hyperion".into()),
                },
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                    value: SurfaceValue::Str("Dan".into()),
                },
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["isbn"]),
                    value: SurfaceValue::Str("isbn-1".into()),
                },
            ],
        ),
        SURFACE_CONFLICT,
    );
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after rejected create")
            .expect("commit metadata remains"),
        baseline
    );
    assert_surface_error(
        read_surface_point(&program, &store, surface, &[SavedKey::Int(2)]),
        SURFACE_ABSENT,
    );
    let by_isbn = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byIsbn"),
    )
    .expect("admit isbn lookup");
    assert_eq!(
        by_isbn
            .lookup_unique(&[SavedKey::Str("isbn-1".into())])
            .expect("lookup conflicting isbn")
            .expect("original row remains indexed")
            .identity
            .expect("identity")
            .keys,
        vec![SavedKey::Int(1)]
    );
}

#[test]
fn stale_admitted_create_handle_rechecks_store_lineage_before_writing() {
    let (program, runtime) = committed_program_and_runtime(CREATE_DELETE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");
    let create = SurfaceCreate::admit(&program, &store, surface).expect("admit create");

    let mut stale = baseline.clone();
    stale.source_digest =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".into();
    store
        .write_commit_metadata(&stale)
        .expect("write stale metadata");

    assert_surface_error(
        create.create_point(
            &[SavedKey::Int(1)],
            &[
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["title"]),
                    value: SurfaceValue::Str("Dune".into()),
                },
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                    value: SurfaceValue::Str("Frank".into()),
                },
                SurfaceCreateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["isbn"]),
                    value: SurfaceValue::Str("isbn-1".into()),
                },
            ],
        ),
        SURFACE_ABI_MISMATCH,
    );
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after stale create")
            .expect("commit metadata remains"),
        stale
    );
    assert_eq!(
        read_data_value(
            &runtime,
            &store,
            "books",
            &[SavedKey::Int(1)],
            &data_path(&runtime, "books", &["title"]),
            marrow_store::value::ScalarType::Str,
        ),
        None
    );
}

#[test]
fn stale_admitted_delete_handle_rechecks_store_lineage_before_writing() {
    let (program, runtime) = committed_program_and_runtime(CREATE_DELETE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let identity = [SavedKey::Int(1)];
    create_book(
        &program, &runtime, &store, &identity, "Dune", "Frank", "isbn-1",
    );
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");
    let delete = SurfaceDelete::admit(&program, &store, surface).expect("admit delete");

    let mut stale = baseline.clone();
    stale.source_digest =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".into();
    store
        .write_commit_metadata(&stale)
        .expect("write stale metadata");

    assert_surface_error(delete.delete_point(&identity), SURFACE_ABI_MISMATCH);
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after stale delete")
            .expect("commit metadata remains"),
        stale
    );
    assert_eq!(
        read_data_value(
            &runtime,
            &store,
            "books",
            &identity,
            &data_path(&runtime, "books", &["title"]),
            marrow_store::value::ScalarType::Str,
        ),
        Some(SavedValue::Str("Dune".into()))
    );
    assert_eq!(
        read_data_value(
            &runtime,
            &store,
            "books",
            &identity,
            &data_path(&runtime, "books", &["author"]),
            marrow_store::value::ScalarType::Str,
        ),
        Some(SavedValue::Str("Frank".into()))
    );
    assert_eq!(
        read_data_value(
            &runtime,
            &store,
            "books",
            &identity,
            &data_path(&runtime, "books", &["isbn"]),
            marrow_store::value::ScalarType::Str,
        ),
        Some(SavedValue::Str("isbn-1".into()))
    );
}

#[test]
fn create_delete_tag_admission_fails_closed_for_wrong_unknown_and_duplicate_tags() {
    let (program, _runtime) = committed_program_and_runtime(CREATE_DELETE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let read_tag = read_operation_tag(&program, surface);

    assert_surface_error(
        SurfaceCreate::admit_by_operation_tag(&program, &store, &read_tag),
        SURFACE_ABI_MISMATCH,
    );
    assert_surface_error(
        SurfaceDelete::admit_by_operation_tag(&program, &store, &read_tag),
        SURFACE_ABI_MISMATCH,
    );
    assert_surface_error(
        SurfaceCreate::admit_by_operation_tag(&program, &store, "sha256:not-a-surface-tag"),
        SURFACE_ABI_MISMATCH,
    );

    let (duplicates, _runtime) =
        committed_program_and_runtime(DUPLICATE_CREATE_DELETE_TAG_SURFACES);
    let duplicate_store = admitted_store(&duplicates);
    let duplicate_surface = surface_id(&duplicates, "Books");
    assert_surface_error(
        SurfaceCreate::admit_by_operation_tag(
            &duplicates,
            &duplicate_store,
            &create_operation_tag(&duplicates, duplicate_surface),
        ),
        SURFACE_ABI_MISMATCH,
    );
    assert_surface_error(
        SurfaceDelete::admit_by_operation_tag(
            &duplicates,
            &duplicate_store,
            &delete_operation_tag(&duplicates, duplicate_surface),
        ),
        SURFACE_ABI_MISMATCH,
    );
}

fn create_book(
    program: &CheckedProgram,
    runtime: &CheckedRuntimeProgram,
    store: &TreeStore,
    identity: &[SavedKey],
    title: &str,
    author: &str,
    isbn: &str,
) {
    let create =
        SurfaceCreate::admit(program, store, surface_id(program, "Books")).expect("admit create");
    create
        .create_point(
            identity,
            &[
                SurfaceCreateField {
                    catalog_id: field_catalog_id(runtime, "books", &["title"]),
                    value: SurfaceValue::Str(title.into()),
                },
                SurfaceCreateField {
                    catalog_id: field_catalog_id(runtime, "books", &["author"]),
                    value: SurfaceValue::Str(author.into()),
                },
                SurfaceCreateField {
                    catalog_id: field_catalog_id(runtime, "books", &["isbn"]),
                    value: SurfaceValue::Str(isbn.into()),
                },
            ],
        )
        .expect("surface create succeeds");
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

fn index_collection_ref(
    program: &CheckedProgram,
    surface: SurfaceId,
    index_name: &str,
) -> marrow_run::SurfaceReadOperationRef {
    let surface_fact = program.facts.surface(surface);
    let ordinal = surface_fact
        .read_operations
        .iter()
        .position(|operation| match operation.kind {
            SurfaceReadOperationKind::PagedIndexCollection { index, .. }
            | SurfaceReadOperationKind::UniqueIndexLookup { index, .. } => {
                program.facts.store_index(index).name == index_name
            }
            _ => false,
        })
        .expect("surface operation is present");
    marrow_run::SurfaceReadOperationRef { surface, ordinal }
}

fn read_operation_tag(program: &CheckedProgram, surface: SurfaceId) -> String {
    program
        .facts
        .surface(surface)
        .read_operations
        .iter()
        .find(|operation| {
            matches!(
                operation.kind,
                SurfaceReadOperationKind::SingletonRead { .. }
                    | SurfaceReadOperationKind::PointRead { .. }
            )
        })
        .and_then(|operation| operation.operation_tag.clone())
        .expect("stable surface read operation tag")
}

fn create_operation_tag(program: &CheckedProgram, surface: SurfaceId) -> String {
    SurfaceCreateOperationDescriptor::from_surface(program, program.facts.surface(surface))
        .map(|descriptor| descriptor.operation_tag)
        .expect("stable surface create operation tag")
}

fn delete_operation_tag(program: &CheckedProgram, surface: SurfaceId) -> String {
    SurfaceDeleteOperationDescriptor::from_surface(program, program.facts.surface(surface))
        .map(|descriptor| descriptor.operation_tag)
        .expect("stable surface delete operation tag")
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

fn assert_surface_error<T>(result: Result<T, SurfaceError>, code: &str) {
    match result {
        Err(error) => assert_eq!(error.code(), code, "{error:?}"),
        Ok(_) => panic!("expected surface error {code}"),
    }
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
