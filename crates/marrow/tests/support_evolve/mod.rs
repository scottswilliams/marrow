use std::fs;
use std::path::{Path, PathBuf};

use marrow_check::{CheckedProgram, CheckedSavedPlace, ProjectConfig, check_project};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, ScalarType, decode_value, encode_value};

use crate::support::{TempProject, temp_project_uncommitted as temp_project, write};

// The saved-place fact lookups are owned by marrow-check behind its `test-support`
// feature, so the CLI evolution suites resolve a member or store catalog id through the
// same helpers the discharge and apply suites do.
#[allow(unused_imports)]
pub(crate) use marrow_check::test_support::{
    member_catalog_id, root_place, store_id_of as store_catalog_id,
};

#[allow(dead_code)]
fn config(root: impl AsRef<Path>) -> ProjectConfig {
    let text = fs::read_to_string(root.as_ref().join("marrow.json")).expect("read config");
    marrow_project::parse_config(&text).expect("parse config")
}

/// Freeze a project's baseline durable identity into its engine-resident store, the way
/// a state-establishing run does, and return the program re-bound against the accepted
/// store snapshot. The store snapshot is the source of truth the production read paths
/// bind.
#[allow(dead_code)]
pub(crate) fn commit_catalog(root: impl AsRef<Path>) -> CheckedProgram {
    let root = root.as_ref();
    let config = config(root);
    let (report, program) = check_project(root, &config).expect("check for proposal");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);

    let store = open_native_store(root);
    marrow_run::evolution::commit_catalog_baseline(&store, &program)
        .expect("commit catalog baseline");

    let accepted = store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot");
    let (report, program) =
        marrow_check::check_project_with_catalog(root, &config, accepted.as_ref())
            .expect("re-check against accepted catalog");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program
}

#[allow(dead_code)]
pub(crate) fn native_store_path(root: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(".data").join("marrow.redb")
}

#[allow(dead_code)]
pub(crate) fn open_native_store(root: impl AsRef<Path>) -> TreeStore {
    let path = native_store_path(root);
    fs::create_dir_all(path.parent().unwrap()).expect("create data dir");
    TreeStore::open(&path).expect("open native store")
}

#[allow(dead_code)]
pub(crate) fn seed_title_only(store: &TreeStore, place: &CheckedSavedPlace, id: i64, title: &str) {
    let store_id = store_catalog_id(place);
    store
        .write_node(&store_id, &[SavedKey::Int(id)])
        .expect("write record");
    let title_id = CatalogId::new(member_catalog_id(place, "title")).expect("title id");
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(title_id)],
            encode_value(&Scalar::Str(title.to_string())).expect("encode title"),
        )
        .expect("write title");
}

#[allow(dead_code)]
pub(crate) fn seed_member(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member: &str,
    value: Scalar,
) {
    let store_id = store_catalog_id(place);
    let member_id = CatalogId::new(member_catalog_id(place, member)).expect("member id");
    store
        .write_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member_id)],
            encode_value(&value).expect("encode member"),
        )
        .expect("write member");
}

#[allow(dead_code)]
pub(crate) fn read_scalar(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member: &str,
    ty: ScalarType,
) -> Option<Scalar> {
    let store_id = store_catalog_id(place);
    let member_id = CatalogId::new(member_catalog_id(place, member)).expect("member id");
    store
        .read_data_value(
            &store_id,
            &[SavedKey::Int(id)],
            &[DataPathSegment::Member(member_id)],
        )
        .expect("read member")
        .map(|bytes| decode_value(&bytes, ty).expect("decode value"))
}

#[allow(dead_code)]
pub(crate) fn read_scalar_by_catalog_id(
    store: &TreeStore,
    place: &CheckedSavedPlace,
    id: i64,
    member_id: &str,
    ty: ScalarType,
) -> Option<Scalar> {
    let store_id = store_catalog_id(place);
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

#[allow(dead_code)]
pub(crate) fn native_books_project(name: &str, source: &str) -> TempProject {
    temp_project(name, |root| {
        write(root, "marrow.json", crate::support::native_config());
        write(root, "src/books.mw", source);
    })
}

/// The accepted catalog a project's engine-resident store publishes. The store is the
/// source of truth a run or an evolution apply advances; reading it here is the typed
/// oracle for the project's committed durable identity.
#[allow(dead_code)]
pub(crate) fn accepted_catalog(root: impl AsRef<Path>) -> marrow_catalog::CatalogMetadata {
    let store = TreeStore::open_read_only(&native_store_path(root)).expect("open store read-only");
    store
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
        .expect("project has an accepted catalog")
}

#[allow(dead_code)]
pub(crate) fn accepted_catalog_entry_id(root: impl AsRef<Path>, path: &str) -> String {
    accepted_catalog(root)
        .entries
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("accepted catalog entry `{path}`"))
        .stable_id
}

#[allow(dead_code)]
pub(crate) fn store_epoch(root: impl AsRef<Path>) -> Option<u64> {
    let store = TreeStore::open(&native_store_path(root)).expect("reopen native store");
    store.read_catalog_epoch().expect("read store epoch")
}

// The before/after `module books` evolution sources live in the repo-root corpus, so
// the same fixture is not re-declared as an inline string here and in the runtime
// crate. The baseline/default/subtitle/retire shapes are shared with marrow-run.
#[allow(dead_code)]
pub(crate) const REQUIRED_DEFAULT_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_required_default.mw");

#[allow(dead_code)]
pub(crate) const REQUIRED_NO_DEFAULT_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_required_no_default.mw");

#[allow(dead_code)]
pub(crate) const REQUIRED_BASELINE_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_required_baseline.mw");

#[allow(dead_code)]
pub(crate) const OPTIONAL_PAGES_BASELINE_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_optional_pages_baseline.mw");

#[allow(dead_code)]
pub(crate) const OPTIONAL_PAGES_DEFAULT_INDEX_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_optional_pages_default_index.mw");

#[allow(dead_code)]
pub(crate) const PRICE_BASELINE_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_price_baseline.mw");

#[allow(dead_code)]
pub(crate) const PRICE_CENTS_TRANSFORM_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_price_cents_transform.mw");

#[allow(dead_code)]
pub(crate) const RETIRE_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_retire_subtitle.mw");

#[allow(dead_code)]
pub(crate) const RETIRE_BASELINE_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_subtitle_baseline.mw");

#[allow(dead_code)]
pub(crate) const RENAME_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_rename_subtitle.mw");

// Baseline with a runnable zero-arg entry: a `subtitle` member a later rename/retire
// consumes, plus a `seed` that writes through the store so the fence is exercised.
#[allow(dead_code)]
pub(crate) const BLOCK_BASELINE_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_seed_subtitle_baseline.mw");

// The renamed source with the consumed rename block present.
#[allow(dead_code)]
pub(crate) const RENAME_BLOCK_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_seed_rename_block.mw");

// The renamed source with the consumed rename block removed: the rename is already
// recorded in the accepted catalog, so the block is transient and safe to delete.
#[allow(dead_code)]
pub(crate) const RENAME_BLOCK_DELETED_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_seed_rename_block_deleted.mw");

// The retired source with the consumed retire block present.
#[allow(dead_code)]
pub(crate) const RETIRE_BLOCK_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_seed_retire_block.mw");

// The retired source with the consumed retire block removed.
#[allow(dead_code)]
pub(crate) const RETIRE_BLOCK_DELETED_SOURCE: &str =
    include_str!("../../../../fixtures/v01/evolution/books_seed_retire_block_deleted.mw");
