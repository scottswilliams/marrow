use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use marrow_check::checked_saved_root_place;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

mod support;

use support::{TempProject, marrow, member_catalog_id, temp_project, write};

/// The `code` field of a single JSON error record printed to stdout.
fn json_code(output: &Output) -> String {
    support::json(output.stdout.clone())["code"]
        .as_str()
        .expect("json error code")
        .to_string()
}

fn marrow_with_env(args: &[&str], key: &str, value: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .env(key, value)
        .output()
        .expect("run marrow")
}

#[cfg(unix)]
fn marrow_with_umask_000(args: &[&str]) -> Output {
    Command::new("/bin/sh")
        .arg("-c")
        .arg("umask 000; exec \"$0\" \"$@\"")
        .arg(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow under permissive umask")
}

#[cfg(unix)]
fn assert_owner_only_file(path: &Path) {
    let mode = fs::metadata(path)
        .expect("read file metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "{} mode is {mode:o}", path.display());
}

fn temp_artifacts_for(path: &Path) -> Vec<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .expect("artifact file name")
        .to_string_lossy();
    let prefix = format!(".{file_name}.");
    fs::read_dir(parent)
        .expect("read artifact parent")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|entry| {
            entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
        })
        .collect()
}

/// A native-store project whose `seed` entry writes one book, plus its committed
/// catalog. Running `seed` populates the store; the data directory can then be
/// removed to model an empty restore target with the same source and catalog.
fn seeded_project(name: &str) -> (TempProject, PathBuf) {
    let root = temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::seed" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Book\n\
             \x20\x20\x20\x20required title: string\n\
             store ^books(id: int): Book\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20var b: Book\n\
             \x20\x20\x20\x20b.title = \"Mort\"\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1) = b\n",
        );
    });
    let data_dir = root.join(".data");
    let seed = marrow(&["run", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    (root, data_dir)
}

/// Empty a project's saved data before a restore, leaving a truly empty store. The
/// accepted catalog is engine-resident, so removing the store data dir removes the
/// accepted snapshot too; restore replays the catalog rows the backup carries, so the
/// target needs no re-established baseline and binds the backup's own accepted identity.
fn empty_store_data(_root: &Path, data_dir: &Path) {
    fs::remove_dir_all(data_dir).expect("remove store data");
}

/// Assert a restore target holds nothing durable: no data or index cells and no accepted
/// catalog. A rejected restore rolls its whole transaction back, so the target is exactly
/// as empty as it was found, carrying neither replayed data nor catalog rows.
fn assert_store_empty(data_dir: &Path) {
    let store_file = data_dir.join("marrow.redb");
    if !store_file.exists() {
        return;
    }
    let store = TreeStore::open_read_only(&store_file).expect("open target store");
    assert!(
        store.is_empty().expect("read target data"),
        "a rejected restore leaves no data or index cells"
    );
    assert_eq!(
        store.read_catalog_snapshot().expect("read target catalog"),
        None,
        "a rejected restore leaves no accepted catalog"
    );
}

/// The accepted catalog snapshot a native store holds, read through the read-only store
/// API. `None` when the store file is absent or holds no accepted catalog.
fn read_store_catalog(data_dir: &Path) -> Option<marrow_catalog::CatalogMetadata> {
    let path = data_dir.join("marrow.redb");
    if !path.exists() {
        return None;
    }
    TreeStore::open_read_only(&path)
        .expect("open store read-only")
        .read_catalog_snapshot()
        .expect("read store catalog snapshot")
}

fn evolution_default_project(name: &str) -> (TempProject, PathBuf) {
    let root = temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::seed" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Book\n\
             \x20\x20\x20\x20required title: string\n\
             store ^books(id: int): Book\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n",
        );
    });
    let data_dir = root.join(".data");
    let seed = marrow(&["run", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    (root, data_dir)
}

fn add_pages_default_evolution(root: impl AsRef<Path>) {
    write(
        root.as_ref(),
        "src/shelf.mw",
        "module shelf\n\
         \n\
         resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20required pages: int\n\
         store ^books(id: int): Book\n\
         \n\
         evolve\n\
         \x20\x20\x20\x20default Book.pages = 0\n\
         \n\
         pub fn seed()\n\
         \x20\x20\x20\x20transaction\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n\
         \x20\x20\x20\x20\x20\x20\x20\x20^books(1).pages = 0\n",
    );
}

/// Every saved `(path, value_b64)` record the store holds, read through the typed
/// `data dump --format json` envelope. Two dumps comparing equal proves a byte-exact
/// round-trip of the saved data, asserted on parsed records rather than rendered text.
fn dump_records(root: impl AsRef<Path>) -> Vec<serde_json::Value> {
    let out = marrow(&[
        "data",
        "dump",
        "--format",
        "json",
        root.as_ref().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "data dump: {out:?}");
    support::json(out.stdout)["records"]
        .as_array()
        .expect("dump records array")
        .clone()
}

/// The stored value bytes at one source-text path, read through the typed
/// `data get --format json` envelope, or `None` when the path holds no direct value.
/// A data-presence check goes through this structured read, never a stdout substring.
fn data_get_value(root: impl AsRef<Path>, path: &str) -> Option<Vec<u8>> {
    let out = marrow(&[
        "data",
        "get",
        "--format",
        "json",
        root.as_ref().to_str().unwrap(),
        path,
    ]);
    assert_eq!(out.status.code(), Some(0), "data get {path}: {out:?}");
    support::json(out.stdout)["value_b64"]
        .as_str()
        .map(|b64| marrow_run::base64::decode(b64).expect("decode value"))
}

#[test]
fn backup_then_restore_round_trips_saved_data() {
    let (root, data_dir) = seeded_project("backup-roundtrip");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let before = dump_records(&root);
    assert_eq!(
        data_get_value(&root, "^books(1).title"),
        Some(b"Mort".to_vec()),
        "seed wrote a book"
    );

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");
    assert!(archive.exists(), "backup wrote the archive file");

    // Empty the store: same source and catalog, no saved data.
    empty_store_data(&root, &data_dir);
    let roots = marrow(&["data", "roots", "--format", "json", &dir]);
    assert_eq!(
        support::json(roots.stdout)["roots"],
        serde_json::json!([]),
        "store is empty before restore"
    );

    let restore = marrow(&["restore", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");

    let after = dump_records(&root);
    assert_eq!(after, before, "restored data matches the original");
    assert_eq!(
        data_get_value(&root, "^books(1).title"),
        Some(b"Mort".to_vec()),
        "the restored book is readable"
    );
}

#[test]
fn backup_failure_preserves_prior_archive_and_removes_temp_file() {
    let (root, _data_dir) = seeded_project("backup-atomic-failure");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");
    let prior = fs::read(&archive).expect("read prior archive");

    let failed = marrow_with_env(
        &["backup", &dir, &archive_arg],
        "MARROW_TEST_BACKUP_FAIL_AFTER_BYTES",
        "32",
    );
    assert_eq!(
        failed.status.code(),
        Some(1),
        "injected write failure must fail: {failed:?}"
    );
    assert_eq!(
        fs::read(&archive).expect("read archive after failure"),
        prior,
        "a failed backup must preserve the previously published archive byte-for-byte"
    );
    assert_eq!(
        temp_artifacts_for(&archive),
        Vec::<PathBuf>::new(),
        "a failed backup must remove its adjacent temp artifact"
    );
}

#[cfg(unix)]
#[test]
fn backup_archive_is_owner_only_under_permissive_umask() {
    let (root, _data_dir) = seeded_project("backup-owner-only-archive");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let backup = marrow_with_umask_000(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");
    assert_owner_only_file(&archive);
}

#[cfg(unix)]
#[test]
fn native_store_file_is_owner_only_under_permissive_umask() {
    let root = support::temp_project_uncommitted("store-owner-only", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::seed" } }"#,
        );
        write(root, "src/app.mw", support::counter_source());
    });
    let dir = root.to_str().unwrap().to_string();

    let run = marrow_with_umask_000(&["run", &dir]);
    assert_eq!(run.status.code(), Some(0), "run: {run:?}");
    assert_owner_only_file(&root.join(".data/marrow.redb"));
}

#[cfg(unix)]
#[test]
fn native_store_symlink_target_is_owner_only_under_permissive_umask() {
    let root = support::temp_project_uncommitted("store-owner-only-symlink", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "app::seed" } }"#,
        );
        write(root, "src/app.mw", support::counter_source());
    });
    let data_dir = root.join(".data");
    let outside_dir = root.join("outside");
    fs::create_dir_all(&data_dir).expect("create data dir");
    fs::create_dir_all(&outside_dir).expect("create symlink target dir");
    let store_path = data_dir.join("marrow.redb");
    let symlink_target = outside_dir.join("marrow.redb");
    std::os::unix::fs::symlink("../outside/marrow.redb", &store_path)
        .expect("create dangling store symlink");
    assert!(!symlink_target.exists(), "fixture target starts missing");

    let dir = root.to_str().unwrap().to_string();
    let run = marrow_with_umask_000(&["run", &dir]);
    assert_eq!(run.status.code(), Some(0), "run: {run:?}");
    assert!(
        fs::symlink_metadata(&store_path)
            .expect("store path metadata")
            .file_type()
            .is_symlink(),
        "store path remains the configured symlink"
    );
    assert_owner_only_file(&symlink_target);
}

