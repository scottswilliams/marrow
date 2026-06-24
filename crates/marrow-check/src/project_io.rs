//! Project file, catalog artifact, and check-loading helpers.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_project::{ProjectConfig, StoreBackend, StoreConfig};
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use crate::{CheckReport, CheckedProgram};

/// The native store `dataDir` directory could not be created.
pub const CONFIG_DATA_DIR: &str = "config.data_dir";

/// No `marrow.json` was found at the project directory: the path is not a Marrow project.
pub const CONFIG_MISSING: &str = "config.missing";

/// The project path is a bare file, not a directory containing `marrow.json`.
pub const CONFIG_NOT_A_PROJECT: &str = "config.not_a_project";

#[derive(Debug)]
pub enum ProjectIoError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// No `marrow.json` exists at the project directory. This is the everyday "wrong directory or
    /// not yet initialized" mistake, distinct from a file that exists but cannot be read, so it
    /// carries a missing-project remedy rather than a raw read fault.
    ConfigMissing {
        dir: PathBuf,
    },
    /// The project path is a bare file, not a directory containing `marrow.json` (reading
    /// `<file>/marrow.json` fails with a non-directory error). This is distinct from a directory
    /// missing `marrow.json`: the `marrow init` remedy does not apply, since a bare file cannot be
    /// turned into a project in place, so it names the mistake instead of pointing at init.
    NotAProject {
        path: PathBuf,
    },
    /// The native store's `dataDir` directory could not be created: the path is
    /// occupied by a non-directory file, a parent denies access, or the
    /// filesystem is read-only. This is a write-path (directory-creation) fault,
    /// distinct from a read of an existing file, so it never carries `io.read`.
    /// The fault carries a typed condition rather than the raw OS error, so the
    /// surfaced message names what went wrong without leaking an `os error N`.
    DataDirCreate {
        path: PathBuf,
        fault: DataDirFault,
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

/// Why the native store's `dataDir` directory could not be created or opened, in clean
/// prose. This classifies the underlying OS condition once so neither the write path's
/// `create_dir_all` failure nor the read path's inspection guard surfaces a raw `os error N`:
/// machine-readable identity is the `config.data_dir` code, and the message names the
/// condition itself. Conditions that do not map to a known cause fall back to the generic
/// non-directory occupant, which is the overwhelmingly common cause of a failed creation.
#[derive(Debug, Clone, Copy)]
pub enum DataDirFault {
    /// The path is occupied by a file, symlink, or other non-directory entry.
    Occupied,
    /// A parent directory denies the access needed to create or read the directory.
    PermissionDenied,
    /// The filesystem holding the path is mounted read-only.
    ReadOnly,
}

impl DataDirFault {
    fn classify(error: &std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            std::io::ErrorKind::ReadOnlyFilesystem => Self::ReadOnly,
            _ => Self::Occupied,
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Occupied => "the path is occupied by a non-directory file",
            Self::PermissionDenied => "a parent directory denies access",
            Self::ReadOnly => "the filesystem is read-only",
        }
    }
}

impl ProjectIoError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => crate::IO_READ,
            Self::ConfigMissing { .. } => CONFIG_MISSING,
            Self::NotAProject { .. } => CONFIG_NOT_A_PROJECT,
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
            Self::ConfigMissing { dir } => format!(
                "no marrow.json in {}; this is not a Marrow project. \
                 Run marrow init {}, or run from a directory containing marrow.json",
                dir.display(),
                dir.display()
            ),
            Self::NotAProject { path } => format!(
                "{} is a bare file, not a project directory containing marrow.json; \
                 pass the project directory, or run from a directory containing marrow.json",
                path.display()
            ),
            Self::DataDirCreate { path, fault } => format!(
                "cannot create the native store `dataDir` directory {}: {}; \
                 point `dataDir` at a writable directory or remove the file occupying it",
                path.display(),
                fault.describe()
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
    let json = fs::read_to_string(&path).map_err(|error| {
        // Every command resolves a project through this loader, so the two everyday "not a Marrow
        // project" mistakes are classified here once rather than re-guarded per command. A
        // directory with no `marrow.json` carries the missing-project remedy (`marrow init`); a
        // bare file passed where a directory is expected (the join reads through a non-directory)
        // carries the distinct not-a-project message, since `marrow init` cannot turn a file into
        // a project in place. Both keep the raw `ENOTDIR`/`ENOENT` errno out of an `io.read` leak.
        match error.kind() {
            std::io::ErrorKind::NotFound => ProjectIoError::ConfigMissing {
                dir: root.to_path_buf(),
            },
            std::io::ErrorKind::NotADirectory => ProjectIoError::NotAProject {
                path: root.to_path_buf(),
            },
            _ => ProjectIoError::Io {
                path: path.clone(),
                error,
            },
        }
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
            fault: DataDirFault::classify(&error),
        })?;
    }
    Ok(Some(path))
}

