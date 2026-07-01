use crate::support;
use std::fs;
use std::path::Path;

use marrow_check::tooling::SavedDataPathSegment;
use marrow_check::{
    ProjectConfig, StoreBackend, StoreConfig, SurfaceId, SurfaceReadOperationKind,
    SurfaceUpdateOperationDescriptor,
};
use marrow_run::evolution::{Approval, apply};
use marrow_run::{
    DataViewUnavailableReason, DataViewWatchTargetKind, EntryArgument, EntryArgumentValue,
    EntryDescriptor, EntryInvocation, EntryScalarArgument, ExecutionBoundaryStoreKind,
    ExecutionSessionKind, Host, ProjectMode, ProjectOpen, ProjectSession,
    ProjectSurfaceReadSession, ProjectSurfaceSession, RUN_ENTRY_ARGUMENT, SURFACE_ABI_MISMATCH,
    SessionEntry, SurfaceCollectionPageRequest, SurfaceReadInput, SurfaceServeMode,
    SurfaceServeProcessControl, SurfaceUpdateField, SurfaceValue,
    data_view_unavailable_reason_for_config,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use support::{TempDir, write_temp_source};

fn native_config() -> ProjectConfig {
    ProjectConfig {
        source_roots: vec!["src".into()],
        default_entry: Some("shelf::show".into()),
        store: StoreConfig {
            backend: StoreBackend::Native,
            data_dir: Some(".data".into()),
        },
        tests: Vec::new(),
        client: None,
    }
}

fn write_native_config(root: &Path) {
    fs::write(
        root.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::show" } }"#,
    )
    .expect("write marrow.json");
}

/// The committed source-tree lock projection a baseline run writes from the store.
fn lock_path(root: &Path) -> std::path::PathBuf {
    root.join("marrow.lock")
}

/// A native config with no `run.defaultEntry`, for fixtures that drive a
/// parameterized entry through an explicit override rather than the default.
fn write_native_config_no_default(root: &Path) {
    fs::write(
        root.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
    )
    .expect("write marrow.json");
}

fn write_memory_config_with_tests(root: &Path) {
    fs::write(
        root.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "tests": ["tests"] }"#,
    )
    .expect("write marrow.json");
}

fn write_native_config_with_tests(root: &Path) {
    fs::write(
        root.join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "tests": ["tests"] }"#,
    )
    .expect("write marrow.json");
}

fn baseline_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"baseline\")\n"
}

fn advanced_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     \x20\x20\x20\x20label: string\n\
     store ^counter(id: int): Counter\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"advanced\")\n"
}

fn persistent_counter_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
     pub fn bump()\n\
     \x20\x20\x20\x20var c: Counter\n\
     \x20\x20\x20\x20c.value = 1\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n\
     pub fn show()\n\
     \x20\x20\x20\x20if not exists(^counter(1))\n\
     \x20\x20\x20\x20\x20\x20\x20\x20print(\"absent\")\n\
     \x20\x20\x20\x20\x20\x20\x20\x20return\n\
     \x20\x20\x20\x20if const value = ^counter(1).value\n\
     \x20\x20\x20\x20\x20\x20\x20\x20print($\"value={value}\")\n"
}

fn mutable_counter_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
     pub fn setOne()\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1).value = 1\n\
     pub fn setTwo()\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1).value = 2\n\
     pub fn setThree()\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1).value = 3\n\
     pub fn show()\n\
     \x20\x20\x20\x20if const value = ^counter(1).value\n\
     \x20\x20\x20\x20\x20\x20\x20\x20print($\"value={value}\")\n\
     \x20\x20\x20\x20\x20\x20\x20\x20return\n\
     \x20\x20\x20\x20print(\"absent\")\n"
}

fn surface_counter_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     \x20\x20\x20\x20note: string\n\
     store ^counter(id: int): Counter\n\
     surface Counters from ^counter\n\
     \x20\x20\x20\x20fields value, note\n\
     \x20\x20\x20\x20update value\n\
     \x20\x20\x20\x20collection ^counter as list\n\
     pub fn seed()\n\
     \x20\x20\x20\x20var c: Counter\n\
     \x20\x20\x20\x20c.value = 1\n\
     \x20\x20\x20\x20c.note = \"seeded\"\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"baseline\")\n"
}

fn advanced_surface_counter_source() -> &'static str {
    "module shelf\n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     \x20\x20\x20\x20note: string\n\
     \x20\x20\x20\x20label: string\n\
     store ^counter(id: int): Counter\n\
     surface Counters from ^counter\n\
     \x20\x20\x20\x20fields value, note, label\n\
     \x20\x20\x20\x20update value\n\
     \x20\x20\x20\x20collection ^counter as list\n\
     pub fn seed()\n\
     \x20\x20\x20\x20var c: Counter\n\
     \x20\x20\x20\x20c.value = 1\n\
     \x20\x20\x20\x20c.note = \"seeded\"\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"advanced\")\n"
}

fn surface_books_with_legacy_source() -> &'static str {
    "module books\n\
     enum Status\n\
     \x20\x20\x20\x20old\n\
     \x20\x20\x20\x20current\n\
     resource Book\n\
     \x20\x20\x20\x20required title: string\n\
     store ^books(id: int): Book\n\
     surface Books from ^books\n\
     \x20\x20\x20\x20fields title\n\
     \x20\x20\x20\x20collection ^books as list\n\
     pub fn seed()\n\
     \x20\x20\x20\x20var b: Book\n\
     \x20\x20\x20\x20b.title = \"Dune\"\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^books(1) = b\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"books\")\n"
}

fn surface_books_retire_legacy_source() -> &'static str {
    "module books\n\
     enum Status\n\
     \x20\x20\x20\x20current\n\
     resource Book\n\
     \x20\x20\x20\x20required title: string\n\
     store ^books(id: int): Book\n\
     surface Books from ^books\n\
     \x20\x20\x20\x20fields title\n\
     \x20\x20\x20\x20collection ^books as list\n\
     evolve\n\
     \x20\x20\x20\x20retire Status.old\n\
     pub fn seed()\n\
     \x20\x20\x20\x20var b: Book\n\
     \x20\x20\x20\x20b.title = \"Dune\"\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^books(1) = b\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"books\")\n"
}

fn surface_books_after_retire_source() -> &'static str {
    "module books\n\
     enum Status\n\
     \x20\x20\x20\x20current\n\
     resource Book\n\
     \x20\x20\x20\x20required title: string\n\
     store ^books(id: int): Book\n\
     surface Books from ^books\n\
     \x20\x20\x20\x20fields title\n\
     \x20\x20\x20\x20collection ^books as list\n\
     pub fn seed()\n\
     \x20\x20\x20\x20var b: Book\n\
     \x20\x20\x20\x20b.title = \"Dune\"\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^books(1) = b\n\
     pub fn show()\n\
     \x20\x20\x20\x20print(\"books\")\n"
}

fn invoke(session: &ProjectSession, entry: &str) -> String {
    let host = Host::new();
    let mut output = String::new();
    session
        .invoke(SessionEntry::new(entry, &host, &mut output))
        .expect("invoke session entry");
    output
}

fn checked_source_identity(root: &Path) -> marrow_check::AnalysisIdentity {
    let config = marrow_check::load_config(root).expect("load config");
    let accepted =
        marrow_check::read_accepted_catalog_artifact(root).expect("read accepted catalog");
    marrow_check::check_source_project_analysis_against(root, &config, accepted.as_ref(), None)
        .expect("check source analysis")
        .content_identity
}

fn checked_program_against_accepted(
    root: &Path,
    accepted: &marrow_catalog::CatalogMetadata,
) -> marrow_check::CheckedProgram {
    let config = marrow_check::load_config(root).expect("load config");
    marrow_check::check_project_against(root, &config, Some(accepted), None)
        .expect("check project against accepted catalog")
}

fn accepted_catalog_from_program(
    program: &marrow_check::CheckedProgram,
) -> marrow_catalog::CatalogMetadata {
    marrow_catalog::CatalogMetadata::from_stored_parts(
        program.catalog.accepted_epoch.expect("accepted epoch"),
        program
            .catalog
            .accepted_digest
            .clone()
            .expect("accepted digest"),
        program.catalog.accepted_entries.clone(),
    )
    .expect("accepted catalog from checked program")
}

fn lock_bound_checked_program(root: &Path) -> marrow_check::CheckedProgram {
    let config = marrow_check::load_config(root).expect("load config");
    let lock = marrow_check::read_committed_lock(root)
        .expect("read committed lock")
        .expect("committed lock");
    marrow_check::check_project_against(root, &config, None, Some(&lock))
        .expect("check project against committed lock")
}

