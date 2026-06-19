use crate::support;
use support::*;

use marrow_check::{
    CheckedProgram, CheckedRuntimeProgram, ProjectConfig, SurfaceId, SurfaceReadOperationKind,
    SurfaceUpdateOperationDescriptor, check_project,
};
use marrow_run::{
    SURFACE_ABI_MISMATCH, SURFACE_ABSENT, SURFACE_CONFLICT, SURFACE_INVALID_DATA, SURFACE_REQUEST,
    SurfaceCollectionPageRequest, SurfaceCollectionRead, SurfaceError, SurfaceReadRecord,
    SurfaceUpdate, SurfaceUpdateField, SurfaceValue, read_surface_point, read_surface_singleton,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, StoreUid, TreeStore};
use marrow_store::value::SavedValue;

const SETTINGS_SURFACE: &str = "\
resource Settings
    required theme: string
    mode: string
store ^settings: Settings

surface SettingsSurface from ^settings
    fields theme, mode
    update theme, mode
";

const UPDATE_SURFACE: &str = "\
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
    update title, author, isbn
    collection ^books.byAuthor as byAuthor
    collection ^books.byIsbn as byIsbn
";

const COMPOSITE_UPDATE_SURFACE: &str = "\
resource Book
    required title: string
    required author: string
    required isbn: string
store ^books(id: int): Book
    index byAuthorIsbn(author, isbn) unique

surface Books from ^books
    fields title, author, isbn
    update author, isbn
    collection ^books.byAuthorIsbn as byAuthorIsbn
";

const READ_ONLY_SURFACE: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
";

const INDEXED_PRIVATE_SURFACE: &str = "\
resource Book
    required title: string
    author: string
    code: string
store ^books(id: int): Book
    index byAuthorCode(author, code, id)

surface Books from ^books
    fields title, author
    update author
    collection ^books.byAuthorCode as byAuthorCode
";

const DUPLICATE_UPDATE_TAG_SURFACES: &str = "\
resource Book
    required title: string
store ^books(id: int): Book

surface Books from ^books
    fields title
    update title

surface Library from ^books
    fields title
    update title
";

#[test]
fn sparse_update_preserves_omitted_fields_and_rewrites_indexes_atomically() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    update
        .update_point(
            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Ursula".into()),
            }],
        )
        .expect("surface update succeeds");

    let record = read_surface_point(
        &program,
        &store,
        surface,
        &[SavedKey::Str("a".into()), SavedKey::Int(1)],
    )
    .expect("surface read after update");
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "books", &["title"]),
                Some(SurfaceValue::Str("Dune".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["author"]),
                Some(SurfaceValue::Str("Ursula".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["isbn"]),
                Some(SurfaceValue::Str("isbn-a1".into())),
            ),
        ]
    );

    let by_author = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byAuthor"),
    )
    .expect("admit index collection");
    assert_eq!(
        collect_page_identities(&by_author, &[SavedKey::Str("Frank".into())], 10),
        Vec::<Vec<SavedKey>>::new()
    );
    assert_eq!(
        collect_page_identities(&by_author, &[SavedKey::Str("Ursula".into())], 10),
        vec![vec![SavedKey::Str("a".into()), SavedKey::Int(1)]]
    );
}

#[test]
fn surface_update_admits_checked_point_operation_tag() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");

    let surface = surface_id(&program, "Books");
    let tag = update_operation_tag(&program, surface);
    let update = SurfaceUpdate::admit_by_operation_tag(&program, &store, &tag)
        .expect("admit point update by operation tag");

    assert_eq!(update.surface(), surface);
    update
        .update_point(
            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Ursula".into()),
            }],
        )
        .expect("surface update succeeds");

    let record = read_surface_point(
        &program,
        &store,
        surface,
        &[SavedKey::Str("a".into()), SavedKey::Int(1)],
    )
    .expect("surface read after update");
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "books", &["title"]),
                Some(SurfaceValue::Str("Dune".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["author"]),
                Some(SurfaceValue::Str("Ursula".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["isbn"]),
                Some(SurfaceValue::Str("isbn-a1".into())),
            ),
        ]
    );
}

#[test]
fn surface_update_admits_checked_singleton_operation_tag() {
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
    let tag = update_operation_tag(&program, surface);
    let update = SurfaceUpdate::admit_by_operation_tag(&program, &store, &tag)
        .expect("admit singleton update by operation tag");

    assert_eq!(update.surface(), surface);
    update
        .update_singleton(&[SurfaceUpdateField {
            catalog_id: field_catalog_id(&runtime, "settings", &["mode"]),
            value: SurfaceValue::Str("compact".into()),
        }])
        .expect("singleton update succeeds");

    let record = read_surface_singleton(&program, &store, surface).expect("surface singleton read");
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "settings", &["theme"]),
                Some(SurfaceValue::Str("dark".into())),
            ),
            (
                field_catalog_id(&runtime, "settings", &["mode"]),
                Some(SurfaceValue::Str("compact".into())),
            ),
        ]
    );
}

