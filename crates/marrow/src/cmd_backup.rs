//! `marrow backup`: write a typed portable backup of a project's saved data.

use std::path::Path;
use std::process::ExitCode;

use marrow_run::SystemNondeterminism;
use marrow_store::tree::TreeStore;

use crate::backup::{create_backup_artifact, ensure_store_uid};
use crate::{CheckFormat, load_checked_project, open_store_for_inspection, report_simple_error};

pub(crate) fn backup(args: &[String]) -> ExitCode {
    let (dir, output) = match backup_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let format = CheckFormat::Text;
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let mut nondeterminism = SystemNondeterminism::new();
    // A project with no saved data on disk yields a valid empty backup.
    let store = match open_store_for_inspection(&dir, &config, format) {
        Ok(Some(store)) => store,
        Ok(None) => {
            let store = TreeStore::memory();
            if let Err(error) = ensure_store_uid(&store, &mut nondeterminism) {
                report_simple_error(error.code(), &error.to_string(), format);
                return ExitCode::FAILURE;
            }
            store
        }
        Err(code) => return code,
    };

    let output_path = Path::new(&output);
    match create_backup_artifact(&program, &store, output_path) {
        Ok(report) => {
            println!(
                "ok: backed up {} record(s) to {output}",
                report.record_count
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), format);
            ExitCode::FAILURE
        }
    }
}

fn backup_args(args: &[String]) -> Result<(String, String), ExitCode> {
    let mut positionals = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("Usage:\n  marrow backup <projectdir> <output-file>\n");
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => return Err(crate::unknown_option("backup", value)),
            value => positionals.push(value.to_string()),
        }
    }
    match positionals.as_slice() {
        [dir, output] => Ok((dir.clone(), output.clone())),
        [] | [_] => {
            eprintln!("marrow backup requires a project directory and an output-file");
            Err(ExitCode::from(2))
        }
        _ => {
            eprintln!("marrow backup accepts one project directory and one output-file");
            Err(ExitCode::from(2))
        }
    }
}