#[test]
fn surface_read_session_serves_existing_native_store_without_advancing_it() {
    let root = TempDir::new("marrow-run-surface-read-session").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let store_path = root.path().join(".data").join("marrow.redb");
    let lock_path = lock_path(root.path());
    let lock_before = fs::read(&lock_path).expect("seed run projects the committed lock");
    let before = {
        let store = TreeStore::open_read_only(&store_path).expect("open seeded store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("seed run stamps the store")
    };

    let session = ProjectSurfaceReadSession::open(root.path()).expect("open surface read session");
    assert_eq!(
        session
            .store_stamp()
            .expect("read surface session store stamp")
            .commit_id,
        before.commit_id
    );
    let point_tag = point_read_operation_tag(session.program(), "Counters");
    let record = session
        .admit_read_by_operation_tag(&point_tag)
        .expect("admit point read")
        .point_read()
        .expect("point read shape")
        .execute(SurfaceReadInput::Point {
            identity: &[SavedKey::Int(1)],
        })
        .expect("read surface record");
    assert_eq!(
        record.identity.expect("point read includes identity").keys,
        vec![SavedKey::Int(1)]
    );
    assert_eq!(record.fields[0].value, Some(SurfaceValue::Int(1)));

    let page_tag = root_page_operation_tag(session.program(), "Counters");
    let page = session
        .admit_read_by_operation_tag(&page_tag)
        .expect("admit root page")
        .page_read()
        .expect("page read shape")
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 10,
            cursor: None,
        })
        .expect("read surface page");
    assert_eq!(page.rows.len(), 1);

    let roots = session.saved_data_roots().expect("read saved data roots");
    assert_eq!(roots.data.len(), 1);
    assert_eq!(roots.data[0].label, "counter");
    let counter_root = roots.data[0].segment.clone();
    let children = session
        .saved_data_children(std::slice::from_ref(&counter_root), 10, None)
        .expect("read saved data children");
    assert_eq!(
        children.data.children,
        vec![marrow_check::tooling::DataChildView {
            segment: marrow_check::tooling::SavedDataPathSegment::Key(SavedKey::Int(1)),
            label: "(1)".into(),
        }]
    );
    let record_key = children.data.children[0].segment.clone();
    let value_children = session
        .saved_data_children(&[counter_root.clone(), record_key], 10, None)
        .expect("read saved data member children");
    let member_labels = value_children
        .data
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(member_labels, vec!["value", "note"]);
    let value_field = value_children
        .data
        .children
        .iter()
        .find(|child| child.label == "value")
        .expect("value member child")
        .segment
        .clone();
    let preview = session
        .saved_data_preview(
            &[
                counter_root,
                SavedDataPathSegment::Key(SavedKey::Int(1)),
                value_field,
            ],
            16,
        )
        .expect("preview saved data path")
        .expect("path is present");
    assert_eq!(preview.data.preview.expect("value preview").text, "1");
    let integrity = session
        .saved_data_integrity_sample(10)
        .expect("sample saved data integrity");
    assert!(integrity.data.items_checked > 0);

    let update_tag = update_operation_tag(session.program(), "Counters");
    let error = match session.admit_read_by_operation_tag(&update_tag) {
        Ok(_) => panic!("read session must reject update operation tags"),
        Err(error) => error,
    };
    assert_eq!(error.code(), SURFACE_ABI_MISMATCH);
    assert_eq!(
        session
            .store_stamp()
            .expect("read surface session store stamp after rejected tag")
            .commit_id,
        before.commit_id
    );
    drop(session);

    let after = {
        let store = TreeStore::open_read_only(&store_path).expect("reopen seeded store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("surface read keeps the store stamped")
    };
    assert_eq!(before, after, "surface reads must not advance commits");
    assert_eq!(
        lock_before,
        fs::read(&lock_path).expect("read committed lock after surface read"),
        "surface reads must not rewrite the committed lock"
    );
}

#[test]
fn surface_read_session_boundary_reports_generation_store_and_watch_targets() {
    let root = TempDir::new("marrow-run-surface-data-view-boundary").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let session = ProjectSurfaceReadSession::open(root.path()).expect("open surface read session");
    let boundary = session.data_view_boundary();

    assert_eq!(
        boundary.source_analysis_generation.checked_source_digest,
        session.program().source_digest()
    );
    assert_eq!(
        boundary.store_snapshot.checked_source_digest,
        session.program().source_digest()
    );
    assert!(
        boundary.store_snapshot.store_uid.is_some(),
        "admitted native data view should carry a store uid"
    );
    assert!(
        boundary.store_snapshot.store_commit.is_some(),
        "admitted native data view should carry committed store metadata"
    );
    assert_eq!(
        boundary
            .watch_targets
            .iter()
            .map(|target| (target.kind, target.path.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                DataViewWatchTargetKind::StoreFile,
                root.path().join(".data").join("marrow.redb"),
            ),
            (DataViewWatchTargetKind::CatalogLock, lock_path(root.path())),
        ]
    );
}

#[test]
fn data_view_unavailable_reason_follows_project_store_config() {
    let root = TempDir::new("marrow-run-data-view-unavailable-reason").expect("create project");
    write_memory_config_with_tests(root.path());
    let config = marrow_check::load_config(root.path()).expect("load memory config");

    assert_eq!(
        data_view_unavailable_reason_for_config(root.path(), &config)
            .expect("classify memory config"),
        Some(DataViewUnavailableReason::MemoryStore)
    );

    write_native_config(root.path());
    let config = marrow_check::load_config(root.path()).expect("load native config");
    assert_eq!(
        data_view_unavailable_reason_for_config(root.path(), &config)
            .expect("classify missing native store"),
        Some(DataViewUnavailableReason::NativeStoreMissing)
    );
}

#[test]
fn surface_serve_boundary_reports_mode_store_watch_targets_and_process_control() {
    let root = TempDir::new("marrow-run-surface-serve-boundary").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let run_session = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open isolated run session");
    let run_boundary = run_session
        .surface_serve_boundary()
        .expect("run serve boundary");
    assert_eq!(run_boundary.mode, SurfaceServeMode::Write);
    assert_eq!(
        run_boundary
            .data_view_boundary
            .source_analysis_generation
            .checked_source_digest,
        run_session.program().source_digest()
    );
    assert_eq!(
        run_boundary
            .data_view_boundary
            .store_snapshot
            .checked_source_digest,
        run_session.program().source_digest()
    );
    assert_eq!(
        run_boundary.process_control,
        SurfaceServeProcessControl::NotExposed
    );
    assert!(
        run_boundary.data_view_boundary.watch_targets.is_empty(),
        "isolated run sessions must not publish live store watch targets"
    );
    drop(run_session);

    let read_session =
        ProjectSurfaceReadSession::open(root.path()).expect("open surface read session");
    let read_boundary = read_session
        .surface_serve_boundary()
        .expect("read serve boundary");

    assert_eq!(read_boundary.mode, SurfaceServeMode::ReadOnly);
    assert_eq!(
        read_boundary
            .data_view_boundary
            .source_analysis_generation
            .checked_source_digest,
        read_session.program().source_digest()
    );
    assert_eq!(
        read_boundary
            .data_view_boundary
            .store_snapshot
            .checked_source_digest,
        read_session.program().source_digest()
    );
    assert_eq!(
        read_boundary
            .data_view_boundary
            .watch_targets
            .iter()
            .map(|target| (target.kind, target.path.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                DataViewWatchTargetKind::StoreFile,
                root.path().join(".data").join("marrow.redb"),
            ),
            (DataViewWatchTargetKind::CatalogLock, lock_path(root.path())),
        ]
    );
    assert_eq!(
        read_boundary.process_control,
        SurfaceServeProcessControl::NotExposed
    );
    drop(read_session);

    let write_session =
        ProjectSurfaceSession::open(root.path()).expect("open surface write session");
    let write_boundary = write_session
        .surface_serve_boundary()
        .expect("write serve boundary");
    let before_write_commit = write_boundary
        .data_view_boundary
        .store_snapshot
        .store_commit
        .as_ref()
        .expect("write boundary carries committed store metadata")
        .commit_id;

    assert_eq!(write_boundary.mode, SurfaceServeMode::Write);
    assert_eq!(
        write_boundary
            .data_view_boundary
            .source_analysis_generation
            .checked_source_digest,
        write_session.program().source_digest()
    );
    assert_eq!(
        write_boundary
            .data_view_boundary
            .store_snapshot
            .checked_source_digest,
        write_session.program().source_digest()
    );
    assert_eq!(
        write_boundary.process_control,
        SurfaceServeProcessControl::NotExposed
    );

    let update_tag = update_operation_tag(write_session.program(), "Counters");
    write_session
        .admit_update_by_operation_tag(&update_tag)
        .expect("admit point update")
        .update_point(
            &[SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: update_field_catalog_id(write_session.program(), "Counters", "value"),
                value: SurfaceValue::Int(11),
            }],
        )
        .expect("execute point update");
    let after_write_commit = write_session
        .surface_serve_boundary()
        .expect("write serve boundary after update")
        .data_view_boundary
        .store_snapshot
        .store_commit
        .as_ref()
        .expect("write boundary carries committed store metadata after update")
        .commit_id;
    assert!(
        after_write_commit > before_write_commit,
        "write serve boundary must report the current store snapshot after admitted writes"
    );
}

