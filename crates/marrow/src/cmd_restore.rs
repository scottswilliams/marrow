//! `marrow restore`: replay a typed backup into an empty native store by
//! default, or into a counted replace target when requested.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::BufReader;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_run::SystemNondeterminism;
use marrow_store::tree::TreeStore;

use crate::backup::{
    BackupError, BackupPrologue, CatalogFingerprintRef, RestoreReceipt, RestoreTargetMode,
    read_backup_prologue, restore_backup_with_prologue,
};
use crate::{
    CheckFormat, load_config_with_format, native_store_path, report_io_error, report_project,
    report_simple_error, resolve_store_path,
};

pub(crate) fn restore(args: &[String]) -> ExitCode {
    let RestoreArgs {
        dir,
        input,
        target_mode,
    } = match parse_restore_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let format = CheckFormat::Text;
    let config = match load_config_with_format(&dir, format) {
        Ok(config) => config,
        Err(code) => return code,
    };

    let (mut reader, prologue) = match read_backup_artifact(&input, format) {
        Ok(backup) => backup,
        Err(code) => return code,
    };

    // Restore binds the project against the catalog the backup carries, so the replayed
    // data validates against the same accepted identity the original store wrote it under
    // rather than a freshly proposed baseline.
    let program = match check_against_backup_catalog(&dir, &config, &prologue, format) {
        Ok(program) => program,
        Err(code) => return code,
    };

    if let Err(code) = reject_current_catalog_mismatch(&dir, &prologue, format) {
        return code;
    }

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

    let mut nondeterminism = SystemNondeterminism::new();
    match restore_backup_with_prologue(
        &program,
        &store,
        prologue,
        &mut reader,
        target_mode,
        &mut nondeterminism,
        verify_restored_data,
    ) {
        Ok(report) => {
            report_restore_text(&input, &report);
            ExitCode::SUCCESS
        }
        Err(error) => {
            drop(store);
            target_files.cleanup_created();
            report_backup_error(error, format)
        }
    }
}

pub(crate) fn mount_backup_for_inspection(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    input: &str,
    format: CheckFormat,
) -> Result<(marrow_check::CheckedProgram, TreeStore), ExitCode> {
    let (mut reader, prologue) = read_backup_artifact(input, format)?;
    let program = check_against_backup_catalog(dir, config, &prologue, format)?;
    let store = TreeStore::memory();
    let mut nondeterminism = SystemNondeterminism::new();
    restore_backup_with_prologue(
        &program,
        &store,
        prologue,
        &mut reader,
        RestoreTargetMode::EmptyOnly,
        &mut nondeterminism,
        verify_restored_data,
    )
    .map_err(|error| report_backup_error(error, format))?;
    Ok((program, store))
}

pub(crate) fn mount_backup_for_evolution_preview(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    input: &str,
    format: CheckFormat,
) -> Result<(marrow_check::CheckedProgram, TreeStore), ExitCode> {
    let (mut reader, prologue) = read_backup_artifact(input, format)?;
    let program = check_project_with_backup_catalog(dir, config, &prologue, format)?;
    reject_current_catalog_mismatch(dir, &prologue, format)?;
    let mut nondeterminism = SystemNondeterminism::new();
    let store = crate::backup::mount_backup_for_evolution_preview(
        &program,
        prologue,
        &mut reader,
        &mut nondeterminism,
    )
    .map_err(|error| report_backup_error(error, format))?;
    Ok((program, store))
}

fn read_backup_artifact(
    input: &str,
    format: CheckFormat,
) -> Result<(BufReader<File>, BackupPrologue), ExitCode> {
    let file = match File::open(input) {
        Ok(file) => file,
        Err(error) => {
            report_simple_error(
                "io.read",
                &format!("could not open {input}: {error}"),
                format,
            );
            return Err(ExitCode::FAILURE);
        }
    };
    let mut reader = BufReader::new(file);
    let prologue =
        read_backup_prologue(&mut reader).map_err(|error| report_backup_error(error, format))?;
    Ok((reader, prologue))
}