/// With engine-resident atomic publish there is no activation window: an `evolve apply`
/// advances the store's catalog and data together, so a backup taken after it carries the
/// evolved accepted catalog. A restore replays those rows, so the restored store is
/// self-contained and runs immediately — no resume or re-evolve step, and a fresh `evolve
/// apply` finds nothing to do.
#[test]
fn restore_of_an_evolved_store_runs_immediately_with_no_resume() {
    let (root, data_dir) = evolution_default_project("backup-evolved-restore");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("evolved.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    add_pages_default_evolution(&root);
    let apply = marrow(&["evolve", "apply", &dir]);
    assert_eq!(apply.status.code(), Some(0), "evolve apply: {apply:?}");
    let applied_catalog = read_store_catalog(&data_dir).expect("evolved catalog snapshot");
    let applied_catalog_epoch = Some(applied_catalog.epoch);

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    empty_store_data(&root, &data_dir);
    let restore = marrow(&["restore", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");

    // Restore replayed the evolved catalog rows, so the restored store carries the same
    // accepted identity the apply published, not a freshly proposed baseline.
    assert_eq!(
        read_store_catalog(&data_dir),
        Some(applied_catalog),
        "restore replays the evolved catalog snapshot from the backup"
    );

    // The restored store runs immediately: the evolved data and backfilled default are
    // readable with no resume step.
    assert_eq!(
        data_get_value(&root, "^books(1).title"),
        Some(b"Mort".to_vec()),
        "the restored book is readable with no resume"
    );
    assert_eq!(
        data_get_value(&root, "^books(1).pages"),
        Some(b"0".to_vec()),
        "the backfilled default is readable with no resume"
    );

    // A fresh apply against the restored store finds nothing new to evolve: the source
    // already matches the restored accepted catalog, so the apply is idempotent and leaves
    // the accepted identity exactly where the restore put it — there is no activation
    // window to resume.
    let reapply = marrow(&["evolve", "apply", "--format", "json", &dir]);
    assert_eq!(reapply.status.code(), Some(0), "reapply: {reapply:?}");
    assert_eq!(
        read_store_catalog(&data_dir).map(|catalog| catalog.epoch),
        applied_catalog_epoch,
        "a no-op apply does not advance the restored accepted catalog"
    );
}

#[test]
fn restore_refuses_a_non_empty_target() {
    let (root, _data_dir) = seeded_project("backup-not-empty");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );
    // The store still holds the seeded data, so restore must refuse it.
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.not_empty");
}

#[test]
fn rejected_restore_does_not_leave_a_created_store_file() {
    let (root, data_dir) = seeded_project("backup-rollback-no-created-store-file");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );
    let mut bytes = fs::read(&archive).expect("read archive");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&archive, &bytes).expect("write corrupt archive");

    fs::remove_dir_all(&data_dir).expect("remove store data");
    fs::create_dir_all(&data_dir).expect("recreate pristine data dir");
    let store_file = data_dir.join("marrow.redb");
    assert!(
        !store_file.exists(),
        "the target starts as an existing .data dir with no store file"
    );

    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.corrupt_chunk");
    assert!(
        !store_file.exists(),
        "a rejected restore removes only the store file it created"
    );
    assert!(
        data_dir.exists(),
        "a pre-existing empty .data directory is left in place"
    );
}

