//! Project file, catalog artifact, and check-loading helpers.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_project::{ProjectConfig, StoreBackend, StoreConfig};
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use crate::{CheckReport, CheckedProgram};

/// The native store `dataDir` directory could not be created.
pub const CONFIG_DATA_DIR: &str = "config.data_dir";

#[derive(Debug)]
pub enum ProjectIoError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The native store's `dataDir` directory could not be created: the path is
    /// occupied by a non-directory file, a parent denies access, or the
    /// filesystem is read-only. This is a write-path (directory-creation) fault,
    /// distinct from a read of an existing file, so it never carries `io.read`.
    DataDirCreate {
        path: PathBuf,
        error: std::io::Error,
    },
    Config {
        code: &'static str,
        message: String,
    },
    Catalog {
        code: &'static str,
        message: String,
    },
    Check {
        report: CheckReport,
    },
    CheckLoad {
        code: &'static str,
        path: PathBuf,
        message: String,
    },
    Store(StoreError),
}

impl ProjectIoError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => crate::IO_READ,
            Self::DataDirCreate { .. } => CONFIG_DATA_DIR,
            Self::Config { code, .. } => code,
            Self::Catalog { code, .. } => code,
            Self::Check { .. } => "check.failed",
            Self::CheckLoad { code, .. } => code,
            Self::Store(error) => error.code(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Io { error, .. } => error.to_string(),
            Self::DataDirCreate { path, error } => format!(
                "cannot create the native store `dataDir` directory {}: {error}; \
                 point `dataDir` at a writable directory or remove the file occupying it",
                path.display()
            ),
            Self::Config { message, .. } => message.clone(),
            Self::Catalog { message, .. } => message.clone(),
            Self::Check { .. } => "project failed to check".to_string(),
            Self::CheckLoad { path, message, .. } => format!("{}: {message}", path.display()),
            Self::Store(error) => error.to_string(),
        }
    }
}

impl From<StoreError> for ProjectIoError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub fn load_config(root: &Path) -> Result<ProjectConfig, ProjectIoError> {
    let path = root.join("marrow.json");
    let json = fs::read_to_string(&path).map_err(|error| ProjectIoError::Io {
        path: path.clone(),
        error,
    })?;
    marrow_project::parse_config(&json).map_err(|error| ProjectIoError::Config {
        code: error.code,
        message: error.message,
    })
}

pub fn native_store_path(
    root: &Path,
    config: &ProjectConfig,
) -> Result<Option<PathBuf>, ProjectIoError> {
    match &config.store {
        StoreConfig {
            backend: StoreBackend::Memory,
            ..
        } => Ok(None),
        StoreConfig {
            backend: StoreBackend::Native,
            data_dir,
        } => {
            let data_dir = data_dir
                .as_deref()
                .filter(|data_dir| !data_dir.is_empty())
                .ok_or_else(native_store_data_dir_error)?;
            Ok(Some(root.join(data_dir).join("marrow.redb")))
        }
    }
}

fn native_store_data_dir_error() -> ProjectIoError {
    ProjectIoError::Config {
        code: marrow_project::CONFIG_INVALID,
        message: "the `native` store backend requires a non-empty `dataDir`".to_string(),
    }
}

