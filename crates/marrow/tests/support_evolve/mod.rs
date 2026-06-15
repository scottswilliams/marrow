use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{CheckedProgram, CheckedSavedPlace, ProjectConfig, check_project};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, StoreUid, TreeStore};
use marrow_store::value::{Scalar, ScalarType, decode_value, encode_value};

use crate::support::{TempProject, temp_project_uncommitted as temp_project, write};

// The saved-place fact lookups are owned by marrow-check behind its `test-support`
// feature, so the CLI evolution suites resolve a member or store catalog id through the
// same helpers the discharge and apply suites do.
pub(crate) use marrow_check::test_support::{
    member_catalog_id, root_place, store_id_of as store_catalog_id,
};
fn config(root: impl AsRef<Path>) -> ProjectConfig {
    let text = fs::read_to_string(root.as_ref().join("marrow.json")).expect("read config");
    marrow_project::parse_config(&text).expect("parse config")
}

/// Freeze a project's baseline durable identity into its store, the way a
/// state-establishing run does, and return the program re-bound against the accepted
/// store snapshot that mirrors the committed catalog artifact.
pub(crate) fn commit_catalog(root: impl AsRef<Path>) -> CheckedProgram {
    let root = root.as_ref();
    let config = config(root);
    let (report, program) = check_project(root, &config).expect("check for proposal");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let store = open_native_store(root);
    store
        .write_store_uid(
            &StoreUid::new("store_00000000000000000000000000000001".to_string())
                .expect("valid fixture store uid"),
        )
        .expect("write fixture store uid");
    marrow_run::evolution::commit_catalog_baseline(&store, &program)
        .expect("commit catalog baseline");
    if let Some(snapshot) = store.read_catalog_snapshot().expect("read store catalog") {
        fs::write(
            root.join("marrow.catalog.json"),
            snapshot.to_json_pretty().expect("catalog renders"),
        )
        .expect("render catalog file");
    }

    let accepted = store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot");
    let (report, program) =
        marrow_check::check_project_with_catalog(root, &config, accepted.as_ref())
            .expect("re-check against accepted catalog");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}
pub(crate) fn native_store_path(root: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(".data").join("marrow.redb")
}
pub(crate) fn open_native_store(root: impl AsRef<Path>) -> TreeStore {
    let path = native_store_path(root);
    fs::create_dir_all(path.parent().unwrap()).expect("create data dir");
    TreeStore::open(&path).expect("open native store")
}
pub(crate) fn seed_record(store: &TreeStore, place: &CheckedSavedPlace, id: i64) {
    let store_id = store_catalog_id(place).expect("store catalog id");
    write_record_node(store, &store_id, &[SavedKey::Int(id)]);
}
fn write_record_node(store: &TreeStore, store_id: &CatalogId, identity: &[SavedKey]) {
    store.write_node(store_id, identity).expect("write record");
}
pub(crate) fn seed_record_member_value(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    identity: &[SavedKey],
    member: &str,
    value: Scalar,
) {
    let store_id = store_catalog_id(place).expect("store catalog id");
    write_record_node(store, &store_id, identity);
    let member_id = CatalogId::new(member_catalog_id(place, member).expect("member catalog id"))
        .expect("member id");
    store
        .write_data_value(
            &store_id,
            identity,
            &[DataPathSegment::Member(member_id)],
            encode_value(&value).expect("encode member"),
        )
        .expect("write member");
}
pub(crate) fn seed_title_only(store: &TreeStore, place: &CheckedSavedPlace, id: i64, title: &str) {
    seed_record_member_value(
        store,
        place,
        &[SavedKey::Int(id)],
        "title",
        Scalar::Str(title.to_string()),
    );
}
pub(crate) fn seed_member(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member: &str,
    value: Scalar,
) {
    let store_id = store_catalog_id(place).expect("store catalog id");
    let member_id = CatalogId::new(member_catalog_id(place, member).expect("member catalog id"))
        .expect("member id");
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member_id)],
            encode_value(&value).expect("encode member"),
        )
        .expect("write member");
}
pub(crate) fn read_scalar(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member: &str,
    ty: ScalarType,
) -> Option<Scalar> {
    let store_id = store_catalog_id(place).expect("store catalog id");
    let member_id = CatalogId::new(member_catalog_id(place, member).expect("member catalog id"))
        .expect("member id");
    store
        .read_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member_id)],
        )
        .expect("read member")
        .map(|bytes| decode_value(&bytes, ty).expect("decode value"))
}
pub(crate) fn read_scalar_by_catalog_id(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member_id: &str,
    ty: ScalarType,
) -> Option<Scalar> {
    let store_id = store_catalog_id(place).expect("store catalog id");
    store
        .read_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(
                CatalogId::new(member_id.to_string()).unwrap(),
            )],
        )
        .expect("read member")
        .map(|bytes| decode_value(&bytes, ty).expect("decode value"))
}
pub(crate) fn native_books_project(name: &str, source: &str) -> TempProject {
    temp_project(name, |root| {
        write(root, "marrow.json", crate::support::native_config());
        write(root, "src/books.mw", source);
    })
}