#[test]
fn surface_update_tag_admission_fails_closed_for_read_unknown_and_duplicate_tags() {
    let (program, _runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let read_tag = read_operation_tag(&program, surface);

    assert_surface_error(
        SurfaceUpdate::admit_by_operation_tag(&program, &store, &read_tag),
        SURFACE_ABI_MISMATCH,
    );
    assert_surface_error(
        SurfaceUpdate::admit_by_operation_tag(&program, &store, "sha256:not-a-surface-tag"),
        SURFACE_ABI_MISMATCH,
    );

    let (duplicates, _runtime) = committed_program_and_runtime(DUPLICATE_UPDATE_TAG_SURFACES);
    let store = admitted_store(&duplicates);
    let tag = update_operation_tag(&duplicates, surface_id(&duplicates, "Books"));
    assert_surface_error(
        SurfaceUpdate::admit_by_operation_tag(&duplicates, &store, &tag),
        SURFACE_ABI_MISMATCH,
    );
}

#[test]
fn successful_surface_update_stamps_commit_metadata() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");

    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");
    assert_eq!(baseline.commit_id, 0);

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    update
        .update_point(
            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Ursula".into()),
            }],
        )
        .expect("surface update succeeds");

    let commit = store
        .read_commit_metadata()
        .expect("read update commit metadata")
        .expect("surface update is stamped");
    assert_eq!(commit.commit_id, baseline.commit_id + 1);
    assert_eq!(
        commit.catalog_epoch,
        program
            .catalog
            .accepted_epoch
            .expect("surface test has accepted catalog epoch")
    );
    assert_eq!(commit.source_digest, program.source_digest());
    assert_eq!(
        commit.changed_root_catalog_ids,
        vec![store_catalog_id(&runtime, "books")]
    );
    assert_eq!(
        commit.changed_index_catalog_ids,
        vec![index_catalog_id(&runtime, "books", "byAuthor")]
    );
}

#[test]
fn stale_admitted_update_handle_rechecks_store_lineage_before_writing() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");

    let mut stale = baseline.clone();
    stale.source_digest =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".into();
    store
        .write_commit_metadata(&stale)
        .expect("write stale metadata");

    assert_surface_error(
        update.update_point(
            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Ursula".into()),
            }],
        ),
        SURFACE_ABI_MISMATCH,
    );
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after stale update")
            .expect("commit metadata remains"),
        stale
    );
    assert_eq!(
        read_data_value(
            &runtime,
            &store,
            "books",
            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
            &data_path(&runtime, "books", &["author"]),
            marrow_store::value::ScalarType::Str,
        ),
        Some(SavedValue::Str("Frank".into()))
    );
}

#[test]
fn update_rejects_corrupt_required_private_backing_data_and_rolls_back() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Str("a".into()), SavedKey::Int(1)];
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    write_data_bytes(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["privateCode"]),
        vec![0xff],
    );
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    assert_surface_error(
        update.update_point(
            &identity,
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Ursula".into()),
            }],
        ),
        SURFACE_INVALID_DATA,
    );
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after invalid update")
            .expect("commit metadata remains"),
        baseline
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
}

#[test]
fn update_rejects_corrupt_projected_backing_data_and_rolls_back() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Str("a".into()), SavedKey::Int(1)];
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    write_data_bytes(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["title"]),
        vec![0xff],
    );
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    assert_surface_error(
        update.update_point(
            &identity,
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Ursula".into()),
            }],
        ),
        SURFACE_INVALID_DATA,
    );
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after invalid update")
            .expect("commit metadata remains"),
        baseline
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
}

#[test]
fn update_requires_declared_update_fields_and_non_empty_patch() {
    let (read_only, _runtime) = committed_program_and_runtime(READ_ONLY_SURFACE);
    let store = admitted_store(&read_only);
    match SurfaceUpdate::admit(&read_only, &store, surface_id(&read_only, "Books")) {
        Err(error) => assert_eq!(error.code(), SURFACE_REQUEST, "{error:?}"),
        Ok(_) => panic!("expected read-only surface update admission to fail"),
    }

    let (program, _runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    write_book(
        &program.runtime(),
        &store,
        "a",
        1,
        "Dune",
        "Frank",
        "isbn-a1",
    );
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");
    let update =
        SurfaceUpdate::admit(&program, &store, surface_id(&program, "Books")).expect("admit");

    assert_surface_error(
        update.update_point(&[SavedKey::Str("a".into()), SavedKey::Int(1)], &[]),
        SURFACE_REQUEST,
    );
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after empty update")
            .expect("commit metadata remains"),
        baseline
    );
}

