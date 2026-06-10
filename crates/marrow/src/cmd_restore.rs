//! `marrow restore`: replay a typed backup into an empty native store.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::process::ExitCode;

use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::backup::{
    BackupError, BackupPrologue, read_backup_prologue, restore_backup_with_prologue,
};
use crate::{
    CheckFormat, dir_and_path_args, load_config_with_format, report_project, report_simple_error,
    resolve_store_path, write_json,
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
            report_simple_error(error.code(), &error.to_string(), format);
            return ExitCode::FAILURE;
        }
    };

    // Restore binds the project against the catalog the backup carries, so the replayed
    // data validates against the same accepted identity the original store wrote it under
    // rather than a freshly proposed baseline.
    let program = match check_against_backup_catalog(&dir, &config, &prologue, format) {
        Ok(program) => program,
        Err(code) => return code,
    };

    // Restore needs a durable target. An in-memory project has nowhere to write to.
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

    match restore_backup_with_prologue(&program, &store, prologue, &mut reader, verify) {
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
            report_simple_error(error.code(), &error.to_string(), format);
            ExitCode::FAILURE
        }
    }
}

/// Check the project bound to the catalog the backup carries. A source-text mismatch is
/// reported as `restore.source_mismatch`; a project that does not check cleanly against
/// the backup's catalog is a `restore.catalog_mismatch` (the backup's accepted identity
/// is not this project's). The returned program keys cells under the backup's catalog
/// ids, so the replay and its integrity check share one identity.
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
        report_simple_error(
            "restore.source_mismatch",
            "backup was written from a program whose schema does not match this project",
            format,
        );
        return Err(ExitCode::FAILURE);
    }
    if report.has_errors() {
        report_project(dir, &report, format);
        return Err(ExitCode::FAILURE);
    }
    Ok(program)
}