#[test]
fn surface_read_session_admits_lock_bound_checked_program() {
    let root = TempDir::new("marrow-run-surface-admits-lock-bound").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let lock_bound = lock_bound_checked_program(root.path());

    let read_session =
        ProjectSurfaceReadSession::open(root.path()).expect("open surface read session");
    assert_eq!(
        lock_bound.catalog.accepted_digest,
        read_session.program().catalog.accepted_digest,
        "committed-lock analysis binds the same accepted digest a present store binds, so the \
         read-only context digest is writer-independent"
    );
    assert!(read_session.admits_checked_program(read_session.program()));
    assert!(read_session.admits_checked_program(&lock_bound));
}

#[test]
fn surface_read_session_admits_lock_bound_program_after_non_final_retire() {
    let root = TempDir::new("marrow-run-surface-admits-lock-bound-retire").expect("create project");
    write_native_config_no_default(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/books.mw"),
        surface_books_with_legacy_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("books::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "books::seed"), "");
    drop(seed);

    let store_path = root.path().join(".data").join("marrow.redb");
    let store = TreeStore::open(&store_path).expect("open native store");
    let accepted = store
        .read_catalog_snapshot()
        .expect("read accepted catalog")
        .expect("accepted catalog");
    let old_status_id = accepted
        .entries
        .iter()
        .find(|entry| entry.path == "books::Status::old")
        .expect("old status accepted entry")
        .stable_id
        .clone();

    write_temp_source(
        root.path(),
        Path::new("src/books.mw"),
        surface_books_retire_legacy_source(),
    );
    let retiring = checked_program_against_accepted(root.path(), &accepted);
    let (witness, _diagnostics) =
        marrow_check::evolution::preview(&retiring, &store).expect("preview retire");
    let approval = Approval {
        retires: vec![(CatalogId::new(old_status_id).expect("old status id"), 0)],
    };
    apply(&witness, &retiring, &store, true, Some(&approval)).expect("apply retire");
    let retired = store
        .read_catalog_snapshot()
        .expect("read retired catalog")
        .expect("retired catalog");
    assert_eq!(
        retired
            .entries
            .iter()
            .find(|entry| entry.path == "books::Status::old")
            .expect("old status member remains reserved")
            .lifecycle,
        marrow_catalog::CatalogLifecycle::Reserved
    );
    write_temp_source(
        root.path(),
        Path::new("src/books.mw"),
        surface_books_after_retire_source(),
    );
    let current = checked_program_against_accepted(root.path(), &retired);
    marrow_check::project_store_lock(root.path(), &retired, &current.source_digest())
        .expect("project retired lock");
    drop(store);

    let session = ProjectSurfaceReadSession::open(root.path()).expect("open surface read session");
    let lock_bound = lock_bound_checked_program(root.path());
    let session_catalog = accepted_catalog_from_program(session.program());
    let lock_catalog = marrow_catalog::CatalogMetadata::new(
        lock_bound.catalog.accepted_epoch.expect("accepted epoch"),
        lock_bound.catalog.accepted_entries.clone(),
    )
    .expect("lock-bound accepted catalog is valid");
    assert_eq!(
        lock_catalog.digest, session_catalog.digest,
        "store-bound and lock-bound accepted identity must be canonical-equivalent"
    );
    // The bound accepted digest is the canonical catalog digest, not absent: a committed-lock
    // analysis binds the same digest a present store binds even when accepted-entry order drifts,
    // so the read-only context digest is writer-independent and order-independent.
    assert_eq!(
        lock_bound.catalog.accepted_digest.as_deref(),
        Some(lock_catalog.digest.as_str()),
        "committed-lock analysis binds the canonical accepted digest"
    );
    assert_ne!(
        lock_bound.catalog.accepted_entries,
        session.program().catalog.accepted_entries,
        "fixture must exercise accepted-entry order drift"
    );

    assert!(session.admits_checked_program(&lock_bound));
}

#[test]
fn surface_read_session_admission_rejects_changed_source() {
    let root = TempDir::new("marrow-run-surface-admission-source-change").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let session = ProjectSurfaceReadSession::open(root.path()).expect("open surface read session");
    let accepted = accepted_catalog_from_program(session.program());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        advanced_surface_counter_source(),
    );

    let changed = checked_program_against_accepted(root.path(), &accepted);

    assert!(!session.admits_checked_program(&changed));
}

#[test]
fn surface_read_session_admission_rejects_changed_store_or_lock_authority() {
    let root =
        TempDir::new("marrow-run-surface-admission-authority-change").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let session = ProjectSurfaceReadSession::open(root.path()).expect("open surface read session");
    let accepted = accepted_catalog_from_program(session.program());
    let changed_store =
        marrow_catalog::CatalogMetadata::new(accepted.epoch + 1, accepted.entries.clone())
            .expect("changed store authority");
    let changed_store_program = checked_program_against_accepted(root.path(), &changed_store);
    assert!(!session.admits_checked_program(&changed_store_program));

    let lock = marrow_check::read_committed_lock(root.path())
        .expect("read committed lock")
        .expect("committed lock");
    let changed_lock = marrow_catalog::CatalogLock::new(
        lock.entries,
        lock.ledger,
        lock.epoch_high_water + 1,
        lock.source_digest,
    )
    .expect("changed lock authority");
    let config = marrow_check::load_config(root.path()).expect("load config");
    let changed_lock_program =
        marrow_check::check_project_against(root.path(), &config, None, Some(&changed_lock))
            .expect("check project against changed lock");

    assert!(!session.admits_checked_program(&changed_lock_program));
}

#[test]
fn surface_write_session_updates_existing_native_store() {
    let root = TempDir::new("marrow-run-surface-write-session").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let session = ProjectSurfaceSession::open(root.path()).expect("open surface write session");
    let before = session
        .store_stamp()
        .expect("write surface session store stamp");
    let update_tag = update_operation_tag(session.program(), "Counters");
    session
        .admit_update_by_operation_tag(&update_tag)
        .expect("admit point update")
        .update_point(
            &[SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: update_field_catalog_id(session.program(), "Counters", "value"),
                value: SurfaceValue::Int(7),
            }],
        )
        .expect("execute point update");

    let point_tag = point_read_operation_tag(session.program(), "Counters");
    let record = session
        .admit_read_by_operation_tag(&point_tag)
        .expect("admit point read")
        .point_read()
        .expect("point read shape")
        .execute(SurfaceReadInput::Point {
            identity: &[SavedKey::Int(1)],
        })
        .expect("read updated surface record");
    assert_eq!(record.fields[0].value, Some(SurfaceValue::Int(7)));

    let error = match session.admit_update_by_operation_tag(&point_tag) {
        Ok(_) => panic!("write session update admission must reject read operation tags"),
        Err(error) => error,
    };
    assert_eq!(error.code(), SURFACE_ABI_MISMATCH);

    let after = session
        .store_stamp()
        .expect("write surface session store stamp after update");
    assert_eq!(after.store_uid, before.store_uid);
    assert_eq!(after.catalog_epoch, before.catalog_epoch);
    assert!(
        after.commit_id > before.commit_id,
        "surface update must advance the store commit"
    );
    drop(session);

    let read_session =
        ProjectSurfaceReadSession::open(root.path()).expect("reopen surface read session");
    let record = read_session
        .admit_read_by_operation_tag(&point_tag)
        .expect("admit reopened point read")
        .point_read()
        .expect("point read shape")
        .execute(SurfaceReadInput::Point {
            identity: &[SavedKey::Int(1)],
        })
        .expect("read persisted surface record");
    assert_eq!(record.fields[0].value, Some(SurfaceValue::Int(7)));
}

/// Rewrite the committed `marrow.lock` so its epoch high-water sits one ahead of the local
/// store's committed epoch, reproducing the state a teammate leaves behind when they commit an
/// activation against the shared source tree that this checkout has not yet caught up to. The
/// store stamp is untouched, so a check still binds the store's own accepted epoch and the
/// per-program fence passes; only the committed lock signals the local store is behind.
fn advance_committed_lock_one_epoch_ahead_of_store(root: &Path) {
    let lock = marrow_check::read_committed_lock(root)
        .expect("read committed lock")
        .expect("committed lock");
    let ahead = marrow_catalog::CatalogLock::new(
        lock.entries,
        lock.ledger,
        lock.epoch_high_water + 1,
        lock.source_digest,
    )
    .expect("ahead lock authority");
    fs::write(
        lock_path(root),
        ahead.to_lock_json_pretty().expect("ahead lock renders"),
    )
    .expect("publish the ahead committed lock");
}

/// The store's durable identity facts a refused open must leave untouched: the minted UID and the
/// commit stamp. A write-capable open touches redb's on-disk header to acquire its handle, but a
/// fenced refusal must commit no Marrow write, so these logical facts are the byte-identity that
/// matters.
fn store_commit_identity(
    store_path: &Path,
) -> (Option<String>, Option<marrow_store::tree::CommitMetadata>) {
    let store = TreeStore::open_read_only(store_path).expect("open store for identity read");
    let uid = store
        .read_store_uid()
        .expect("read store uid")
        .map(|uid| uid.as_str().to_string());
    let commit = store.read_commit_metadata().expect("read commit metadata");
    (uid, commit)
}