/// The accepted catalog snapshot a project's store holds as the crash bridge. Reading
/// it here is the typed oracle for tests that inspect the committed durable identity.
pub(crate) fn accepted_catalog(root: impl AsRef<Path>) -> marrow_catalog::CatalogMetadata {
    let store = TreeStore::open_read_only(&native_store_path(root)).expect("open store read-only");
    store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("project has an accepted catalog")
}
pub(crate) fn accepted_catalog_entry_id(root: impl AsRef<Path>, path: &str) -> String {
    accepted_catalog(root)
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("accepted catalog entry `{path}`"))
        .stable_id
}
pub(crate) fn store_epoch(root: impl AsRef<Path>) -> Option<u64> {
    let store = TreeStore::open_read_only(&native_store_path(root)).expect("reopen native store");
    store
        .read_commit_metadata()
        .expect("read store commit")
        .map(|commit| commit.catalog_epoch)
}

// The before/after `module books` evolution sources live in the repo-root corpus, so
// the same fixture is not re-declared as an inline string here and in the runtime
// crate. The baseline/default/subtitle/retire shapes are shared with marrow-run.
pub(crate) const REQUIRED_DEFAULT_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_required_default.mw"
));
pub(crate) const REQUIRED_NO_DEFAULT_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_required_no_default.mw"
));
pub(crate) const REQUIRED_BASELINE_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_required_baseline.mw"
));
pub(crate) const OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_optional_pages_default_index.mw"
));
pub(crate) const RETIRE_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_retire_subtitle.mw"
));
pub(crate) const RETIRE_BASELINE_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_subtitle_baseline.mw"
));
pub(crate) const RENAME_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_rename_subtitle.mw"
));

// Baseline with a runnable zero-arg entry: a `subtitle` member a later rename/retire
// consumes, plus a `seed` that writes through the store so the fence is exercised.
pub(crate) const BLOCK_BASELINE_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_seed_subtitle_baseline.mw"
));

// The renamed source with the consumed rename block present.
pub(crate) const RENAME_BLOCK_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_seed_rename_block.mw"
));

// The renamed source with the consumed rename block removed: the rename is already
// recorded in the accepted catalog, so the block is transient and safe to delete.
pub(crate) const RENAME_BLOCK_DELETED_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_seed_rename_block_deleted.mw"
));

// The retired source with the consumed retire block present.
pub(crate) const RETIRE_BLOCK_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_seed_retire_block.mw"
));

// The retired source with the consumed retire block removed.
pub(crate) const RETIRE_BLOCK_DELETED_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/books_seed_retire_block_deleted.mw"
));
pub(crate) const BRANCH_WORKFLOW_BASELINE_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/branch_workflow_baseline.mw"
));
pub(crate) const BRANCH_WORKFLOW_EVOLVED_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/branch_workflow_evolved.mw"
));
pub(crate) const LEAF_RETYPE_BASELINE_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/leaf_retype_baseline.mw"
));
pub(crate) const LEAF_RETYPE_TRANSFORM_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/leaf_retype_transform.mw"
));
pub(crate) const LEAF_RETYPE_RETIRE_OLD_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/leaf_retype_retire_old.mw"
));
pub(crate) const STORE_REKEY_BASELINE_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/store_rekey_baseline.mw"
));
pub(crate) const STORE_REKEY_STRING_TARGET_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/store_rekey_string_target.mw"
));
pub(crate) const ORPHAN_REPAIR_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/orphan_repair_source.mw"
));
pub(crate) const ORPHAN_REPAIRED_TARGET_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/v01/evolution/orphan_repaired_target.mw"
));
