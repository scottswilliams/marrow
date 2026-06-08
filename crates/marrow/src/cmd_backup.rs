//! `marrow backup`: write a typed portable backup of a project's saved data.

use std::fs::File;
use std::io::BufWriter;
use std::process::ExitCode;

use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::backup::create_backup;
use crate::{
    CheckFormat, dir_and_path_args, load_checked_project, open_store_for_inspection,
    report_simple_error, write_json,
};

pub(crate) fn backup(args: &[String]) -> ExitCode {
    let (dir, output, format) = match dir_and_path_args("backup", "output-file", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    // A project with no saved data on disk yields a valid empty backup.
    let store = match open_store_for_inspection(&dir, &config, format) {
        Ok(Some(store)) => store,
        Ok(None) => TreeStore::memory(),
        Err(code) => return code,
    };

    let file = match File::create(&output) {
        Ok(file) => file,
        Err(error) => {
            report_simple_error(
                "io.write",
                &format!("could not create {output}: {error}"),
                format,
            );
            return ExitCode::FAILURE;
        }
    };
    let mut writer = BufWriter::new(file);
    match create_backup(&program, &store, &mut writer) {
        Ok(report) => {
            match format {
                CheckFormat::Text => {
                    println!(
                        "ok: backed up {} record(s) to {output}",
                        report.record_count
                    );
                }
                CheckFormat::Json | CheckFormat::Jsonl => write_json(json!({
                    "output": output,
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