/// A fresh active store-root lock entry, cloned from a real committed store entry so it carries a
/// valid shape fingerprint, then re-pathed and re-identified to a `(path, stable_id)` the store
/// does not present — the way a teammate's committed activation would have added a saved root.
fn fresh_active_store_root(
    lock: &marrow_catalog::CatalogLock,
    path: &str,
    id: u64,
) -> marrow_catalog::LockEntry {
    let mut entry = lock
        .entries
        .iter()
        .find(|entry| entry.kind == marrow_catalog::CatalogEntryKind::Store)
        .cloned()
        .expect("the committed lock records a store root to clone");
    entry.path = path.to_string();
    entry.stable_id = format!("cat_{id:032x}");
    entry.aliases = Vec::new();
    entry.lifecycle = marrow_catalog::CatalogLifecycle::Active;
    entry
}

fn seed_surface_counter_store(root: &Path) {
    write_native_config(root);
    write_temp_source(root, Path::new("src/shelf.mw"), surface_counter_source());
    let seed = ProjectSession::open(root, ProjectOpen::run().with_entry_override("shelf::seed"))
        .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);
}

#[test]
fn surface_write_session_fails_closed_when_store_is_behind_the_committed_lock() {
    // A binary must not commit a write to a store it is not admitted against. Run and evolve apply
    // already fail closed when the local store lags an ahead committed lock high-water; the surface
    // write path must mirror them, leaving the store byte-identical rather than committing a write
    // against an epoch a teammate has already advanced past.
    let root = TempDir::new("marrow-run-surface-write-behind-lock").expect("create project");
    seed_surface_counter_store(root.path());
    advance_committed_lock_one_epoch_ahead_of_store(root.path());

    let store_path = root.path().join(".data").join("marrow.redb");
    let before = store_commit_identity(&store_path);

    let error = ProjectSurfaceSession::open(root.path())
        .expect_err("surface write must fail closed when the store is behind the committed lock");
    assert_eq!(error.code(), "run.store_behind");

    assert_eq!(
        before,
        store_commit_identity(&store_path),
        "a refused surface write open must commit no write: no re-stamp, no new commit",
    );
}

#[test]
fn surface_read_session_admits_a_store_behind_the_committed_lock() {
    // A read cannot corrupt the store, so a behind read-only surface is admitted: the lag is a
    // local checkout that has not caught up, and refusing reads would needlessly block inspection
    // of data that is still valid at its own epoch. Only the write path fails closed.
    let root = TempDir::new("marrow-run-surface-read-behind-lock").expect("create project");
    seed_surface_counter_store(root.path());
    advance_committed_lock_one_epoch_ahead_of_store(root.path());

    ProjectSurfaceReadSession::open(root.path())
        .expect("surface read must admit a store behind the committed lock");
}

#[test]
fn surface_write_session_admits_a_store_at_the_committed_lock_high_water() {
    // The fence refuses only a behind store; a store at (or ahead of) the committed lock high-water
    // is legitimately admitted and commits its write.
    let root = TempDir::new("marrow-run-surface-write-at-lock").expect("create project");
    seed_surface_counter_store(root.path());

    let session = ProjectSurfaceSession::open(root.path())
        .expect("surface write must admit a store at the committed lock high-water");
    let before = session.store_stamp().expect("store stamp before update");
    let update_tag = update_operation_tag(session.program(), "Counters");
    session
        .admit_update_by_operation_tag(&update_tag)
        .expect("admit point update")
        .update_point(
            &[SavedKey::Int(1)],
            &[SurfaceUpdateField {
                catalog_id: update_field_catalog_id(session.program(), "Counters", "value"),
                value: SurfaceValue::Int(7),
            }],
        )
        .expect("execute point update");
    let after = session.store_stamp().expect("store stamp after update");
    assert!(
        after.commit_id > before.commit_id,
        "an admitted surface write advances the store commit",
    );
}

#[test]
fn surface_write_session_requires_existing_accepted_native_store_without_creating_it() {
    let root = TempDir::new("marrow-run-surface-write-session-empty").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );

    let error = ProjectSurfaceSession::open(root.path())
        .expect_err("write surface session must not create the first store");

    assert_eq!(error.code(), "run.durable_store_required");
    assert!(
        !root.path().join(".data").exists(),
        "write surface session must not create the configured native data dir"
    );
    assert!(
        !lock_path(root.path()).exists(),
        "write surface session must not freeze accepted catalog identity"
    );
}

#[test]
fn surface_write_session_rejects_populated_unstamped_store_without_minting_uid() {
    let root = TempDir::new("marrow-run-surface-write-session-unstamped").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let store_path = root.path().join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().expect("store parent")).expect("create store dir");
    let store = TreeStore::open(&store_path).expect("open native store");
    store
        .write_data_value(
            &CatalogId::new("cat_00000000000000000000000000000001").expect("store id"),
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(
                CatalogId::new("cat_00000000000000000000000000000002").expect("member id"),
            )],
            b"v".to_vec(),
        )
        .expect("write unstamped data");
    drop(store);

    let error = ProjectSurfaceSession::open(root.path())
        .expect_err("write surface session must not adopt unstamped data");

    assert_eq!(error.code(), "run.store_unstamped");
    let store = TreeStore::open_read_only(&store_path).expect("reopen native store");
    assert!(
        store
            .read_store_uid()
            .expect("read store UID after rejected write surface open")
            .is_none(),
        "write surface session must not mint a UID while rejecting an unstamped store"
    );
    assert!(
        store
            .read_commit_metadata()
            .expect("read commit metadata after rejected write surface open")
            .is_none(),
        "write surface session must not stamp commit metadata while rejecting an unstamped store"
    );
    assert!(
        !lock_path(root.path()).exists(),
        "write surface session must not freeze accepted catalog identity"
    );
}

#[test]
fn surface_read_session_does_not_repair_a_missing_lock() {
    let root =
        TempDir::new("marrow-run-surface-read-session-missing-catalog").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let store_path = root.path().join(".data").join("marrow.redb");
    let lock_path = lock_path(root.path());
    fs::remove_file(&lock_path).expect("remove committed lock");
    let before = {
        let store = TreeStore::open_read_only(&store_path).expect("open seeded store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("seed run stamps the store")
    };

    let session = ProjectSurfaceReadSession::open(root.path())
        .expect("open surface read session from store snapshot");
    assert!(
        !lock_path.exists(),
        "read-only surface session must not re-project a missing lock"
    );
    assert_eq!(
        session
            .store_stamp()
            .expect("read surface session store stamp")
            .commit_id,
        before.commit_id
    );
    drop(session);

    let after = {
        let store = TreeStore::open_read_only(&store_path).expect("reopen seeded store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("surface read keeps the store stamped")
    };
    assert_eq!(before, after, "surface read must not advance commits");
    assert!(
        !lock_path.exists(),
        "read-only surface session must leave the missing lock missing"
    );
}

#[test]
fn surface_read_session_fails_closed_on_a_corrupt_lock() {
    let root =
        TempDir::new("marrow-run-surface-read-session-invalid-catalog").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    // The committed lock is the independent witness to durable identity. A corrupt, unparseable
    // lock is a present durable artifact the operator must delete, not a witness to ignore: a
    // missing lock is the genuine-absence case the store wins over, but a present corrupt lock
    // fails read serving closed exactly as doctor, data stats, backup, and serve --write do. The
    // refused open neither repairs the corrupt lock nor writes the store.
    let lock_path = lock_path(root.path());
    fs::write(&lock_path, "not a valid lock").expect("corrupt the committed lock");
    let store_path = root.path().join(".data").join("marrow.redb");
    let before = store_commit_identity(&store_path);

    let error = ProjectSurfaceReadSession::open(root.path())
        .expect_err("a corrupt committed lock must fail read serving closed");
    assert_eq!(error.code(), "catalog.lock_corrupt");

    assert_eq!(
        fs::read_to_string(&lock_path).expect("read corrupt lock"),
        "not a valid lock",
        "a refused read-only open must not repair the corrupt lock"
    );
    assert_eq!(
        before,
        store_commit_identity(&store_path),
        "a refused read-only open must commit no write"
    );
}

#[test]
fn surface_read_session_requires_an_existing_accepted_native_store() {
    let root = TempDir::new("marrow-run-surface-read-session-empty").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );

    let error = ProjectSurfaceReadSession::open(root.path())
        .expect_err("read-only surface session must not create the first store");

    assert_eq!(error.code(), "run.durable_store_required");
    assert!(
        !root.path().join(".data").exists(),
        "read-only surface session must not create the configured native data dir"
    );
    assert!(
        !lock_path(root.path()).exists(),
        "read-only surface session must not freeze accepted catalog identity"
    );
}

#[test]
fn surface_read_session_rejects_populated_unstamped_store_before_baseline() {
    let root = TempDir::new("marrow-run-surface-read-session-unstamped").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let store_path = root.path().join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().expect("store parent")).expect("create store dir");
    let store = TreeStore::open(&store_path).expect("open native store");
    store
        .write_data_value(
            &CatalogId::new("cat_00000000000000000000000000000001").expect("store id"),
            &[SavedKey::Int(1)],
            &[DataPathSegment::Member(
                CatalogId::new("cat_00000000000000000000000000000002").expect("member id"),
            )],
            b"v".to_vec(),
        )
        .expect("write unstamped data");
    drop(store);

    let error = ProjectSurfaceReadSession::open(root.path())
        .expect_err("read-only surface session must not adopt unstamped data");

    assert_eq!(error.code(), "run.store_unstamped");
    assert!(
        !lock_path(root.path()).exists(),
        "read-only surface session must not freeze accepted catalog identity"
    );
}