#[test]
fn malformed_stored_indexed_value_is_invalid_data_not_store_failure() {
    let (program, runtime) = committed_program_and_runtime(INDEXED_PRIVATE_SURFACE);
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
        &data_path(&runtime, "books", &["author"]),
        SavedValue::Str("Frank".into()),
    );
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["code"]),
        SavedValue::Str("code-1".into()),
    );
    write_non_unique_index_entry(
        &runtime,
        &store,
        "byAuthorCode",
        &[
            SavedKey::Str("Frank".into()),
            SavedKey::Str("code-1".into()),
            SavedKey::Int(1),
        ],
        &identity,
    );
    write_data_bytes(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["code"]),
        vec![0xff],
    );
    let baseline = store
        .read_commit_metadata()
        .expect("read baseline commit metadata")
        .expect("catalog baseline is stamped");

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    assert_surface_error(
        update.update_point(
            &identity,
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Ursula".into()),
            }],
        ),
        SURFACE_INVALID_DATA,
    );
    assert_eq!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after invalid index update")
            .expect("commit metadata remains"),
        baseline
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
}

#[test]
fn unique_conflict_leaves_every_field_and_index_unchanged() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");
    write_book(&runtime, &store, "b", 2, "Hyperion", "Dan", "isbn-b2");

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    assert_surface_error(
        update.update_point(
            &[SavedKey::Str("b".into()), SavedKey::Int(2)],
            &[
                SurfaceUpdateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["title"]),
                    value: SurfaceValue::Str("Changed".into()),
                },
                SurfaceUpdateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["isbn"]),
                    value: SurfaceValue::Str("isbn-a1".into()),
                },
            ],
        ),
        SURFACE_CONFLICT,
    );

    let unchanged = read_surface_point(
        &program,
        &store,
        surface,
        &[SavedKey::Str("b".into()), SavedKey::Int(2)],
    )
    .expect("surface read after rejected update");
    assert_eq!(
        field_values(&unchanged),
        vec![
            (
                field_catalog_id(&runtime, "books", &["title"]),
                Some(SurfaceValue::Str("Hyperion".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["author"]),
                Some(SurfaceValue::Str("Dan".into())),
            ),
            (
                field_catalog_id(&runtime, "books", &["isbn"]),
                Some(SurfaceValue::Str("isbn-b2".into())),
            ),
        ]
    );

    let by_isbn = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byIsbn"),
    )
    .expect("admit unique lookup");
    assert_eq!(
        by_isbn
            .lookup_unique(&[SavedKey::Str("isbn-b2".into())])
            .expect("lookup unchanged isbn")
            .expect("unchanged row is indexed")
            .identity
            .expect("identity")
            .keys,
        vec![SavedKey::Str("b".into()), SavedKey::Int(2)]
    );
    assert_eq!(
        by_isbn
            .lookup_unique(&[SavedKey::Str("isbn-a1".into())])
            .expect("lookup original isbn")
            .expect("original row is indexed")
            .identity
            .expect("identity")
            .keys,
        vec![SavedKey::Str("a".into()), SavedKey::Int(1)]
    );
}

#[test]
fn update_request_rejects_duplicate_disallowed_and_unknown_catalog_fields() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    write_book(&runtime, &store, "a", 1, "Dune", "Frank", "isbn-a1");

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    let title = field_catalog_id(&runtime, "books", &["title"]);
    let private = field_catalog_id(&runtime, "books", &["privateCode"]);

    assert_surface_error(
        update.update_point(
            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
            &[
                SurfaceUpdateField {
                    catalog_id: title.clone(),
                    value: SurfaceValue::Str("One".into()),
                },
                SurfaceUpdateField {
                    catalog_id: title,
                    value: SurfaceValue::Str("Two".into()),
                },
            ],
        ),
        SURFACE_REQUEST,
    );
    assert_surface_error(
        update.update_point(
            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: private,
                value: SurfaceValue::Str("leak".into()),
            }],
        ),
        SURFACE_REQUEST,
    );
    assert_surface_error(
        update.update_point(
            &[SavedKey::Str("a".into()), SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: CatalogId::new("cat_ffffffffffffffffffffffffffffffff")
                    .expect("test catalog id"),
                value: SurfaceValue::Str("unknown".into()),
            }],
        ),
        SURFACE_REQUEST,
    );
}