#[cfg(unix)]
#[test]
fn rejected_restore_preserves_dangling_store_symlink_without_orphaning_target() {
    let (root, data_dir) = seeded_project("backup-rollback-dangling-store-symlink");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );
    let mut bytes = fs::read(&archive).expect("read archive");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&archive, &bytes).expect("write corrupt archive");

    fs::remove_dir_all(&data_dir).expect("remove store data");
    fs::create_dir_all(&data_dir).expect("recreate data dir");
    let store_file = data_dir.join("marrow.redb");
    let symlink_target_dir = root.join("outside-data");
    fs::create_dir_all(&symlink_target_dir).expect("create symlink target parent");
    let symlink_target = symlink_target_dir.join("marrow.redb");
    std::os::unix::fs::symlink(&symlink_target, &store_file).expect("create store symlink");
    assert!(
        fs::symlink_metadata(&store_file)
            .expect("read store symlink metadata")
            .file_type()
            .is_symlink(),
        "fixture store path starts as a symlink"
    );
    assert!(!store_file.exists(), "fixture store symlink is dangling");
    assert!(!symlink_target.exists(), "fixture target starts absent");

    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.corrupt_chunk");

    let link_preserved = fs::symlink_metadata(&store_file)
        .as_ref()
        .is_ok_and(|metadata| metadata.file_type().is_symlink());
    let target_exists = symlink_target.exists();
    assert!(
        link_preserved && !target_exists,
        "rejected restore must preserve the symlink and remove the created target; \
         link_preserved={link_preserved} target_exists={target_exists}"
    );
    assert_eq!(
        fs::read_link(&store_file).expect("read preserved store symlink"),
        symlink_target,
        "restore leaves the configured symlink target unchanged"
    );
}

