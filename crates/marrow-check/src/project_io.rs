//! Project file, catalog artifact, and check-loading helpers.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_project::{ProjectConfig, StoreBackend, StoreConfig};
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use crate::{CheckReport, CheckedProgram};

#[derive(Debug)]
pub enum ProjectIoError {
    Io {
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
        fs::create_dir_all(parent).map_err(|error| ProjectIoError::Io {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    Ok(Some(path))
}

pub fn read_accepted_catalog_artifact(
    root: &Path,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ProjectIoError> {
    accepted_catalog_file_result(read_accepted_catalog_file(root), None)
}

pub fn read_accepted_catalog_with_store(
    root: &Path,
    store: Option<&TreeStore>,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ProjectIoError> {
    let file_accepted = read_accepted_catalog_file(root);
    if let AcceptedCatalogFile::Invalid(error) = &file_accepted
        && error.code == marrow_catalog::CATALOG_MERGE_CONFLICT
    {
        return Err(ProjectIoError::Catalog {
            code: error.code,
            message: error.message.clone(),
        });
    }
    let Some(store) = store else {
        return accepted_catalog_file_result(file_accepted, None);
    };
    let accepted = store.read_catalog_snapshot()?;
    if let Some(snapshot) = &accepted
        && store_snapshot_repairs_file(&file_accepted, snapshot)
    {
        render_accepted_catalog_file(root, snapshot)?;
        return Ok(accepted);
    }
    accepted_catalog_file_result(file_accepted, accepted)
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

pub fn recheck_against_store_catalog(
    root: &Path,
    config: &ProjectConfig,
    store: &TreeStore,
) -> Result<CheckedProgram, ProjectIoError> {
    let accepted = store.read_catalog_snapshot()?;
    if let Some(snapshot) = &accepted {
        render_accepted_catalog_file(root, snapshot)?;
    }
    check_project_against(root, config, accepted.as_ref())
}

pub fn render_accepted_catalog_file(
    root: &Path,
    snapshot: &marrow_catalog::CatalogMetadata,
) -> Result<(), ProjectIoError> {
    let path = root.join(marrow_project::CATALOG_FILE_NAME);
    let desired = snapshot.to_json_pretty();
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

enum AcceptedCatalogFile {
    Missing,
    Snapshot(marrow_catalog::CatalogMetadata),
    Invalid(marrow_catalog::CatalogError),
    ReadError {
        path: PathBuf,
        error: std::io::Error,
    },
}

fn read_accepted_catalog_file(root: &Path) -> AcceptedCatalogFile {
    let path = root.join(marrow_project::CATALOG_FILE_NAME);
    let json = match fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return AcceptedCatalogFile::Missing;
        }
        Err(error) => return AcceptedCatalogFile::ReadError { path, error },
    };
    match marrow_catalog::CatalogMetadata::from_json(&json) {
        Ok(snapshot) => AcceptedCatalogFile::Snapshot(snapshot),
        Err(error) => AcceptedCatalogFile::Invalid(error),
    }
}

fn accepted_catalog_file_result(
    file_accepted: AcceptedCatalogFile,
    fallback: Option<marrow_catalog::CatalogMetadata>,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ProjectIoError> {
    match file_accepted {
        AcceptedCatalogFile::Missing => Ok(fallback),
        AcceptedCatalogFile::Snapshot(snapshot) => Ok(Some(snapshot)),
        AcceptedCatalogFile::Invalid(error) => Err(ProjectIoError::Catalog {
            code: error.code,
            message: error.message,
        }),
        AcceptedCatalogFile::ReadError { path, error } => Err(ProjectIoError::Io { path, error }),
    }
}

fn store_snapshot_repairs_file(
    file_accepted: &AcceptedCatalogFile,
    store_snapshot: &marrow_catalog::CatalogMetadata,
) -> bool {
    match file_accepted {
        AcceptedCatalogFile::Missing | AcceptedCatalogFile::Invalid(_) => true,
        AcceptedCatalogFile::ReadError { error, .. } => {
            error.kind() == std::io::ErrorKind::InvalidData
        }
        AcceptedCatalogFile::Snapshot(file) => {
            file != store_snapshot && store_snapshot.epoch >= file.epoch
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use marrow_project::{ProjectConfig, StoreBackend, StoreConfig};

    use super::{ProjectIoError, native_store_path, resolve_store_path};

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
}