#[test]
fn surface_read_session_fences_drift_without_auto_apply() {
    let root = TempDir::new("marrow-run-surface-read-session-drift").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let store_path = root.path().join(".data").join("marrow.redb");
    let lock_path = lock_path(root.path());
    let lock_before = fs::read(&lock_path).expect("seed run projects the committed lock");
    let before = {
        let store = TreeStore::open_read_only(&store_path).expect("open seeded store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("seed run stamps the store")
    };
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        advanced_surface_counter_source(),
    );

    let error =
        ProjectSurfaceReadSession::open(root.path()).expect_err("drift must fence read serving");

    assert_eq!(error.code(), "run.schema_drift");
    let after = {
        let store = TreeStore::open_read_only(&store_path).expect("reopen seeded store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("surface read keeps the store stamped")
    };
    assert_eq!(before, after, "read serving must not auto-apply drift");
    assert_eq!(
        lock_before,
        fs::read(&lock_path).expect("read committed lock after fenced serving open"),
        "read serving must not rewrite the committed lock"
    );
}

/// A fresh native-backed checkout is the exact git-clone shape: `marrow.lock` is committed and
/// records the saved roots, but the gitignored `.data` body is absent. Read-only `serve` must
/// present the empty committed identity the lock determines — zero records — exactly as the
/// read-only inspection family does on the same checkout, and it must never write the store body.
#[test]
fn surface_read_session_serves_empty_committed_identity_on_a_fresh_checkout() {
    let root = TempDir::new("marrow-run-surface-read-fresh-checkout").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        surface_counter_source(),
    );

    // Establish the committed identity (catalog plus a lock recording the ^counter root) with zero
    // records, then delete the store body so only the committed lock survives.
    drop(ProjectSession::open(root.path(), ProjectOpen::run()).expect("establish baseline"));
    let data_dir = root.path().join(".data");
    let store_path = data_dir.join("marrow.redb");
    assert!(store_path.exists(), "baseline run stamps the native store");
    assert!(
        lock_path(root.path()).exists(),
        "baseline run projects the committed lock",
    );
    fs::remove_dir_all(&data_dir).expect("remove the store body for the fresh checkout");
    assert!(!store_path.exists());

    let session = ProjectSurfaceReadSession::open(root.path())
        .expect("read-only serve must serve the empty committed identity on a fresh checkout");

    let page_tag = root_page_operation_tag(session.program(), "Counters");
    let page = session
        .admit_read_by_operation_tag(&page_tag)
        .expect("admit root page")
        .page_read()
        .expect("page read shape")
        .page(SurfaceCollectionPageRequest {
            exact_keys: &[],
            range: None,
            limit: 10,
            cursor: None,
        })
        .expect("read surface page");
    assert!(
        page.rows.is_empty(),
        "the empty committed identity serves zero records",
    );
    drop(session);

    assert!(
        !store_path.exists(),
        "read-only serve must not write the store body on a fresh checkout",
    );
    assert!(
        !data_dir.exists(),
        "read-only serve must not create the data dir",
    );
}

/// A present store that no longer presents a root its committed lock records, at the same epoch as
/// the lock, has lost durable identity: the store-behind carve-out does not apply because the store
/// is caught up to the lock high-water. Read-only `serve` must fail closed `store.corruption`,
/// identically to `serve --write` and the inspection family, rather than misreport the loss as
/// schema drift, and it must commit no write.
#[test]
fn surface_read_session_fails_closed_on_a_store_missing_a_committed_root() {
    let root = TempDir::new("marrow-run-surface-read-lost-root").expect("create project");
    seed_surface_counter_store(root.path());

    let lock = marrow_check::read_committed_lock(root.path())
        .expect("read committed lock")
        .expect("committed lock");
    let mut entries = lock.entries.clone();
    entries.push(fresh_active_store_root(&lock, "shelf::^ghost", 0x6057));
    let lost_root_lock = marrow_catalog::CatalogLock::new(
        entries,
        lock.ledger.clone(),
        lock.epoch_high_water,
        lock.source_digest.clone(),
    )
    .expect("the lost-root lock validates");
    fs::write(
        lock_path(root.path()),
        lost_root_lock.to_lock_json_pretty().expect("render lock"),
    )
    .expect("publish the lost-root committed lock");

    let store_path = root.path().join(".data").join("marrow.redb");
    let before = store_commit_identity(&store_path);

    let error = ProjectSurfaceReadSession::open(root.path())
        .expect_err("read-only serve must fail closed on a store missing a committed root");
    assert_eq!(error.code(), "store.corruption");

    assert_eq!(
        before,
        store_commit_identity(&store_path),
        "a refused read-only open must commit no write",
    );
}

/// A store committed below the lock's epoch high-water that also presents fewer roots is a
/// legitimately-behind local checkout whose missing root is a later activation's addition, not a
/// loss. The lock-root witness honors that carve-out for read-only serve exactly as for the
/// inspection family, so the open is admitted rather than falsely corrupted.
#[test]
fn surface_read_session_admits_an_epoch_behind_store_missing_an_added_root() {
    let root = TempDir::new("marrow-run-surface-read-behind-added-root").expect("create project");
    seed_surface_counter_store(root.path());

    let lock = marrow_check::read_committed_lock(root.path())
        .expect("read committed lock")
        .expect("committed lock");
    let mut entries = lock.entries.clone();
    entries.push(fresh_active_store_root(&lock, "shelf::^ghost", 0x6058));
    let ahead_lock = marrow_catalog::CatalogLock::new(
        entries,
        lock.ledger.clone(),
        lock.epoch_high_water + 1,
        lock.source_digest.clone(),
    )
    .expect("the ahead lock validates");
    fs::write(
        lock_path(root.path()),
        ahead_lock.to_lock_json_pretty().expect("render lock"),
    )
    .expect("publish the ahead committed lock");

    ProjectSurfaceReadSession::open(root.path())
        .expect("read-only serve must admit a legitimately epoch-behind store");
}

#[test]
fn project_session_invokes_protocol_arguments() {
    let root = TempDir::new("marrow-run-session-protocol-args").expect("create project");
    write_native_config_no_default(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\
         pub fn echo(label: string, n: int): int\n\
         \x20\x20\x20\x20print(label)\n\
         \x20\x20\x20\x20return n\n",
    );
    let session = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_entry_override("shelf::echo")
            .with_fresh_memory_store(),
    )
    .expect("open session");
    let host = Host::new();
    let mut output = String::new();

    let descriptor = EntryDescriptor::resolve(session.runtime_program(), "shelf::echo")
        .expect("entry descriptor");
    let result = session
        .invoke(SessionEntry::protocol(
            EntryInvocation {
                identity: descriptor.identity,
                arguments: vec![
                    EntryArgument {
                        name: "label".into(),
                        value: EntryArgumentValue::Scalar(EntryScalarArgument::String(
                            "typed".into(),
                        )),
                    },
                    EntryArgument {
                        name: "n".into(),
                        value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(7)),
                    },
                ],
            },
            &host,
            &mut output,
        ))
        .expect("protocol args invoke");

    assert_eq!(output, "typed\n");
    assert_eq!(result.value, Some(marrow_run::Value::Int(7)));
}

#[test]
fn project_session_rejects_stale_protocol_invocation_identity() {
    let stale = support::checked_program("module shelf\npub fn echo(n: int): int\n    return n\n");
    let stale = EntryDescriptor::resolve(&stale, "shelf::echo").expect("stale descriptor");
    let stale = EntryInvocation {
        identity: stale.identity,
        arguments: vec![EntryArgument {
            name: "n".into(),
            value: EntryArgumentValue::Scalar(EntryScalarArgument::Int(7)),
        }],
    };
    let root = TempDir::new("marrow-run-session-stale-protocol-args").expect("create project");
    write_native_config_no_default(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\
         pub fn echo(label: string): string\n\
         \x20\x20\x20\x20return label\n",
    );
    let session = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_entry_override("shelf::echo")
            .with_fresh_memory_store(),
    )
    .expect("open session");
    let host = Host::new();
    let mut output = String::new();

    let error = session
        .invoke(SessionEntry::protocol(stale, &host, &mut output))
        .expect_err("stale protocol descriptor should fail closed");

    assert_eq!(error.code(), RUN_ENTRY_ARGUMENT);
    assert_eq!(output, "");
}

#[test]
fn fresh_memory_run_does_not_create_native_store_or_catalog_artifact() {
    let root = TempDir::new("marrow-run-session-fresh-memory").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let session = ProjectSession::open(root.path(), ProjectOpen::run().with_fresh_memory_store())
        .expect("open fresh-memory session");
    assert_eq!(session.run_entry(), Some("shelf::show"));
    assert!(
        session.store_stamp().expect("read store stamp").is_none(),
        "fresh-memory sessions do not expose durable store stamps"
    );

    let output = invoke(&session, "shelf::show");

    assert_eq!(output, "baseline\n");
    assert!(
        !root.path().join(".data").exists(),
        "fresh-memory session must not create the configured native data dir"
    );
    assert!(
        !lock_path(root.path()).exists(),
        "fresh-memory session must not project the committed lock"
    );
}