/// Classify a native `dataDir` occupied by a non-directory file as the same
/// `config.data_dir` fault the write path raises, before a read-only inspection
/// tries to open a store under it.
///
/// The read path never creates the directory, so it cannot lean on `create_dir_all`
/// to surface the condition; left to the store open, the stray file's `ENOTDIR`
/// would leak as a raw `store.io` errno. This shares the directory-creation fault
/// owner so every read-only inspection reports the occupied `dataDir` exactly as
/// `run` does. A directory or an absent path is left to the open: a present store is
/// read, and an absent one is the empty first run.
pub fn guard_data_dir(root: &Path, config: &ProjectConfig) -> Result<(), ProjectIoError> {
    let Some(path) = native_store_path(root, config)? else {
        return Ok(());
    };
    let Some(data_dir) = path.parent() else {
        return Ok(());
    };
    match fs::metadata(data_dir) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        // A present non-directory, an inaccessible parent, or a parent component that
        // is itself a file: each is the directory the write path could not create.
        Ok(_) => Err(ProjectIoError::DataDirCreate {
            path: data_dir.to_path_buf(),
            fault: DataDirFault::Occupied,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ProjectIoError::DataDirCreate {
            path: data_dir.to_path_buf(),
            fault: DataDirFault::classify(&error),
        }),
    }
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

/// Check the project against an optional accepted catalog and an optional committed lock,
/// returning the runtime-lowered program. When no accepted store snapshot is present, the
/// committed lock drives first-run adoption so a fresh checkout over an empty store re-establishes
/// the lock's committed identity and accepted epoch rather than minting fresh ids. With an accepted
/// snapshot present, the store wins and the lock is inert.
pub fn check_project_against(
    root: &Path,
    config: &ProjectConfig,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<CheckedProgram, ProjectIoError> {
    let snapshot = crate::analysis::analyze_project(
        root,
        config,
        &crate::ProjectSources::new(),
        accepted,
        lock,
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
    Ok(snapshot.program)
}

/// Check the source project against an optional accepted catalog and an optional committed lock.
/// When no accepted store snapshot is present, the committed lock drives first-run adoption so a
/// fresh checkout over an empty store re-establishes the lock's committed identity rather than
/// minting fresh ids. With an accepted snapshot present, the store wins and the lock is inert.
pub fn check_source_project_analysis_against(
    root: &Path,
    config: &ProjectConfig,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
    lock: Option<&marrow_catalog::CatalogLock>,
) -> Result<crate::AnalysisSnapshot, ProjectIoError> {
    let snapshot = crate::analysis::analyze_source_project(
        root,
        config,
        &crate::ProjectSources::new(),
        accepted,
        lock,
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
    check_source_project_analysis_against(root, config, accepted.as_ref(), None)
}

/// Re-project the committed store baseline into `marrow.lock`. This is the single owner of the
/// lock write: the post-commit re-projection runs only after a valid store write, never as a
/// read side effect, so a read never repairs the lock and a valid live store is never overridden
/// by a source-tree artifact.
///
/// The store snapshot is the durable authority for accepted identity, including retired ids: a
/// retire reserves the entry in the store catalog, so the lock's append-only id ledger is derived
/// from the snapshot's reserved entries rather than carried forward from a prior lock file. This
/// makes the ledger re-derivable from the store alone, so a deleted lock is recovered with its
/// retired ids and a retired id is never reissued even across lock loss. Active entries project as
/// lock entries; reserved entries project as ledger tombstones, never both, so the same id never
/// appears as an active entry and a tombstone.
///
/// The epoch high-water and the retired-id ledger are monotonic and append-only by contract, so
/// the projection never regresses an ahead committed lock from a behind local store: it reads the
/// existing committed lock, takes the higher of the lock's high-water and the snapshot's epoch, and
/// unions the lock's previously-committed tombstones with the snapshot-derived ones. Without this a
/// local store behind an ahead lock (a teammate's already-committed activation, replayed from a
/// fresh checkout's seed) would rewind the high-water and erase a tombstone, letting a later
/// checkout reissue a retired id to a different entity. A tombstone the snapshot has since promoted
/// back to an active id is not re-added, so the union never resurrects an id the store now holds
/// live. The projection writes the canonical lock atomically (temp file, fsync, rename, parent
/// fsync) so a torn write can never leave a corrupt lock, and is idempotent on the bytes a
/// converged lock holds.
pub fn project_store_lock(
    root: &Path,
    snapshot: &marrow_catalog::CatalogMetadata,
    source_digest: &str,
) -> Result<LockProjection, ProjectIoError> {
    let existing = read_committed_lock(root)?;
    let entries: Vec<marrow_catalog::LockEntry> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.lifecycle == marrow_catalog::CatalogLifecycle::Active)
        .map(marrow_catalog::LockEntry::from_catalog_entry)
        .collect();
    let ledger = union_committed_ledger(existing.as_ref(), snapshot, &entries);
    let epoch_high_water = existing.as_ref().map_or(snapshot.epoch, |lock| {
        lock.epoch_high_water.max(snapshot.epoch)
    });
    let lock = marrow_catalog::CatalogLock::new(
        entries,
        ledger,
        epoch_high_water,
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
    let projection = match fs::read_to_string(&path) {
        Ok(current) if current == desired => return Ok(LockProjection::Unchanged),
        Ok(_) => LockProjection::Updated,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LockProjection::Created,
        Err(error) => {
            return Err(ProjectIoError::Io {
                path: path.clone(),
                error,
            });
        }
    };
    write_lock_atomically(&path, &desired)?;
    Ok(projection)
}

/// What a `marrow.lock` projection did to the on-disk file. The write path surfaces this to the
/// developer so the otherwise-invisible lock lifecycle is announced once: a first run that creates
/// the lock teaches that the file exists and must be committed; a re-projection that rewrites it
/// teaches that the committed lock changed and must be re-committed. An idempotent no-op stays
/// silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockProjection {
    /// The on-disk lock already matched the projection; nothing was written.
    Unchanged,
    /// No lock existed on disk; the projection created it.
    Created,
    /// A different lock existed on disk; the projection rewrote it.
    Updated,
}

/// Union the snapshot's reserved tombstones with the committed lock's, so an append-only ledger
/// never drops an id a prior projection recorded. A snapshot-derived tombstone wins on its id so a
/// re-projection records the snapshot's authoritative high-water for an id both hold; a committed
/// tombstone is carried forward only when the snapshot neither reserves it nor promotes it back to
/// an active entry, keeping a retired id reserved across store loss without resurrecting one the
/// store now holds live.
fn union_committed_ledger(
    existing: Option<&marrow_catalog::CatalogLock>,
    snapshot: &marrow_catalog::CatalogMetadata,
    active: &[marrow_catalog::LockEntry],
) -> Vec<marrow_catalog::LockLedgerTombstone> {
    let mut ledger: Vec<marrow_catalog::LockLedgerTombstone> = snapshot
        .entries
        .iter()
        .filter(|entry| entry.lifecycle != marrow_catalog::CatalogLifecycle::Active)
        .map(|entry| {
            marrow_catalog::LockLedgerTombstone::from_reserved_entry(entry, snapshot.epoch)
        })
        .collect();
    let snapshot_ids: std::collections::HashSet<String> =
        ledger.iter().map(|stone| stone.id.clone()).collect();
    let active_ids: std::collections::HashSet<&str> = active
        .iter()
        .map(|entry| entry.stable_id.as_str())
        .collect();
    if let Some(existing) = existing {
        for stone in &existing.ledger {
            if !snapshot_ids.contains(&stone.id) && !active_ids.contains(stone.id.as_str()) {
                ledger.push(stone.clone());
            }
        }
    }
    ledger
}

/// Write the committed lock atomically: render to a sibling temp file in the same directory, fsync
/// its contents, rename it over the target, then fsync the parent directory. A rename within a
/// directory is atomic on the target filesystems, so a crash mid-write leaves either the prior lock
/// or the new one, never a torn projection a reader would reject as corrupt. Fsyncing the parent
/// after the rename persists the directory entry itself, so a host crash right after the rename
/// cannot lose the renamed file.
fn write_lock_atomically(path: &Path, contents: &str) -> Result<(), ProjectIoError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| marrow_project::CATALOG_FILE_NAME.to_string());
    let temp = dir.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        next_lock_temp_nonce()
    ));
    let io_error = |error: std::io::Error| ProjectIoError::Io {
        path: temp.clone(),
        error,
    };
    let mut file = fs::File::create(&temp).map_err(io_error)?;
    use std::io::Write;
    file.write_all(contents.as_bytes()).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    if let Err(error) = fs::rename(&temp, path) {
        fs::remove_file(&temp).ok();
        return Err(ProjectIoError::Io {
            path: path.to_path_buf(),
            error,
        });
    }
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    fs::File::open(dir)
        .and_then(|handle| handle.sync_all())
        .map_err(|error| ProjectIoError::Io {
            path: dir.to_path_buf(),
            error,
        })?;
    Ok(())
}