#[cfg(unix)]
#[test]
fn rejected_restore_removes_created_final_target_behind_store_symlink_chain() {
    let (root, data_dir) = seeded_project("backup-rollback-store-symlink-chain");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );
    let mut bytes = fs::read(&archive).expect("read archive");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&archive, &bytes).expect("write corrupt archive");

    fs::remove_dir_all(&data_dir).expect("remove store data");
    fs::create_dir_all(&data_dir).expect("recreate data dir");
    let store_file = data_dir.join("marrow.redb");
    let intermediate_link = root.join("outside-link.redb");
    let final_target = root.join("missing-final.redb");
    std::os::unix::fs::symlink("missing-final.redb", &intermediate_link)
        .expect("create intermediate symlink");
    std::os::unix::fs::symlink("../outside-link.redb", &store_file).expect("create store symlink");

    assert!(
        fs::symlink_metadata(&store_file)
            .expect("read store symlink metadata")
            .file_type()
            .is_symlink(),
        "fixture store path starts as a symlink"
    );
    assert!(
        fs::symlink_metadata(&intermediate_link)
            .expect("read intermediate symlink metadata")
            .file_type()
            .is_symlink(),
        "fixture intermediate path starts as a symlink"
    );
    assert!(!store_file.exists(), "fixture store symlink is dangling");
    assert!(
        !intermediate_link.exists(),
        "fixture intermediate symlink is dangling"
    );
    assert!(!final_target.exists(), "fixture final target starts absent");

    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.corrupt_chunk");

    let store_link_preserved = fs::symlink_metadata(&store_file)
        .as_ref()
        .is_ok_and(|metadata| metadata.file_type().is_symlink());
    let intermediate_link_preserved = fs::symlink_metadata(&intermediate_link)
        .as_ref()
        .is_ok_and(|metadata| metadata.file_type().is_symlink());
    let final_target_exists = final_target.exists();
    assert!(
        store_link_preserved && intermediate_link_preserved && !final_target_exists,
        "rejected restore must preserve both symlinks and remove the created final target; \
         store_link_preserved={store_link_preserved} \
         intermediate_link_preserved={intermediate_link_preserved} \
         final_target_exists={final_target_exists}"
    );
    assert_eq!(
        fs::read_link(&store_file).expect("read preserved store symlink"),
        PathBuf::from("../outside-link.redb"),
        "restore leaves the configured symlink target unchanged"
    );
    assert_eq!(
        fs::read_link(&intermediate_link).expect("read preserved intermediate symlink"),
        PathBuf::from("missing-final.redb"),
        "restore leaves the intermediate symlink target unchanged"
    );
}