#[test]
fn update_absent_point_returns_absent_without_upsert() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");

    assert_surface_error(
        update.update_point(
            &[SavedKey::Str("missing".into()), SavedKey::Int(404)],
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["title"]),
                value: SurfaceValue::Str("Ghost".into()),
            }],
        ),
        SURFACE_ABSENT,
    );
    assert_surface_error(
        read_surface_point(
            &program,
            &store,
            surface,
            &[SavedKey::Str("missing".into()), SavedKey::Int(404)],
        ),
        SURFACE_ABSENT,
    );
}

#[test]
fn source_only_surface_update_admission_is_an_abi_mismatch() {
    let program = source_only_program(UPDATE_SURFACE);
    let store = TreeStore::memory();

    match SurfaceUpdate::admit(&program, &store, surface_id(&program, "Books")) {
        Err(error) => assert_eq!(error.code(), SURFACE_ABI_MISMATCH, "{error:?}"),
        Ok(_) => panic!("expected surface update admission to fail"),
    }
}

#[test]
fn update_rejects_existing_incomplete_backing_record_as_invalid_data() {
    let (program, runtime) = committed_program_and_runtime(UPDATE_SURFACE);
    let store = admitted_store(&program);
    let identity = [SavedKey::Str("a".into()), SavedKey::Int(1)];
    write_record_presence(&runtime, &store, "books", &identity);

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    assert_surface_error(
        update.update_point(
            &identity,
            &[SurfaceUpdateField {
                catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                value: SurfaceValue::Str("Frank".into()),
            }],
        ),
        SURFACE_INVALID_DATA,
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
        None
    );
}

#[test]
fn update_rewrites_composite_indexes_from_the_combined_patch() {
    let (program, runtime) = committed_program_and_runtime(COMPOSITE_UPDATE_SURFACE);
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
        &data_path(&runtime, "books", &["author"]),
        SavedValue::Str("Frank".into()),
    );
    write_data_value(
        &runtime,
        &store,
        "books",
        &identity,
        &data_path(&runtime, "books", &["isbn"]),
        SavedValue::Str("isbn-old".into()),
    );
    write_unique_index_entry(
        &runtime,
        &store,
        "byAuthorIsbn",
        &[
            SavedKey::Str("Frank".into()),
            SavedKey::Str("isbn-old".into()),
        ],
        &identity,
    );

    let surface = surface_id(&program, "Books");
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit surface update");
    update
        .update_point(
            &identity,
            &[
                SurfaceUpdateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["author"]),
                    value: SurfaceValue::Str("Ursula".into()),
                },
                SurfaceUpdateField {
                    catalog_id: field_catalog_id(&runtime, "books", &["isbn"]),
                    value: SurfaceValue::Str("isbn-new".into()),
                },
            ],
        )
        .expect("surface update succeeds");

    let by_pair = SurfaceCollectionRead::admit(
        &program,
        &store,
        index_collection_ref(&program, surface, "byAuthorIsbn"),
    )
    .expect("admit unique lookup");
    assert_eq!(
        by_pair
            .lookup_unique(&[
                SavedKey::Str("Ursula".into()),
                SavedKey::Str("isbn-new".into()),
            ])
            .expect("lookup updated tuple")
            .expect("updated tuple is indexed")
            .identity
            .expect("identity")
            .keys,
        vec![SavedKey::Int(1)]
    );
    assert_eq!(
        by_pair
            .lookup_unique(&[
                SavedKey::Str("Frank".into()),
                SavedKey::Str("isbn-old".into()),
            ])
            .expect("lookup old tuple"),
        None
    );
}

#[test]
fn singleton_update_targets_the_keyless_record() {
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
    let update = SurfaceUpdate::admit(&program, &store, surface).expect("admit singleton update");
    update
        .update_singleton(&[SurfaceUpdateField {
            catalog_id: field_catalog_id(&runtime, "settings", &["mode"]),
            value: SurfaceValue::Str("compact".into()),
        }])
        .expect("singleton update succeeds");

    let record = read_surface_singleton(&program, &store, surface).expect("surface singleton read");
    assert_eq!(
        field_values(&record),
        vec![
            (
                field_catalog_id(&runtime, "settings", &["theme"]),
                Some(SurfaceValue::Str("dark".into())),
            ),
            (
                field_catalog_id(&runtime, "settings", &["mode"]),
                Some(SurfaceValue::Str("compact".into())),
            ),
        ]
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

fn update_operation_tag(program: &CheckedProgram, surface: SurfaceId) -> String {
    SurfaceUpdateOperationDescriptor::from_surface(program, program.facts.surface(surface))
        .map(|descriptor| descriptor.operation_tag)
        .expect("stable surface update operation tag")
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
    let root = TempDir::new("marrow-surface-update-test").expect("create surface test project");
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