pub fn resolve_store_path(
    root: &Path,
    config: &ProjectConfig,
) -> Result<Option<PathBuf>, ProjectIoError> {
    let Some(path) = native_store_path(root, config)? else {
        return Ok(None);
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ProjectIoError::DataDirCreate {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    Ok(Some(path))
}

/// The committed lock, read from `marrow.lock` and decoded through its [`CatalogLock`]
/// owner. A present-but-corrupt lock fails closed so a fresh checkout never mints over a
/// committed identity; an absent lock is a true first run. This is read-only: reading a
/// lock never writes the source tree.
pub fn read_committed_lock(
    root: &Path,
) -> Result<Option<marrow_catalog::CatalogLock>, ProjectIoError> {
    match read_lock_file(root) {
        LockRead::Missing => Ok(None),
        LockRead::Lock(lock) => Ok(Some(lock)),
        LockRead::Corrupt(error) => Err(ProjectIoError::Catalog {
            code: error.code,
            message: error.message,
        }),
        LockRead::ReadError { path, error } => Err(ProjectIoError::Io { path, error }),
    }
}

/// Read the accepted catalog with no saved-data store available: there is no accepted
/// authority to bind, so the result is the first-run `None`, but a present-but-corrupt
/// committed lock still fails closed rather than mint a fresh baseline over it.
pub fn read_accepted_catalog_artifact(
    root: &Path,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ProjectIoError> {
    read_committed_lock(root)?;
    Ok(None)
}

/// Bind the accepted catalog from the live store, never from the committed lock. The live
/// store is the sole write-time authority for accepted identity: a valid stamped store wins
/// unconditionally, even against a committed lock that disagrees or claims a newer epoch. The
/// lock only seeds first-run adoption (consumed at analyze time, not materialized here) and
/// reports staleness, so when no store snapshot is present the accepted catalog is the
/// first-run `None`. A present-but-corrupt lock fails closed. This never writes the source
/// tree: a read never repairs or re-projects the lock.
pub fn read_accepted_catalog_with_store(
    root: &Path,
    store: Option<&TreeStore>,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ProjectIoError> {
    bind_store_else_refuse_corrupt_lock(root, store)
}

pub fn read_accepted_catalog_with_store_read_only(
    root: &Path,
    store: Option<&TreeStore>,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ProjectIoError> {
    bind_store_else_refuse_corrupt_lock(root, store)
}

/// The accepted-catalog binding both read entry points share: the live store snapshot when a
/// valid stamped store is present; otherwise the first-run `None`, after failing closed on a
/// present-but-corrupt committed lock so a fresh mint can never silently override a committed
/// identity.
fn bind_store_else_refuse_corrupt_lock(
    root: &Path,
    store: Option<&TreeStore>,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ProjectIoError> {
    if let Some(store) = store
        && let Some(snapshot) = store.read_catalog_snapshot()?
    {
        return Ok(Some(snapshot));
    }
    read_committed_lock(root)?;
    Ok(None)
}

pub fn check_project_against(
    root: &Path,
    config: &ProjectConfig,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
) -> Result<CheckedProgram, ProjectIoError> {
    let (report, program) =
        crate::check_project_with_catalog(root, config, accepted).map_err(|error| {
            ProjectIoError::CheckLoad {
                code: error.code,
                path: error.path,
                message: error.message,
            }
        })?;
    if report.has_errors() {
        return Err(ProjectIoError::Check { report });
    }
    Ok(program)
}

pub fn check_source_project_analysis_against(
    root: &Path,
    config: &ProjectConfig,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
) -> Result<crate::AnalysisSnapshot, ProjectIoError> {
    let snapshot = crate::analysis::analyze_source_project(
        root,
        config,
        &crate::ProjectSources::new(),
        accepted,
        None,
    )
    .map_err(|error| ProjectIoError::CheckLoad {
        code: error.code,
        path: error.path,
        message: error.message,
    })?;
    if snapshot.report.has_errors() {
        return Err(ProjectIoError::Check {
            report: snapshot.report,
        });
    }
    Ok(snapshot)
}

pub fn recheck_against_store_catalog(
    root: &Path,
    config: &ProjectConfig,
    store: &TreeStore,
) -> Result<CheckedProgram, ProjectIoError> {
    recheck_source_project_analysis_against_store_catalog(root, config, store)
        .map(|snapshot| snapshot.program)
}

pub fn recheck_source_project_analysis_against_store_catalog(
    root: &Path,
    config: &ProjectConfig,
    store: &TreeStore,
) -> Result<crate::AnalysisSnapshot, ProjectIoError> {
    let accepted = store.read_catalog_snapshot()?;
    check_source_project_analysis_against(root, config, accepted.as_ref())
}

/// Re-project the committed store baseline into `marrow.lock`. This is the single owner of the
/// lock write: the post-commit re-projection runs only after a valid store write, never as a
/// read side effect, so a read never repairs the lock and a valid live store is never overridden
/// by a source-tree artifact. The shape, ledger, epoch high-water, and source digest are the
/// committed inputs the caller supplies; this fingerprints each entry through the [`CatalogLock`]
/// owner and writes the canonical projection, idempotent on the bytes a prior projection wrote.
pub fn project_store_lock(
    root: &Path,
    snapshot: &marrow_catalog::CatalogMetadata,
    ledger: &[marrow_catalog::LockLedgerTombstone],
    source_digest: &str,
) -> Result<(), ProjectIoError> {
    let entries = snapshot
        .entries
        .iter()
        .map(marrow_catalog::LockEntry::from_catalog_entry)
        .collect();
    let lock = marrow_catalog::CatalogLock::new(
        entries,
        ledger.to_vec(),
        snapshot.epoch,
        source_digest.to_string(),
    )
    .map_err(|error| ProjectIoError::Catalog {
        code: error.code,
        message: error.message,
    })?;
    let desired = lock
        .to_lock_json_pretty()
        .map_err(|error| ProjectIoError::Catalog {
            code: error.code,
            message: error.message,
        })?;
    let path = root.join(marrow_project::CATALOG_FILE_NAME);
    match fs::read_to_string(&path) {
        Ok(current) if current == desired => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ProjectIoError::Io {
                path: path.clone(),
                error,
            });
        }
    }
    fs::write(&path, desired).map_err(|error| ProjectIoError::Io { path, error })
}

/// The outcome of reading `marrow.lock` from a project root: absent, a valid decoded lock, a
/// present-but-corrupt artifact carrying its typed refusal, or an I/O failure. Lock structure is
/// decoded and validated by the [`CatalogLock`] owner; this only classifies the read.
enum LockRead {
    Missing,
    Lock(marrow_catalog::CatalogLock),
    Corrupt(marrow_catalog::CatalogError),
    ReadError {
        path: PathBuf,
        error: std::io::Error,
    },
}

fn read_lock_file(root: &Path) -> LockRead {
    let path = root.join(marrow_project::CATALOG_FILE_NAME);
    let json = match fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return LockRead::Missing,
        Err(error) => return LockRead::ReadError { path, error },
    };
    match marrow_catalog::CatalogLock::from_lock_json(&json) {
        Ok(lock) => LockRead::Lock(lock),
        Err(error) => LockRead::Corrupt(error),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use marrow_project::{ProjectConfig, StoreBackend, StoreConfig};

    use super::{CONFIG_DATA_DIR, ProjectIoError, native_store_path, resolve_store_path};

    fn native_config(data_dir: Option<&str>) -> ProjectConfig {
        ProjectConfig {
            source_roots: vec!["src".to_string()],
            default_entry: None,
            store: StoreConfig {
                backend: StoreBackend::Native,
                data_dir: data_dir.map(str::to_string),
            },
            tests: Vec::new(),
        }
    }

    fn assert_native_data_dir_error(error: ProjectIoError) {
        let ProjectIoError::Config { code, message } = error else {
            panic!("expected config error");
        };
        assert_eq!(code, marrow_project::CONFIG_INVALID);
        assert_eq!(
            message,
            "the `native` store backend requires a non-empty `dataDir`"
        );
    }

    #[test]
    fn native_store_path_rejects_missing_native_data_dir() {
        let error = native_store_path(Path::new("/project"), &native_config(None)).unwrap_err();

        assert_native_data_dir_error(error);
    }

    #[test]
    fn native_store_path_rejects_empty_native_data_dir() {
        let error = native_store_path(Path::new("/project"), &native_config(Some(""))).unwrap_err();

        assert_native_data_dir_error(error);
    }

    #[test]
    fn native_store_path_returns_configured_redb_file() {
        let path = native_store_path(Path::new("/project"), &native_config(Some(".data")))
            .expect("valid native store path");

        assert_eq!(path, Some(PathBuf::from("/project/.data/marrow.redb")));
    }

    #[test]
    fn resolve_store_path_propagates_native_data_dir_errors() {
        let error = resolve_store_path(Path::new("/project"), &native_config(None)).unwrap_err();

        assert_native_data_dir_error(error);
    }

    #[test]
    fn resolve_store_path_reports_a_dir_create_failure_as_a_config_fault() {
        // Creating the `dataDir` directory is a write-path operation owned here,
        // so any `create_dir_all` failure is a `config.data_dir` fault regardless
        // of its errno — never the `io.read` of an existing file. A project root
        // under a regular file fails the directory creation deterministically.
        let dir = std::env::temp_dir().join(format!(
            "marrow-datadir-create-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&dir, b"not a directory").expect("write occupying file");

        let error = resolve_store_path(&dir, &native_config(Some(".data"))).unwrap_err();
        std::fs::remove_file(&dir).ok();

        assert!(
            matches!(error, ProjectIoError::DataDirCreate { .. }),
            "a dataDir directory-creation failure is a typed config fault, not an Io read",
        );
        assert_eq!(error.code(), CONFIG_DATA_DIR);
        let message = error.message();
        assert!(
            message.contains("create") && message.contains("dataDir"),
            "the message names the directory it could not create: {message}"
        );
    }

    mod store_vs_lock {
        use std::fs;

        use marrow_catalog::{
            CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogLock, CatalogMetadata,
            LOCK_CORRUPT, LockEntry,
        };
        use marrow_store::tree::TreeStore;

        use crate::{
            project_store_lock, read_accepted_catalog_with_store,
            read_accepted_catalog_with_store_read_only,
        };
        use marrow_project::CATALOG_FILE_NAME;

        fn temp_root(name: &str) -> std::path::PathBuf {
            let root = std::env::temp_dir().join(format!(
                "marrow-store-vs-lock-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).expect("create project root");
            root
        }

        fn entry(kind: CatalogEntryKind, path: &str, suffix: u8) -> CatalogEntry {
            CatalogEntry {
                kind,
                path: path.to_string(),
                stable_id: format!("cat_{suffix:032x}"),
                aliases: Vec::new(),
                lifecycle: CatalogLifecycle::Active,
                accepted_key_shape: None,
                accepted_index_shape: None,
                accepted_struct: None,
            }
        }

        /// A real stamped store at `epoch`, holding a books resource + store entry. The
        /// snapshot is committed through the production store catalog writer, so the read
        /// path binds it exactly as it binds an on-disk store.
        fn stamped_store(epoch: u64) -> (TreeStore, CatalogMetadata) {
            let snapshot = CatalogMetadata::new(
                epoch,
                vec![
                    entry(CatalogEntryKind::Resource, "app::Book", 1),
                    entry(CatalogEntryKind::Store, "app::books", 2),
                ],
            )
            .expect("store snapshot builds");
            let store = TreeStore::memory();
            store.begin().expect("begin");
            store
                .replace_catalog_snapshot(&snapshot)
                .expect("commit catalog snapshot");
            store.commit().expect("commit");
            (store, snapshot)
        }

        /// A committed lock that DISAGREES with the stamped store: a higher epoch and a
        /// different stable-id set, so a read path that preferred the lock would return the
        /// lock's identity instead of the store's.
        fn disagreeing_lock() -> CatalogLock {
            CatalogLock::new(
                vec![
                    LockEntry::from_catalog_entry(&entry(
                        CatalogEntryKind::Resource,
                        "app::Book",
                        9,
                    )),
                    LockEntry::from_catalog_entry(&entry(
                        CatalogEntryKind::Store,
                        "app::books",
                        10,
                    )),
                ],
                Vec::new(),
                999,
                "sha256:".to_string() + &"0".repeat(64),
            )
            .expect("disagreeing lock builds")
        }

        fn id_set(snapshot: &CatalogMetadata) -> Vec<String> {
            let mut ids: Vec<String> = snapshot
                .entries
                .iter()
                .map(|entry| entry.stable_id.clone())
                .collect();
            ids.sort();
            ids
        }

        #[test]
        fn a_stale_lock_never_overrides_a_valid_live_store_and_corrupt_lock_fails_closed() {
            // Oracle (1): a valid stamped store AND a disagreeing on-disk lock bind from the
            // STORE through BOTH read entrypoints — the returned epoch and stable-id set are
            // the store snapshot's, never the lock's.
            let root = temp_root("store-wins");
            let (store, store_snapshot) = stamped_store(5);
            let lock = disagreeing_lock();
            let lock_path = root.join(CATALOG_FILE_NAME);
            fs::write(
                &lock_path,
                lock.to_lock_json_pretty().expect("lock renders"),
            )
            .expect("write lock");

            let read_only = read_accepted_catalog_with_store_read_only(&root, Some(&store))
                .expect("read-only bind")
                .expect("a bound snapshot");
            assert_eq!(
                read_only.epoch, store_snapshot.epoch,
                "read-only path binds the store epoch, not the lock's"
            );
            assert_eq!(
                id_set(&read_only),
                id_set(&store_snapshot),
                "read-only path binds the store id-set, not the lock's"
            );

            // Oracle (2): the binding read performs NO in-read write — the on-disk lock is
            // byte-identical across the read (today this path repairs/renders the file).
            let before = fs::read(&lock_path).expect("lock before");
            let bound = read_accepted_catalog_with_store(&root, Some(&store))
                .expect("bind")
                .expect("a bound snapshot");
            let after = fs::read(&lock_path).expect("lock after");
            assert_eq!(
                before, after,
                "read_accepted_catalog_with_store must not rewrite the lock during a read"
            );
            assert_eq!(
                bound.epoch, store_snapshot.epoch,
                "the store-store path binds the store epoch, not the lock's"
            );
            assert_eq!(id_set(&bound), id_set(&store_snapshot));

            // A SEPARATE post-commit re-projection path DOES rewrite the lock from the
            // committed store baseline: it overwrites the disagreeing on-disk lock with a
            // valid projection of the committed store snapshot, parseable as a CatalogLock
            // (not catalog JSON).
            project_store_lock(&root, &store_snapshot, &[], &store_snapshot.digest)
                .expect("re-project the committed store lock");
            let reprojected = fs::read_to_string(&lock_path).expect("lock after re-projection");
            let projected = CatalogLock::from_lock_json(&reprojected)
                .expect("re-projection writes a valid lock projection, not catalog JSON");
            assert_eq!(
                projected.epoch_high_water, store_snapshot.epoch,
                "the re-projected lock carries the committed store epoch"
            );

            fs::remove_dir_all(&root).ok();

            // Oracle (3): an EMPTY store and a CORRUPT on-disk lock FAIL CLOSED with the typed
            // lock_corrupt code — never Ok(None), never silent fresh minting.
            let corrupt_root = temp_root("corrupt-lock");
            fs::write(
                corrupt_root.join(CATALOG_FILE_NAME),
                "{ this is not a valid lock",
            )
            .expect("write corrupt lock");
            let empty_store = TreeStore::memory();
            let error = read_accepted_catalog_with_store(&corrupt_root, Some(&empty_store))
                .expect_err("a corrupt lock over an empty store fails closed");
            assert_eq!(
                error.code(),
                LOCK_CORRUPT,
                "a corrupt lock surfaces the typed lock_corrupt code"
            );
            fs::remove_dir_all(&corrupt_root).ok();
        }
    }
}