#[test]
fn fresh_memory_run_execution_boundary_reports_explicit_fresh_memory() {
    let root = TempDir::new("marrow-run-session-fresh-memory-boundary").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let session = ProjectSession::open(root.path(), ProjectOpen::run().with_fresh_memory_store())
        .expect("open fresh-memory session");
    let boundary = session.execution_boundary();

    assert_eq!(boundary.session_kind, ExecutionSessionKind::Run);
    assert_eq!(
        boundary.source_analysis_generation,
        session.source_analysis_snapshot().generation()
    );
    assert_eq!(boundary.store.kind, ExecutionBoundaryStoreKind::FreshMemory);
    assert!(boundary.store.stamp.is_none());
}

#[test]
fn plain_memory_run_execution_boundary_is_not_fresh_memory() {
    let root = TempDir::new("marrow-run-session-plain-memory-boundary").expect("create project");
    fs::write(
        root.path().join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "shelf::show" } }"#,
    )
    .expect("write marrow.json");
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\npub fn show()\n    print(\"memory\")\n",
    );

    let session = ProjectSession::open(root.path(), ProjectMode::Run).expect("open session");
    let boundary = session.execution_boundary();

    assert_eq!(boundary.store.kind, ExecutionBoundaryStoreKind::PlainMemory);
    assert!(boundary.store.stamp.is_none());
}

#[test]
fn isolated_policy_over_memory_reports_plain_memory_boundary() {
    let root = TempDir::new("marrow-run-session-isolated-memory-boundary").expect("create project");
    fs::write(
        root.path().join("marrow.json"),
        r#"{ "sourceRoots": ["src"], "store": { "backend": "memory" }, "run": { "defaultEntry": "shelf::show" } }"#,
    )
    .expect("write marrow.json");
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\npub fn show()\n    print(\"memory\")\n",
    );

    let session = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open isolated memory session");
    let boundary = session.execution_boundary();

    assert_eq!(
        boundary.store.kind,
        ExecutionBoundaryStoreKind::PlainMemory,
        "isolated policy over a memory store must not claim an isolated native boundary"
    );
    assert!(boundary.store.stamp.is_none());
}

#[test]
fn run_session_source_analysis_identity_changes_for_body_edits() {
    let root = TempDir::new("marrow-run-session-analysis-identity").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\npub fn show()\n    print(\"first\")\n",
    );

    let first = ProjectSession::open(root.path(), ProjectOpen::run().with_fresh_memory_store())
        .expect("open first session");
    let first_identity = first.source_analysis_identity().clone();
    let first_entry =
        EntryDescriptor::resolve(first.runtime_program(), "shelf::show").expect("first entry");

    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\npub fn show()\n    print(\"second\")\n",
    );

    let second = ProjectSession::open(root.path(), ProjectOpen::run().with_fresh_memory_store())
        .expect("open second session");
    let second_entry =
        EntryDescriptor::resolve(second.runtime_program(), "shelf::show").expect("second entry");

    assert_ne!(first_identity, *second.source_analysis_identity());
    assert_eq!(
        first_entry.identity.entry_tag,
        second_entry.identity.entry_tag
    );
}

#[test]
fn run_session_exposes_the_source_analysis_snapshot_used_by_its_runtime_program() {
    let root = TempDir::new("marrow-run-session-analysis-snapshot").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let session = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open session");
    let snapshot = session.source_analysis_snapshot();

    assert_eq!(
        snapshot.content_identity(),
        session.source_analysis_identity()
    );
    assert_eq!(
        snapshot.program.source_digest(),
        session.runtime_program().source_digest()
    );
    assert_eq!(
        snapshot.program.read_only_context_digest(),
        session.runtime_program().read_only_context_digest()
    );
}

#[test]
fn isolated_run_execution_boundary_reports_generation_and_store_stamp() {
    let root = TempDir::new("marrow-run-session-isolated-boundary").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        persistent_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::bump"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::bump"), "");
    drop(seed);

    let session = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open isolated session");
    let boundary = session.execution_boundary();

    assert_eq!(boundary.session_kind, ExecutionSessionKind::Run);
    assert_eq!(
        boundary.source_analysis_generation,
        session.source_analysis_snapshot().generation()
    );
    assert_eq!(boundary.store.kind, ExecutionBoundaryStoreKind::Isolated);
    let stamp = boundary
        .store
        .stamp
        .expect("isolated native boundary exposes the opened store stamp");
    assert!(stamp.store_uid.starts_with("store_"));
    assert_eq!(stamp.catalog_epoch, 1);
    assert!(stamp.commit_id > 0);
}

#[test]
fn isolated_native_execution_boundary_does_not_reopen_store_path() {
    let root =
        TempDir::new("marrow-run-session-isolated-boundary-captured").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        persistent_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::bump"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::bump"), "");
    let seed_stamp = seed
        .store_stamp()
        .expect("read seed store stamp")
        .expect("seed stamps the native store");
    drop(seed);

    let session = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open isolated session");
    fs::remove_dir_all(root.path().join(".data")).expect("remove original native store path");

    let boundary = session.execution_boundary();

    assert_eq!(boundary.store.kind, ExecutionBoundaryStoreKind::Isolated);
    assert_eq!(boundary.store.stamp, Some(seed_stamp));
}

#[test]
fn native_commit_run_execution_boundary_reports_opened_store_stamp() {
    let root = TempDir::new("marrow-run-session-native-boundary").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        persistent_counter_source(),
    );

    let session = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::bump"),
    )
    .expect("open native commit session");
    let opened_stamp = session
        .store_stamp()
        .expect("read opened store stamp")
        .expect("native commit session exposes the opened store stamp");
    let boundary = session.execution_boundary();

    assert_eq!(boundary.session_kind, ExecutionSessionKind::Run);
    assert_eq!(
        boundary.store.kind,
        ExecutionBoundaryStoreKind::NativeCommit
    );
    let stamp = boundary
        .store
        .stamp
        .expect("native commit boundary exposes the committed store stamp");
    assert!(stamp.store_uid.starts_with("store_"));
    assert_eq!(stamp.catalog_epoch, 1);
    assert_eq!(stamp, opened_stamp);
}

#[test]
fn native_commit_execution_boundary_keeps_open_stamp_after_invocation() {
    let root = TempDir::new("marrow-run-session-native-boundary-captured").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\
         resource Counter\n\
         \x20\x20\x20\x20required value: int\n\
         store ^counter(id: int): Counter\n\
         pub fn seed()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^counter(1).value = 1\n\
         pub fn bump()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^counter(1).value = 2\n\
         pub fn show()\n\
         \x20\x20\x20\x20print(\"ok\")\n",
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let session = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::bump"),
    )
    .expect("open native commit session");
    let opened_boundary = session.execution_boundary();
    let opened_stamp = opened_boundary
        .store
        .stamp
        .clone()
        .expect("native boundary has an opened stamp");

    assert_eq!(invoke(&session, "shelf::bump"), "");
    let live_stamp = session
        .store_stamp()
        .expect("read live store stamp")
        .expect("native store remains stamped");
    assert!(
        live_stamp.commit_id > opened_stamp.commit_id,
        "the write should advance the live store commit"
    );

    let after_invoke = session.execution_boundary();

    assert_eq!(
        after_invoke.store.kind,
        ExecutionBoundaryStoreKind::NativeCommit
    );
    assert_eq!(
        after_invoke.store.stamp,
        Some(opened_stamp),
        "execution boundary must describe the opened session, not the live post-invocation store"
    );
}

#[test]
fn isolated_native_session_invokes_against_opened_store_snapshot() {
    let root =
        TempDir::new("marrow-run-session-isolated-boundary-pins-store").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        mutable_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::setOne"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::setOne"), "");
    let seed_stamp = seed
        .store_stamp()
        .expect("read seed store stamp")
        .expect("seed stamps the native store");
    drop(seed);

    let isolated = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_isolated_writes()
            .with_entry_override("shelf::show"),
    )
    .expect("open isolated session");
    assert_eq!(isolated.execution_boundary().store.stamp, Some(seed_stamp));

    let advance = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::setTwo"),
    )
    .expect("open advancing session");
    assert_eq!(invoke(&advance, "shelf::setTwo"), "");
    let advanced_stamp = advance
        .store_stamp()
        .expect("read advanced store stamp")
        .expect("advance stamps the native store");
    assert!(
        advanced_stamp.commit_id > isolated.execution_boundary().store.stamp.unwrap().commit_id,
        "advancing session should move the real store past the isolated boundary"
    );
    drop(advance);

    assert_eq!(invoke(&isolated, "shelf::show"), "value=1\n");
}

