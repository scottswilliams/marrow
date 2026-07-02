//! `marrow backup`: write a typed portable backup of a project's saved data.

use std::path::Path;
use std::process::ExitCode;

use marrow_check::tooling::verify_store_completeness;
use marrow_run::SystemNondeterminism;
use marrow_store::tree::TreeStore;

use crate::backup::{count_live_entities, create_backup_artifact, ensure_store_uid};
use crate::term_style::{self, Stream, Style};
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
    let lock = match crate::read_committed_lock(&dir, format) {
        Ok(lock) => lock,
        Err(code) => return code,
    };
    let mut nondeterminism = SystemNondeterminism::new();
    // A project with no saved data on disk yields a valid empty backup.
    let on_disk = match open_store_for_inspection(&dir, &config, format) {
        Ok(store) => store,
        Err(code) => return code,
    };

    // The committed-root cross-check is the separate witness for a present store rolled back below
    // its committed identity, run against the on-disk store rather than the empty fallback. An
    // absent store body is the disposable-store case, not a loss, so it yields an empty archive
    // rather than failing closed.
    match crate::verify_lock_roots(on_disk.as_deref(), lock.as_ref()) {
        crate::LockRootVerdict::Clean => {}
        crate::LockRootVerdict::Lost(error) => {
            report_simple_error(error.code(), &error.to_string(), format);
            return ExitCode::FAILURE;
        }
    }

    let store = match on_disk {
        Some(store) => store.into_store(),
        None => {
            let store = TreeStore::memory();
            if let Err(error) = ensure_store_uid(&store, &mut nondeterminism) {
                report_simple_error(error.code(), &error.to_string(), format);
                return ExitCode::FAILURE;
            }
            store
        }
    };

    // A backup carries data cells and rebuilds the index family on restore, so data
    // silently truncated by a damaged page, or an index whose entries were dropped,
    // would be archived as if healthy and its under-read masked. Fail closed on an
    // incomplete store before writing the artifact rather than propagating a store the
    // schema can no longer fully derive.
    if let Err(error) = verify_store_completeness(&store, &program) {
        report_simple_error(error.code(), &error.to_string(), format);
        return ExitCode::FAILURE;
    }

    let output_path = Path::new(&output);
    match create_backup_artifact(&program, &store, output_path) {
        Ok(()) => {
            // The printed count is the saved entities, the user-facing record count that
            // `data stats records:` and the restore `--count` guard also report. The
            // artifact's own physical cell-frame count stays internal to the manifest.
            let records = match count_live_entities(&program, &store) {
                Ok(records) => records,
                Err(error) => {
                    report_simple_error(error.code(), &error.to_string(), format);
                    return ExitCode::FAILURE;
                }
            };
            println!(
                "{} backed up {records} record(s) to {output}",
                term_style::paint(Stream::Stdout, Style::Success, "ok:")
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
