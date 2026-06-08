//! `marrow restore`: replay a typed backup into an empty native store.

use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;

use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::backup::{BackupError, restore_backup};
use crate::{
    CheckFormat, dir_and_path_args, load_checked_project, report_simple_error, resolve_store_path,
    write_json,
};

pub(crate) fn restore(args: &[String]) -> ExitCode {
    let (dir, input, format) = match dir_and_path_args("restore", "backup-file", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
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

    // Restore validates the whole replayed store before commit, including orphan
    // cells under dropped roots or members.
    let verify = |restore_program: &marrow_check::CheckedProgram, store: &TreeStore| {
        match marrow_check::tooling::count_activation_integrity_problems(store, restore_program) {
            Ok((_, 0)) => Ok(()),
            Ok((_, problems)) => Err(BackupError::DataInvalid(format!(
                "restored data has {problems} schema problem(s); the backup does not match this project"
            ))),
            Err(error) => Err(BackupError::Store(error)),
        }
    };

    match restore_backup(&program, &store, &mut reader, verify) {
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