#[test]
fn native_commit_session_entry_isolated_writes_are_invocation_scoped() {
    let root = TempDir::new("marrow-run-session-entry-isolated-writes").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        mutable_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::setOne"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::setOne"), "");
    drop(seed);

    let session = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::show"),
    )
    .expect("open native commit session");
    let opened_boundary = session.execution_boundary();
    assert_eq!(
        opened_boundary.store.kind,
        ExecutionBoundaryStoreKind::NativeCommit
    );
    let opened_stamp = opened_boundary
        .store
        .stamp
        .expect("native commit boundary has a stamp");

    assert_eq!(invoke(&session, "shelf::setTwo"), "");
    let committed_stamp = session
        .store_stamp()
        .expect("read committed store stamp")
        .expect("native commit session remains stamped");
    assert!(committed_stamp.commit_id > opened_stamp.commit_id);

    let host = Host::new();
    let mut isolated_output = String::new();
    session
        .invoke(
            SessionEntry::new("shelf::setThree", &host, &mut isolated_output)
                .with_isolated_writes(),
        )
        .expect("invoke entry with isolated writes");
    assert_eq!(isolated_output, "");

    assert_eq!(invoke(&session, "shelf::show"), "value=2\n");
    assert_eq!(
        session.execution_boundary().store.stamp,
        Some(opened_stamp),
        "per-invocation isolated writes do not change the native-commit session boundary"
    );
}

#[test]
fn isolated_run_reuses_source_analysis_admission() {
    let root = TempDir::new("marrow-run-session-analysis-admission").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let launch = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open launch session");
    let admission: marrow_run::SourceAnalysisAdmission = launch
        .source_analysis_admission()
        .expect("build admission")
        .expect("durable run session has source admission");
    let reopened = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_isolated_writes()
            .with_source_analysis_admission(admission),
    )
    .expect("open admitted session");

    assert_eq!(
        reopened
            .source_analysis_snapshot()
            .program
            .read_only_context_digest(),
        launch
            .source_analysis_snapshot()
            .program
            .read_only_context_digest()
    );
    assert_eq!(
        reopened.runtime_program().read_only_context_digest(),
        launch.runtime_program().read_only_context_digest()
    );
}

#[test]
fn source_analysis_admission_rejects_source_changes() {
    let root =
        TempDir::new("marrow-run-session-analysis-admission-change").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let launch = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open launch session");
    let admission = launch
        .source_analysis_admission()
        .expect("build admission")
        .expect("durable run session has source admission");

    write_temp_source(root.path(), Path::new("src/shelf.mw"), advanced_source());

    let error = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_isolated_writes()
            .with_source_analysis_admission(admission),
    )
    .expect_err("admission belongs to the original source analysis");

    assert_eq!(error.code(), "run.schema_drift");
}

#[test]
fn source_analysis_admission_rejects_changed_store_authority() {
    let root = TempDir::new("marrow-run-session-analysis-admission-store").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let launch = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open launch session");
    let admission = launch
        .source_analysis_admission()
        .expect("build admission")
        .expect("durable run session has source admission");
    drop(launch);

    let baseline = ProjectSession::open(root.path(), ProjectMode::Run).expect("open baseline");
    assert_eq!(invoke(&baseline, "shelf::show"), "baseline\n");
    drop(baseline);

    let error = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_isolated_writes()
            .with_source_analysis_admission(admission),
    )
    .expect_err("admission must not override store-bound accepted identity");

    assert_eq!(error.code(), "run.schema_drift");
}

#[test]
fn source_analysis_admission_rejects_changed_lock_authority_without_store() {
    let root = TempDir::new("marrow-run-session-analysis-admission-lock").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let launch = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open launch session");
    let admission = launch
        .source_analysis_admission()
        .expect("build admission")
        .expect("durable run session has source admission");
    drop(launch);

    let baseline = ProjectSession::open(root.path(), ProjectMode::Run).expect("open baseline");
    assert_eq!(invoke(&baseline, "shelf::show"), "baseline\n");
    drop(baseline);
    fs::remove_dir_all(root.path().join(".data")).expect("remove native store");

    let error = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_isolated_writes()
            .with_source_analysis_admission(admission),
    )
    .expect_err("admission must not override committed lock identity");

    assert_eq!(error.code(), "run.schema_drift");
}

#[test]
fn source_analysis_admission_rejects_commit_mode_without_writes() {
    let root =
        TempDir::new("marrow-run-session-analysis-admission-commit").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let launch = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open launch session");
    let admission = launch
        .source_analysis_admission()
        .expect("build admission")
        .expect("durable run session has source admission");
    drop(launch);

    let error = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_source_analysis_admission(admission),
    )
    .expect_err("admission is not a write-mode authority");

    assert_eq!(error.code(), "run.schema_drift");
    assert!(
        !root.path().join(".data").exists(),
        "commit-mode rejection must not create the native store"
    );
    assert!(
        !lock_path(root.path()).exists(),
        "commit-mode rejection must not project a committed lock"
    );
}

#[test]
fn source_analysis_admission_preserves_unstamped_store_guard() {
    let root =
        TempDir::new("marrow-run-session-analysis-admission-unstamped").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let launch = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open launch session");
    let admission = launch
        .source_analysis_admission()
        .expect("build admission")
        .expect("durable run session has source admission");
    drop(launch);

    let store_path = root.path().join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().expect("store parent")).expect("create store dir");
    let store = TreeStore::open(&store_path).expect("open native store");
    store
        .write_record_presence(
            &CatalogId::new("cat_00000000000000000000000000000001").expect("store id"),
            &[SavedKey::Int(1)],
        )
        .expect("write unstamped record");
    drop(store);

    let error = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_isolated_writes()
            .with_source_analysis_admission(admission),
    )
    .expect_err("admission must not bypass unstamped store rejection");

    assert_eq!(error.code(), "run.store_unstamped");
}

#[test]
fn fresh_memory_run_reuses_source_analysis_admission() {
    let root = TempDir::new("marrow-run-session-analysis-admission-fresh").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let launch = ProjectSession::open(root.path(), ProjectOpen::run().with_isolated_writes())
        .expect("open launch session");
    let admission = launch
        .source_analysis_admission()
        .expect("build admission")
        .expect("durable run session has source admission");
    let fresh = ProjectSession::open(
        root.path(),
        ProjectOpen::run()
            .with_fresh_memory_store()
            .with_source_analysis_admission(admission),
    )
    .expect("open admitted fresh-memory session");

    assert_eq!(
        fresh.runtime_program().read_only_context_digest(),
        launch.runtime_program().read_only_context_digest()
    );
}

#[test]
fn test_session_keeps_source_analysis_snapshot_separate_from_test_program() {
    let root = TempDir::new("marrow-run-session-test-source-snapshot").expect("create project");
    write_memory_config_with_tests(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\npub fn helper(): int\n    return 1\n",
    );
    write_temp_source(
        root.path(),
        Path::new("tests/smoke_test.mw"),
        "pub fn smoke()\n    std::assert::isTrue(shelf::helper() == 1)\n",
    );

    let session = ProjectSession::open(root.path(), ProjectOpen::test()).expect("open tests");
    let source_modules: Vec<&str> = session
        .source_analysis_snapshot()
        .program
        .modules
        .iter()
        .map(|module| module.name.as_str())
        .collect();
    let session_modules: Vec<&str> = session
        .program()
        .modules
        .iter()
        .map(|module| module.name.as_str())
        .collect();

    assert_eq!(source_modules, ["shelf"]);
    assert!(session_modules.contains(&"tests::smoke_test"));
    assert_eq!(session.test_cases()[0].name, "tests::smoke_test::smoke");
}

#[test]
fn test_session_execution_boundary_reports_test_memory() {
    let root = TempDir::new("marrow-run-session-test-boundary").expect("create project");
    write_memory_config_with_tests(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\npub fn helper(): int\n    return 1\n",
    );
    write_temp_source(
        root.path(),
        Path::new("tests/smoke_test.mw"),
        "pub fn smoke()\n    std::assert::isTrue(shelf::helper() == 1)\n",
    );

    let session = ProjectSession::open(root.path(), ProjectOpen::test()).expect("open tests");
    let boundary = session.execution_boundary();

    assert_eq!(boundary.session_kind, ExecutionSessionKind::Test);
    assert_eq!(
        boundary.source_analysis_generation,
        session.source_analysis_snapshot().generation()
    );
    assert_eq!(boundary.store.kind, ExecutionBoundaryStoreKind::TestMemory);
    assert!(boundary.store.stamp.is_none());
}

#[cfg(unix)]
#[test]
fn test_session_does_not_read_an_existing_native_store() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new("marrow-run-session-test-existing-store").expect("create project");
    write_native_config_with_tests(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        "module shelf\n\
         resource Counter\n\
         \x20\x20\x20\x20required value: int\n\
         store ^counter(id: int): Counter\n\
         pub fn seed()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20    ^counter(1).value = 1\n",
    );
    write_temp_source(
        root.path(),
        Path::new("tests/smoke_test.mw"),
        "pub fn smoke()\n    std::assert::isTrue(true)\n",
    );
    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::seed"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::seed"), "");
    drop(seed);

    let store_path = root.path().join(".data").join("marrow.redb");
    let mut unreadable = fs::metadata(&store_path)
        .expect("seed run creates a native store")
        .permissions();
    unreadable.set_mode(0o0);
    fs::set_permissions(&store_path, unreadable).expect("make native store unreadable");

    let session = ProjectSession::open(root.path(), ProjectOpen::test())
        .expect("test sessions run over fresh memory without inspecting the native store");
    assert_eq!(session.test_cases()[0].name, "tests::smoke_test::smoke");

    let host = Host::new();
    let mut output = String::new();
    session
        .invoke(SessionEntry::new(
            &session.test_cases()[0].name,
            &host,
            &mut output,
        ))
        .expect("invoke discovered test over a fresh store");
}