fn verify_restored_data(
    restore_program: &marrow_check::CheckedProgram,
    store: &TreeStore,
) -> Result<(), BackupError> {
    match marrow_check::tooling::count_activation_integrity_problems(store, restore_program) {
        Ok((_, 0)) => Ok(()),
        Ok((_, problems)) => Err(BackupError::DataInvalid(format!(
            "restored data has {problems} schema problem(s); the backup does not match this project"
        ))),
        Err(error) => Err(BackupError::Store(error)),
    }
}

struct RestoreArgs {
    dir: String,
    input: String,
    target_mode: RestoreTargetMode,
}

fn parse_restore_args(args: &[String]) -> Result<RestoreArgs, ExitCode> {
    let mut positionals = Vec::new();
    let mut replace = false;
    let mut expected_live_records = None;
    let mut saw_count = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--replace" => {
                if replace {
                    eprintln!("duplicate --replace");
                    return Err(ExitCode::from(2));
                }
                replace = true;
            }
            "--count" => {
                if saw_count {
                    eprintln!("duplicate --count");
                    return Err(ExitCode::from(2));
                }
                saw_count = true;
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --count");
                    return Err(ExitCode::from(2));
                };
                expected_live_records = Some(parse_count_value(value)?);
            }
            "--help" | "-h" => {
                print!(
                    "Usage:\n  marrow restore [--replace --count N] <projectdir> <backup-file>\n"
                );
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => return Err(crate::unknown_option("restore", value)),
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    if replace && expected_live_records.is_none() {
        eprintln!("--replace requires --count");
        return Err(ExitCode::from(2));
    }
    if !replace && expected_live_records.is_some() {
        eprintln!("--count requires --replace");
        return Err(ExitCode::from(2));
    }
    let target_mode = match expected_live_records {
        Some(expected_live_records) => RestoreTargetMode::Replace {
            expected_live_records,
        },
        None => RestoreTargetMode::EmptyOnly,
    };
    match positionals.as_slice() {
        [dir, input] => Ok(RestoreArgs {
            dir: dir.clone(),
            input: input.clone(),
            target_mode,
        }),
        [] | [_] => {
            eprintln!("marrow restore requires a project directory and a backup-file");
            Err(ExitCode::from(2))
        }
        _ => {
            eprintln!("marrow restore accepts one project directory and one backup-file");
            Err(ExitCode::from(2))
        }
    }
}

fn parse_count_value(value: &str) -> Result<u64, ExitCode> {
    value.parse::<u64>().map_err(|_| {
        eprintln!("--count must be a nonnegative integer");
        ExitCode::from(2)
    })
}

fn report_restore_text(input: &str, report: &crate::backup::RestoreReport) {
    match report.receipt {
        RestoreReceipt::EmptyOnly => {
            println!(
                "ok: restored {} record(s) from {input}",
                report.record_count
            );
        }
        RestoreReceipt::Replace {
            expected_live_records,
            replaced_live_records,
        } => {
            println!(
                "ok: restored {} record(s) from {input}; receipt: mode=replace expected_live_records={expected_live_records} replaced_live_records={replaced_live_records}",
                report.record_count
            );
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
    let backup_catalog = CatalogFingerprintRef::from_catalog(prologue.catalog());
    let project_catalog = CatalogFingerprintRef::from_catalog(Some(&current));
    if project_catalog == backup_catalog {
        return Ok(());
    }
    Err(report_backup_error(
        BackupError::catalog_mismatch(backup_catalog, project_catalog),
        format,
    ))
}

fn read_source_tree_catalog(
    dir: &str,
    format: CheckFormat,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ExitCode> {
    let path = Path::new(dir).join(marrow_project::CATALOG_FILE_NAME);
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
    let program = check_project_with_backup_catalog(dir, config, prologue, format)?;
    let project_source_digest = program.source_digest();
    if prologue.source_digest() != project_source_digest {
        return Err(report_backup_error(
            BackupError::source_mismatch(prologue.source_digest(), project_source_digest.as_str()),
            format,
        ));
    }
    Ok(program)
}

fn check_project_with_backup_catalog(
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
    if report.has_errors() {
        report_project(dir, &report, format);
        return Err(ExitCode::FAILURE);
    }
    Ok(program)
}