#[test]
fn restore_refuses_a_catalog_only_target() {
    let (source, _source_data_dir) = seeded_project("backup-catalog-only-source");
    let source_dir = source.to_str().unwrap().to_string();
    let archive = source.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["backup", &source_dir, &archive_arg]).status.code(),
        Some(0)
    );

    let target = temp_project("backup-catalog-only-target", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::seed" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Book\n\
             \x20\x20\x20\x20required title: string\n\
             store ^books(id: int): Book\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20var b: Book\n\
             \x20\x20\x20\x20b.title = \"Mort\"\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1) = b\n",
        );
    });
    let target_dir = target.to_str().unwrap().to_string();
    let target_data_dir = target.join(".data");
    let target_store_file = target_data_dir.join("marrow.redb");
    let before_catalog = read_store_catalog(&target_data_dir).expect("catalog-only baseline");
    assert!(
        TreeStore::open_read_only(&target_store_file)
            .expect("open target read-only")
            .is_empty()
            .expect("catalog-only target has no data or index cells"),
        "fixture target is catalog-only"
    );

    let restore = marrow(&["restore", "--format", "json", &target_dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.not_empty");
    assert!(
        target_store_file.exists(),
        "restore must not delete a pre-existing catalog-only store"
    );
    assert_eq!(
        read_store_catalog(&target_data_dir),
        Some(before_catalog),
        "restore leaves the pre-existing catalog-only baseline unchanged"
    );
}

#[test]
fn restore_rejects_a_corrupt_backup() {
    let (root, data_dir) = seeded_project("backup-corrupt");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );

    // Flip the final byte of the cell stream, leaving the header intact, so the
    // data checksum no longer matches.
    let mut bytes = fs::read(&archive).expect("read archive");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&archive, &bytes).expect("write corrupt archive");

    empty_store_data(&root, &data_dir);
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.corrupt_chunk");
    assert_store_empty(&data_dir);
}

/// A native-store project with both a non-unique and a unique index over a keyed
/// root, plus a `seed` entry that writes several books and lookups that read through
/// each index. Running `seed` populates the data and the maintained indexes.
fn indexed_project(name: &str) -> (TempProject, PathBuf) {
    let root = temp_project(name, |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::seed" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Book\n\
             \x20\x20\x20\x20required title: string\n\
             \x20\x20\x20\x20shelf: string\n\
             \x20\x20\x20\x20isbn: string\n\
             store ^books(id: int): Book\n\
             \n\
             \x20\x20\x20\x20index byShelf(shelf, id)\n\
             \x20\x20\x20\x20index byIsbn(isbn) unique\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"Mort\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).shelf = \"fiction\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).isbn = \"978-1\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).title = \"Reaper\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).shelf = \"fiction\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).isbn = \"978-2\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(3).title = \"Sourcery\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(3).shelf = \"history\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(3).isbn = \"978-3\"\n\
             \n\
             pub fn find_isbn()\n\
             \x20\x20\x20\x20for id in ^books.byIsbn(\"978-2\")\n\
             \x20\x20\x20\x20\x20\x20\x20\x20print(^books(id).title)\n\
             \n\
             pub fn count_shelf()\n\
             \x20\x20\x20\x20var c = 0\n\
             \x20\x20\x20\x20for id in keys(^books.byShelf(\"fiction\"))\n\
             \x20\x20\x20\x20\x20\x20\x20\x20c = c + 1\n\
             \x20\x20\x20\x20print($\"{c}\")\n",
        );
    });
    let data_dir = root.join(".data");
    let seed = marrow(&["run", root.to_str().unwrap()]);
    assert_eq!(seed.status.code(), Some(0), "seed run: {seed:?}");
    (root, data_dir)
}