#[test]
fn native_run_source_analysis_identity_matches_baseline_recheck() {
    let root = TempDir::new("marrow-run-session-baseline-identity").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());

    let session = ProjectSession::open(root.path(), ProjectMode::Run).expect("open session");

    assert_eq!(
        *session.source_analysis_identity(),
        checked_source_identity(root.path())
    );
    assert_eq!(invoke(&session, "shelf::show"), "baseline\n");
}

#[test]
fn auto_apply_run_source_analysis_identity_matches_recheck() {
    let root = TempDir::new("marrow-run-session-auto-identity").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());
    let baseline = ProjectSession::open(root.path(), ProjectMode::Run).expect("open baseline");
    assert_eq!(invoke(&baseline, "shelf::show"), "baseline\n");
    drop(baseline);

    write_temp_source(root.path(), Path::new("src/shelf.mw"), advanced_source());

    let advanced = ProjectSession::open(root.path(), ProjectMode::Run).expect("open advanced");

    assert!(
        advanced
            .notices()
            .iter()
            .any(|notice| matches!(notice, marrow_run::ProjectSessionNotice::AutoApplied { .. })),
        "advanced session should auto-apply zero-mutation schema drift"
    );
    assert_eq!(
        *advanced.source_analysis_identity(),
        checked_source_identity(root.path())
    );
    assert_eq!(invoke(&advanced, "shelf::show"), "advanced\n");
}

#[test]
fn fresh_memory_run_does_not_read_or_advance_an_existing_native_store() {
    let root =
        TempDir::new("marrow-run-session-fresh-memory-existing-store").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        persistent_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::bump"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::bump"), "");
    drop(seed);

    let store_path = root.path().join(".data").join("marrow.redb");
    let lock_path = lock_path(root.path());
    let lock_before = fs::read(&lock_path).expect("seed run projects the committed lock");
    let before = {
        let store = TreeStore::open_read_only(&store_path).expect("open real store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("seed run stamps the real store")
    };

    let fresh = ProjectSession::open(root.path(), ProjectOpen::run().with_fresh_memory_store())
        .expect("open fresh-memory session over existing native store");
    assert_eq!(
        invoke(&fresh, "shelf::show"),
        "absent\n",
        "fresh-memory run must not read the real native store"
    );
    assert_eq!(
        invoke(&fresh, "shelf::bump"),
        "",
        "fresh-memory writes must stay inside the in-memory session store"
    );

    let after = {
        let store = TreeStore::open_read_only(&store_path).expect("reopen real store");
        store
            .read_commit_metadata()
            .expect("read commit metadata")
            .expect("real store remains stamped")
    };
    assert_eq!(
        before, after,
        "fresh-memory run must not advance the real store commit"
    );
    assert_eq!(
        lock_before,
        fs::read(&lock_path).expect("fresh-memory run preserves the committed lock"),
        "fresh-memory run must not rewrite the committed lock"
    );
}

#[test]
fn opening_a_store_whose_snapshot_outran_its_commit_stamp_fails_closed_as_corruption() {
    let root = TempDir::new("marrow-run-session-inconsistent").expect("create project");
    write_native_config(root.path());
    write_temp_source(root.path(), Path::new("src/shelf.mw"), baseline_source());
    let config = native_config();
    let store_path = root.path().join(".data").join("marrow.redb");
    fs::create_dir_all(store_path.parent().expect("store parent")).expect("create store dir");

    let baseline = {
        let (report, pending) =
            marrow_check::check_project_with_catalog(root.path(), &config, None)
                .expect("check baseline");
        assert!(!report.has_errors(), "{:#?}", report.diagnostics);
        let store = marrow_store::tree::TreeStore::open(&store_path).expect("open native store");
        assert!(
            marrow_run::evolution::commit_catalog_baseline(&store, &pending)
                .expect("commit baseline"),
            "baseline fixture should commit a first catalog"
        );
        store
            .read_catalog_snapshot()
            .expect("read baseline catalog")
            .expect("baseline catalog")
    };

    // The commit stamp and the accepted catalog snapshot both name one accepted epoch and are
    // stamped together in a single transaction, so no production write leaves them disagreeing.
    // Publishing an advanced snapshot without re-stamping the commit fabricates that impossible
    // state: the store now claims two different accepted epochs at once. That internal
    // contradiction is backend corruption, not a legitimately-behind checkout, so the store-open
    // readability cross-check fails it closed with `store.corruption`.
    write_temp_source(root.path(), Path::new("src/shelf.mw"), advanced_source());
    let (report, advanced) =
        marrow_check::check_project_with_catalog(root.path(), &config, Some(&baseline))
            .expect("check advanced source");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let advanced_catalog = advanced
        .catalog
        .proposal
        .expect("advanced source proposes the next catalog");
    assert!(
        advanced_catalog.epoch > baseline.epoch,
        "advanced source advances the catalog epoch"
    );
    {
        let store = marrow_store::tree::TreeStore::open(&store_path).expect("reopen native store");
        store.begin().expect("begin");
        store
            .replace_catalog_snapshot(&advanced_catalog)
            .expect("publish the advanced catalog snapshot without re-stamping");
        store.commit().expect("commit");
    }

    let error = ProjectSession::open(root.path(), ProjectMode::Run)
        .expect_err("an inconsistent store fails closed");

    assert_eq!(error.code(), "store.corruption");
}

#[test]
fn run_over_a_store_behind_the_committed_lock_high_water_is_fenced() {
    // The legitimate store-behind: a locally-consistent store (its commit stamp and catalog
    // snapshot agree) whose committed lock a teammate advanced one epoch past the local commit.
    // The store is not corrupt, so the cross-check admits it; the lock high-water is the
    // independent witness that a write here would commit against an epoch the shared source tree
    // has left behind, so run fails closed with `run.store_behind`.
    let root = TempDir::new("marrow-run-behind-committed-lock").expect("create project");
    write_native_config(root.path());
    write_temp_source(
        root.path(),
        Path::new("src/shelf.mw"),
        persistent_counter_source(),
    );

    let seed = ProjectSession::open(
        root.path(),
        ProjectOpen::run().with_entry_override("shelf::bump"),
    )
    .expect("open seed session");
    assert_eq!(invoke(&seed, "shelf::bump"), "");
    drop(seed);

    advance_committed_lock_one_epoch_ahead_of_store(root.path());

    let error = ProjectSession::open(root.path(), ProjectMode::Run)
        .expect_err("a store behind the committed lock high-water is fenced");

    assert_eq!(error.code(), "run.store_behind");
}

fn surface_id(program: &marrow_check::CheckedProgram, name: &str) -> SurfaceId {
    program
        .facts
        .surfaces()
        .iter()
        .find(|surface| surface.name == name)
        .unwrap_or_else(|| panic!("surface `{name}` is present in checked facts"))
        .id
}

fn point_read_operation_tag(program: &marrow_check::CheckedProgram, surface: &str) -> String {
    operation_tag(program, surface, |kind| {
        matches!(
            kind,
            SurfaceReadOperationKind::SingletonRead { .. }
                | SurfaceReadOperationKind::PointRead { .. }
        )
    })
}

fn root_page_operation_tag(program: &marrow_check::CheckedProgram, surface: &str) -> String {
    operation_tag(program, surface, |kind| {
        matches!(kind, SurfaceReadOperationKind::PagedRootCollection { .. })
    })
}

fn update_operation_tag(program: &marrow_check::CheckedProgram, surface: &str) -> String {
    let surface = program.facts.surface(surface_id(program, surface));
    SurfaceUpdateOperationDescriptor::from_surface(program, surface)
        .map(|descriptor| descriptor.operation_tag)
        .expect("stable surface update operation tag")
}

fn update_field_catalog_id(
    program: &marrow_check::CheckedProgram,
    surface: &str,
    field: &str,
) -> CatalogId {
    let surface_fact = program.facts.surface(surface_id(program, surface));
    SurfaceUpdateOperationDescriptor::from_surface(program, surface_fact)
        .and_then(|descriptor| {
            descriptor
                .fields
                .into_iter()
                .find(|candidate| candidate.render_label == field)
                .map(|candidate| candidate.member_catalog_id)
        })
        .unwrap_or_else(|| panic!("surface `{surface}` exposes update field `{field}`"))
}

fn operation_tag(
    program: &marrow_check::CheckedProgram,
    surface: &str,
    matches_kind: impl Fn(&SurfaceReadOperationKind) -> bool,
) -> String {
    let surface = program.facts.surface(surface_id(program, surface));
    surface
        .read_operations
        .iter()
        .find(|operation| matches_kind(&operation.kind))
        .and_then(|operation| operation.operation_tag.clone())
        .expect("stable surface operation tag")
}
