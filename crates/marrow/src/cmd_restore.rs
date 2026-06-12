//! `marrow restore`: replay a typed backup into an empty native store.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::BufReader;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_run::SystemNondeterminism;
use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::backup::{
    BackupError, BackupPrologue, read_backup_prologue, restore_backup_with_prologue,
};
use crate::{
    CheckFormat, dir_and_path_args, load_config_with_format, native_store_path, report_io_error,
    report_project, report_simple_error, resolve_store_path, write_json,
};

pub(crate) fn restore(args: &[String]) -> ExitCode {
    let (dir, input, format) = match dir_and_path_args("restore", "backup-file", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let config = match load_config_with_format(&dir, format) {
        Ok(config) => config,
        Err(code) => return code,
    };

    let file = match File::open(&input) {
        Ok(file) => file,
        Err(error) => {
            report_simple_error(
                "io.read",
                &format!("could not open {input}: {error}"),
                format,
            );
            return ExitCode::FAILURE;
        }
    };
    let mut reader = BufReader::new(file);
    let prologue = match read_backup_prologue(&mut reader) {
        Ok(prologue) => prologue,
        Err(error) => {
            return report_backup_error(error, format);
        }
    };

    if let Err(code) = reject_current_catalog_mismatch(&dir, &prologue, format) {
        return code;
    }

    // Restore binds the project against the catalog the backup carries, so the replayed
    // data validates against the same accepted identity the original store wrote it under
    // rather than a freshly proposed baseline.
    let program = match check_against_backup_catalog(&dir, &config, &prologue, format) {
        Ok(program) => program,
        Err(code) => return code,
    };

    // Restore needs a durable target. An in-memory project has nowhere to write to.
    let target_files = match RestoreTargetFiles::capture(&dir, &config) {
        Ok(files) => files,
        Err(code) => return code,
    };
    let path = match resolve_store_path(&dir, &config, format) {
        Ok(Some(path)) => path,
        Ok(None) => {
            report_simple_error(
                "config.invalid",
                "restore requires a native store backend with a dataDir",
                format,
            );
            return ExitCode::FAILURE;
        }
        Err(code) => return code,
    };
    let store = match TreeStore::open(&path) {
        Ok(store) => store,
        Err(error) => {
            target_files.cleanup_created();
            report_simple_error(error.code(), &error.to_string(), format);
            return ExitCode::FAILURE;
        }
    };

    // Restore validates the whole replayed store before commit, including orphan
    // cells under dropped roots or members, against the restored catalog.
    let verify = |restore_program: &marrow_check::CheckedProgram, store: &TreeStore| {
        match marrow_check::tooling::count_activation_integrity_problems(store, restore_program) {
            Ok((_, 0)) => Ok(()),
            Ok((_, problems)) => Err(BackupError::DataInvalid(format!(
                "restored data has {problems} schema problem(s); the backup does not match this project"
            ))),
            Err(error) => Err(BackupError::Store(error)),
        }
    };

    let mut nondeterminism = SystemNondeterminism::new();
    match restore_backup_with_prologue(
        &program,
        &store,
        prologue,
        &mut reader,
        &mut nondeterminism,
        verify,
    ) {
        Ok(report) => {
            match format {
                CheckFormat::Text => {
                    println!(
                        "ok: restored {} record(s) from {input}",
                        report.record_count
                    );
                }
                CheckFormat::Json | CheckFormat::Jsonl => write_json(json!({
                    "input": input,
                    "records": report.record_count,
                    "catalog_epoch": report.catalog_epoch,
                })),
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            drop(store);
            target_files.cleanup_created();
            report_backup_error(error, format)
        }
    }
}

fn report_backup_error(error: BackupError, format: CheckFormat) -> ExitCode {
    report_simple_error(error.code(), &error.to_string(), format);
    ExitCode::FAILURE
}

fn reject_current_catalog_mismatch(
    dir: &str,
    prologue: &BackupPrologue,
    format: CheckFormat,
) -> Result<(), ExitCode> {
    let Some(current) = read_source_tree_catalog(dir, format)? else {
        return Ok(());
    };
    if catalog_fingerprint(Some(&current)) == catalog_fingerprint(prologue.catalog()) {
        return Ok(());
    }
    Err(report_backup_error(
        BackupError::CatalogMismatch(
            "backup catalog does not match this project's accepted catalog".to_string(),
        ),
        format,
    ))
}

fn read_source_tree_catalog(
    dir: &str,
    format: CheckFormat,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ExitCode> {
    let path = Path::new(dir).join("marrow.catalog.json");
    let json = match fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            report_io_error(&path.display().to_string(), &error, format);
            return Err(ExitCode::FAILURE);
        }
    };
    marrow_catalog::CatalogMetadata::from_json(&json)
        .map(Some)
        .map_err(|error| {
            report_simple_error(error.code, &error.message, format);
            ExitCode::FAILURE
        })
}

fn catalog_fingerprint(catalog: Option<&marrow_catalog::CatalogMetadata>) -> Option<(u64, &str)> {
    catalog.map(|catalog| (catalog.epoch, catalog.digest.as_str()))
}

struct RestoreTargetFiles {
    store_path: Option<StorePathSnapshot>,
    parent_path: Option<PathBuf>,
    parent_entry_existed: bool,
}

struct StorePathSnapshot {
    cleanup_path: Option<PathBuf>,
}

impl RestoreTargetFiles {
    fn capture(
        dir: &str,
        config: &marrow_project::ProjectConfig,
    ) -> Result<RestoreTargetFiles, ExitCode> {
        let Some(path) = native_store_path(dir, config)? else {
            return Ok(RestoreTargetFiles {
                store_path: None,
                parent_path: None,
                parent_entry_existed: true,
            });
        };
        let parent_path = path.parent().map(Path::to_path_buf);
        let parent_entry_existed = parent_path
            .as_ref()
            .is_none_or(|parent| path_entry_existed(parent));
        Ok(RestoreTargetFiles {
            store_path: Some(StorePathSnapshot::capture(path)),
            parent_path,
            parent_entry_existed,
        })
    }

    fn cleanup_created(&self) {
        if let Some(store_path) = &self.store_path {
            store_path.cleanup_created();
        }
        if !self.parent_entry_existed
            && let Some(path) = &self.parent_path
        {
            let _ = fs::remove_dir(path);
        }
    }
}

impl StorePathSnapshot {
    fn capture(path: PathBuf) -> StorePathSnapshot {
        StorePathSnapshot {
            cleanup_path: missing_store_file_target(path),
        }
    }

    fn cleanup_created(&self) {
        if let Some(path) = &self.cleanup_path {
            remove_created_store_file(path);
        }
    }
}

fn path_entry_existed(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn missing_store_file_target(mut path: PathBuf) -> Option<PathBuf> {
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(path.clone()) {
            return None;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Some(path),
            Err(_) => return None,
        };
        if !metadata.file_type().is_symlink() {
            return None;
        }
        let target = fs::read_link(&path).ok()?;
        path = resolve_link_target(&path, target);
    }
}

fn resolve_link_target(link_path: &Path, target: PathBuf) -> PathBuf {
    if target.is_absolute() {
        target
    } else {
        link_path
            .parent()
            .map_or_else(|| target.clone(), |parent| parent.join(&target))
    }
}

fn remove_created_store_file(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_file() {
        let _ = fs::remove_file(path);
    }
}

/// Check the project bound to the catalog the backup carries. Source-text and accepted-catalog
/// mismatches are reported through their typed backup errors. The returned program keys cells
/// under the backup's catalog ids, so the replay and its integrity check share one identity.
fn check_against_backup_catalog(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    prologue: &BackupPrologue,
    format: CheckFormat,
) -> Result<marrow_check::CheckedProgram, ExitCode> {
    let (report, program) =
        marrow_check::check_project_with_catalog(Path::new(dir), config, prologue.catalog())
            .map_err(|error| {
                report_simple_error(
                    error.code,
                    &format!("{}: {}", error.path.display(), error.message),
                    format,
                );
                ExitCode::FAILURE
            })?;
    if prologue.source_digest() != program.source_digest() {
        return Err(report_backup_error(
            BackupError::SourceMismatch(
                "backup was written from a program whose schema does not match this project"
                    .to_string(),
            ),
            format,
        ));
    }
    if report.has_errors() {
        report_project(dir, &report, format);
        return Err(ExitCode::FAILURE);
    }
    Ok(program)
}