/// A backup carries data only; restore rebuilds the generated indexes from the
/// restored records. After a backup, an emptied store, and a restore, both a unique
/// lookup and a non-unique `keys` traversal resolve the rebuilt entries.
#[test]
fn restore_rebuilds_indexes_usable_through_lookups() {
    let (root, data_dir) = indexed_project("backup-index-rebuild");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    // Remove the entire store data directory, then restore from the archive.
    empty_store_data(&root, &data_dir);
    let restore = marrow(&["restore", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");

    // The unique index resolves the looked-up record by isbn; the program prints the
    // title of the single matching book, so its stdout is exactly that runtime value.
    let unique = marrow(&["run", "--entry", "shelf::find_isbn", &dir]);
    assert_eq!(unique.status.code(), Some(0), "find_isbn run: {unique:?}");
    assert_eq!(
        String::from_utf8(unique.stdout).expect("utf8"),
        "Reaper\n",
        "rebuilt unique index resolves the book"
    );
    // The non-unique index resolves both fiction books; the program prints the count.
    let count = marrow(&["run", "--entry", "shelf::count_shelf", &dir]);
    assert_eq!(count.status.code(), Some(0), "count_shelf run: {count:?}");
    assert_eq!(
        String::from_utf8(count.stdout).expect("utf8"),
        "2\n",
        "rebuilt non-unique index resolves both fiction books"
    );
}

fn checked_books_place(root: impl AsRef<Path>) -> marrow_check::CheckedSavedPlace {
    let root = root.as_ref();
    support::commit_catalog_if_clean(root);
    let config_text = fs::read_to_string(root.join("marrow.json")).expect("read config");
    let config = marrow_project::parse_config(&config_text).expect("parse config");
    // Bind the program against the engine-resident accepted catalog so its saved roots
    // carry the same catalog ids the live store keys cells under.
    let accepted = support::native_store_path(root, &config)
        .filter(|path| path.exists())
        .and_then(|path| {
            marrow_store::tree::TreeStore::open_read_only(&path)
                .expect("open store read-only")
                .read_catalog_snapshot()
                .expect("read store catalog snapshot")
        });
    let (report, program) =
        marrow_check::check_project_with_catalog(root, &config, accepted.as_ref())
            .expect("check project");
    assert!(!report.has_errors(), "fixture checks cleanly: {report:#?}");
    checked_saved_root_place(&program, "books", marrow_syntax::SourceSpan::default())
        .expect("checked saved root place")
}

/// The real store catalog id of `^books`, read through the production check path so
/// an orphan cell can be written under the live store but an undeclared member.
fn store_catalog_id(root: impl AsRef<Path>) -> CatalogId {
    let place = checked_books_place(root);
    CatalogId::new(place.store_catalog_id.expect("accepted store catalog id"))
        .expect("store catalog id")
}

/// A backup that carries an orphan data cell is not valid under the target
/// source/catalog, so restore rejects it before committing anything to the empty
/// target. Orphans are compiler/data-integrity facts, not faithful debris restore
/// may activate.
#[test]
fn restore_rejects_a_backup_carrying_an_orphan_cell() {
    let (root, data_dir) = seeded_project("backup-orphan");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    // Write a data cell under the live store but an undeclared member catalog id: a
    // dropped field left this behind. It is a data-family cell, so the backup copies it.
    let store_catalog = store_catalog_id(&root);
    {
        let store =
            TreeStore::open(&data_dir.join("marrow.redb")).expect("open native store for orphan");
        store
            .write_data_value(
                &store_catalog,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(
                    CatalogId::new("cat_000000000000000000000000cafef00d".to_string())
                        .expect("orphan member id"),
                )],
                b"left-behind".to_vec(),
            )
            .expect("write orphan cell");
    }

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    empty_store_data(&root, &data_dir);
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(
        restore.status.code(),
        Some(1),
        "restore rejects a backup with orphan debris: {restore:?}"
    );
    assert_eq!(json_code(&restore), "restore.data_invalid");
    assert_store_empty(&data_dir);
}

#[test]
fn restore_rejects_a_backup_carrying_an_impossible_data_cell_shape() {
    let (root, data_dir) = seeded_project("backup-impossible-cell-shape");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let place = checked_books_place(&root);
    let store_catalog = CatalogId::new(
        place
            .store_catalog_id
            .clone()
            .expect("accepted store catalog id"),
    )
    .expect("store catalog id");
    let title_catalog = member_catalog_id(&place.root_members, "title");
    {
        let store = TreeStore::open(&data_dir.join("marrow.redb"))
            .expect("open native store for impossible cell");
        store
            .write_data_value(
                &store_catalog,
                &[SavedKey::Int(1)],
                &[
                    DataPathSegment::Member(title_catalog),
                    DataPathSegment::Key(SavedKey::Int(99)),
                ],
                b"impossible".to_vec(),
            )
            .expect("write impossible cell shape");
    }

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    empty_store_data(&root, &data_dir);
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(
        restore.status.code(),
        Some(1),
        "restore rejects a backup with an impossible data cell shape: {restore:?}"
    );
    assert_eq!(json_code(&restore), "restore.data_invalid");
    assert_store_empty(&data_dir);
}

/// A faithful backup ends exactly at its last cell. Appending bytes makes the file
/// no longer the backup the manifest describes, so restore rejects it.
#[test]
fn restore_rejects_trailing_bytes() {
    let (root, data_dir) = seeded_project("backup-trailing");
    let dir = root.to_str().unwrap().to_string();
    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0)
    );

    // Append stray bytes after the cell stream, leaving the header and checksum intact.
    let mut bytes = fs::read(&archive).expect("read archive");
    bytes.extend_from_slice(b"trailing");
    fs::write(&archive, &bytes).expect("write archive with trailing bytes");

    empty_store_data(&root, &data_dir);
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(1), "restore: {restore:?}");
    assert_eq!(json_code(&restore), "restore.corrupt_chunk");
    assert_store_empty(&data_dir);
}