/// A per-write nonce that, with the process id, keeps concurrent or rapid successive lock writes
/// from colliding on the same temp path before the atomic rename.
fn next_lock_temp_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
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
        // A missing lock, or a project path that is itself a bare file (so the lock cannot exist
        // under a non-directory) is an absent lock — a true first run or a not-a-project path the
        // config loader already classifies — never a corrupt lock or a raw read fault to surface.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return LockRead::Missing;
        }
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

    use super::{
        CONFIG_DATA_DIR, ProjectIoError, guard_data_dir, native_store_path, resolve_store_path,
    };

    fn native_config(data_dir: Option<&str>) -> ProjectConfig {
        ProjectConfig {
            source_roots: vec!["src".to_string()],
            default_entry: None,
            store: StoreConfig {
                backend: StoreBackend::Native,
                data_dir: data_dir.map(str::to_string),
            },
            tests: Vec::new(),
            client: None,
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
        assert!(
            !message.contains("os error") && message.contains("occupied by a non-directory file"),
            "the write path emits clean prose, never a raw OS errno: {message}"
        );
    }

    #[test]
    fn guard_data_dir_classifies_a_non_directory_as_a_config_fault() {
        // The read-only inspection guard never creates the directory, so it must
        // detect a `dataDir` occupied by a regular file itself and raise the same
        // `config.data_dir` fault the write path does, without leaking a raw OS errno.
        let root = std::env::temp_dir().join(format!(
            "marrow-guard-datadir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).expect("create project root");
        std::fs::write(root.join(".data"), b"not a directory").expect("occupy dataDir");

        let error = guard_data_dir(&root, &native_config(Some(".data"))).unwrap_err();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(error.code(), CONFIG_DATA_DIR);
        let message = error.message();
        assert!(
            message.contains("dataDir") && !message.contains("os error"),
            "the message names the dataDir and leaks no errno: {message}"
        );
    }

    #[test]
    fn guard_data_dir_accepts_an_absent_or_present_directory() {
        let root = std::env::temp_dir().join(format!(
            "marrow-guard-datadir-ok-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).expect("create project root");
        let config = native_config(Some(".data"));

        // Absent dataDir: the empty first run, left to the open.
        guard_data_dir(&root, &config).expect("absent dataDir is accepted");
        // A real directory is accepted too.
        std::fs::create_dir(root.join(".data")).expect("create dataDir");
        guard_data_dir(&root, &config).expect("present dataDir is accepted");
        std::fs::remove_dir_all(&root).ok();
    }

    mod store_vs_lock {
        use std::fs;

        use marrow_catalog::{
            CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogLock, CatalogMetadata,
            LOCK_CORRUPT, LockEntry, LockLedgerTombstone,
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
                applied_transform: None,
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
            // (not catalog JSON). The store is the identity authority, so the projected active
            // entries are the store's id-set, not the disagreeing lock's.
            project_store_lock(&root, &store_snapshot, &store_snapshot.digest)
                .expect("re-project the committed store lock");
            let reprojected = fs::read_to_string(&lock_path).expect("lock after re-projection");
            let projected = CatalogLock::from_lock_json(&reprojected)
                .expect("re-projection writes a valid lock projection, not catalog JSON");
            let mut projected_ids: Vec<String> = projected
                .entries
                .iter()
                .map(|entry| entry.stable_id.clone())
                .collect();
            projected_ids.sort();
            assert_eq!(
                projected_ids,
                id_set(&store_snapshot),
                "the re-projected active entries bind the store id-set, not the disagreeing lock's"
            );
            // The high-water is monotonic: the disagreeing lock claimed epoch 999, above the
            // store's 5, so the projection holds the higher value rather than rewinding it. A
            // store ahead of the lock advances the high-water; a store behind it never rewinds it.
            assert_eq!(
                projected.epoch_high_water,
                store_snapshot.epoch.max(999),
                "re-projection takes the max of the committed lock's high-water and the store epoch, never regresses"
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

        /// A retired id committed as a ledger tombstone at a high epoch. A fresh checkout seeded
        /// from this lock reserves the id so it is never reissued.
        fn tombstone(suffix: u8, high_water: u64) -> LockLedgerTombstone {
            let reserved = CatalogEntry {
                kind: CatalogEntryKind::Resource,
                path: format!("app::Retired{suffix}"),
                stable_id: format!("cat_{suffix:032x}"),
                aliases: Vec::new(),
                lifecycle: CatalogLifecycle::Reserved,
                accepted_key_shape: None,
                accepted_index_shape: None,
                accepted_struct: None,
                applied_transform: None,
            };
            LockLedgerTombstone::from_reserved_entry(&reserved, high_water)
        }

        #[test]
        fn re_projecting_an_older_snapshot_never_regresses_an_ahead_committed_lock() {
            // A committed lock ahead of the local store: high-water epoch 9 with a retired-id
            // tombstone. This is what a teammate already activated and committed.
            let root = temp_root("ahead-lock-no-regress");
            let ahead = CatalogLock::new(
                vec![LockEntry::from_catalog_entry(&entry(
                    CatalogEntryKind::Resource,
                    "app::Book",
                    1,
                ))],
                vec![tombstone(7, 9)],
                9,
                "sha256:".to_string() + &"0".repeat(64),
            )
            .expect("ahead lock builds");
            let lock_path = root.join(CATALOG_FILE_NAME);
            fs::write(
                &lock_path,
                ahead.to_lock_json_pretty().expect("ahead lock renders"),
            )
            .expect("write ahead lock");

            // The local store snapshot is BEHIND: epoch 2, and it has never seen the retired id,
            // so a projection built purely from the snapshot would carry epoch 2 and an empty
            // ledger.
            let behind =
                CatalogMetadata::new(2, vec![entry(CatalogEntryKind::Resource, "app::Book", 1)])
                    .expect("behind snapshot builds");

            project_store_lock(&root, &behind, &behind.digest)
                .expect("re-project over the ahead lock");

            let written = fs::read_to_string(&lock_path).expect("lock after re-projection");
            let result =
                CatalogLock::from_lock_json(&written).expect("re-projection writes a valid lock");

            // The high-water is monotonic: it must never rewind below the committed lock's.
            assert_eq!(
                result.epoch_high_water, 9,
                "epoch_high_water must take the max of the committed lock and the snapshot, \
                 never regress to the older snapshot's epoch"
            );
            // The retired-id ledger is append-only: a previously-committed tombstone must survive
            // even though the older snapshot never reserved it, or a fresh checkout could reissue
            // the retired id to a different entity.
            assert!(
                result
                    .ledger
                    .iter()
                    .any(|stone| stone.id == format!("cat_{:032x}", 7)),
                "a previously-committed retired-id tombstone must never be dropped by re-projection"
            );
        }

        #[test]
        fn lock_is_written_atomically_and_leaves_no_temp_artifact() {
            // The projection writes through a sibling temp file and an atomic rename, so a torn
            // write can never leave a corrupt lock. After a successful projection the directory
            // holds the canonical lock and no leftover temp file: the rename consumed it.
            let root = temp_root("atomic-write");
            let snapshot =
                CatalogMetadata::new(3, vec![entry(CatalogEntryKind::Resource, "app::Book", 1)])
                    .expect("snapshot builds");

            project_store_lock(&root, &snapshot, &snapshot.digest).expect("project lock");

            let lock_path = root.join(CATALOG_FILE_NAME);
            let written = fs::read_to_string(&lock_path).expect("lock present after write");
            CatalogLock::from_lock_json(&written).expect("a complete, valid lock was written");

            let leftover: Vec<String> = fs::read_dir(&root)
                .expect("read project dir")
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name != CATALOG_FILE_NAME)
                .collect();
            assert!(
                leftover.is_empty(),
                "the atomic rename must leave no temp artifact behind: {leftover:?}"
            );

            // A second projection of identical bytes is idempotent: it short-circuits on the
            // matching on-disk bytes and still leaves no temp file.
            project_store_lock(&root, &snapshot, &snapshot.digest).expect("re-project lock");
            let leftover_again: Vec<String> = fs::read_dir(&root)
                .expect("read project dir")
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name != CATALOG_FILE_NAME)
                .collect();
            assert!(
                leftover_again.is_empty(),
                "an idempotent re-projection leaves no temp artifact: {leftover_again:?}"
            );

            fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn re_projection_unions_tombstones_and_takes_the_higher_epoch() {
            // The committed lock carries one retired id at high-water 9; the snapshot carries a
            // different, newer reserved id at its own epoch 4. The union must keep both, and the
            // high-water must be the max of the two.
            let root = temp_root("union-tombstones");
            let committed = CatalogLock::new(
                Vec::new(),
                vec![tombstone(7, 9)],
                9,
                "sha256:".to_string() + &"0".repeat(64),
            )
            .expect("committed lock builds");
            let lock_path = root.join(CATALOG_FILE_NAME);
            fs::write(
                &lock_path,
                committed
                    .to_lock_json_pretty()
                    .expect("committed lock renders"),
            )
            .expect("write committed lock");

            let snapshot = CatalogMetadata::new(
                4,
                vec![CatalogEntry {
                    kind: CatalogEntryKind::Resource,
                    path: "app::Dropped8".to_string(),
                    stable_id: format!("cat_{:032x}", 8),
                    aliases: Vec::new(),
                    lifecycle: CatalogLifecycle::Reserved,
                    accepted_key_shape: None,
                    accepted_index_shape: None,
                    accepted_struct: None,
                    applied_transform: None,
                }],
            )
            .expect("snapshot builds");

            project_store_lock(&root, &snapshot, &snapshot.digest)
                .expect("re-project unions the ledger");

            let written = fs::read_to_string(&lock_path).expect("lock after re-projection");
            let result = CatalogLock::from_lock_json(&written).expect("valid lock");

            assert_eq!(result.epoch_high_water, 9, "high-water is the max of both");
            let ids: Vec<&str> = result
                .ledger
                .iter()
                .map(|stone| stone.id.as_str())
                .collect();
            assert!(
                ids.contains(&format!("cat_{:032x}", 7).as_str())
                    && ids.contains(&format!("cat_{:032x}", 8).as_str()),
                "the union keeps both the committed and snapshot-derived tombstones: {ids:?}"
            );
        }
    }
}