/// `nextId(^books)` allocates from the highest stored record id, which lives in the
/// data the backup carries. After a round-trip into an emptied store, the next id
/// continues from the same value the original store would have allocated.
#[test]
fn restore_continues_next_id_from_the_restored_data() {
    let root = temp_project("backup-next-id", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" }, "run": { "defaultEntry": "shelf::seed" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\
             \n\
             resource Book\n\
             \x20\x20\x20\x20required title: string\n\
             store ^books(id: int): Book\n\
             \n\
             pub fn seed()\n\
             \x20\x20\x20\x20transaction\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(1).title = \"a\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(2).title = \"b\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20^books(3).title = \"c\"\n\
             \n\
             pub fn peek_next()\n\
             \x20\x20\x20\x20print($\"{nextId(^books)}\")\n",
        );
    });
    let data_dir = root.join(".data");
    let dir = root.to_str().unwrap().to_string();
    assert_eq!(marrow(&["run", &dir]).status.code(), Some(0), "seed");

    // The highest stored id is 3, so nextId is 4 before any round-trip.
    let before = marrow(&["run", "--entry", "shelf::peek_next", &dir]);
    assert_eq!(before.status.code(), Some(0), "peek before: {before:?}");
    assert_eq!(
        String::from_utf8(before.stdout).expect("utf8"),
        "4\n",
        "nextId before restore is 4"
    );

    let archive = root.join("books.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["backup", &dir, &archive_arg]).status.code(),
        Some(0),
        "backup"
    );
    empty_store_data(&root, &data_dir);
    assert_eq!(
        marrow(&["restore", &dir, &archive_arg]).status.code(),
        Some(0),
        "restore"
    );

    // After restore, nextId continues from the restored data: still 4.
    let after = marrow(&["run", "--entry", "shelf::peek_next", &dir]);
    assert_eq!(after.status.code(), Some(0), "peek after: {after:?}");
    assert_eq!(
        String::from_utf8(after.stdout).expect("utf8"),
        "4\n",
        "nextId after restore continues from the restored data"
    );
}

#[test]
fn backup_of_an_unseeded_project_restores_empty() {
    let root = temp_project("backup-empty", |root| {
        write(
            root,
            "marrow.json",
            r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#,
        );
        write(
            root,
            "src/shelf.mw",
            "module shelf\n\nresource Book\n\x20\x20\x20\x20required title: string\nstore ^books(id: int): Book\n",
        );
    });
    let dir = root.to_str().unwrap().to_string();
    let data_dir = root.join(".data");
    let archive = root.join("empty.mwbackup");
    let archive_arg = archive.to_str().unwrap().to_string();

    let backup = marrow(&["backup", &dir, &archive_arg]);
    assert_eq!(backup.status.code(), Some(0), "backup: {backup:?}");

    empty_store_data(&root, &data_dir);
    let restore = marrow(&["restore", "--format", "json", &dir, &archive_arg]);
    assert_eq!(restore.status.code(), Some(0), "restore: {restore:?}");
    assert_eq!(
        support::json(restore.stdout)["records"],
        serde_json::json!(0),
        "an empty backup restores zero records"
    );
}
