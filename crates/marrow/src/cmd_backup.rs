//! `marrow backup` / `marrow restore`: move a project's saved data to and from a
//! portable archive.

use std::io::Write;
use std::process::ExitCode;

use crate::{CheckFormat, load_config, open_owned_store, report_io_error, report_simple_error};

/// Parse exactly two positional paths (a project directory and an archive) for
/// `backup`/`restore`, handling `--help` and rejecting options or a wrong count.
fn two_positionals(command: &str, args: &[String]) -> Result<(String, String), ExitCode> {
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                print!("Usage:\n  marrow {command} <projectdir> <archive>\n");
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => {
                eprintln!("unknown {command} option: {value}");
                return Err(ExitCode::from(2));
            }
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    match positionals.as_slice() {
        [dir, archive] => Ok((dir.clone(), archive.clone())),
        _ => {
            eprintln!("marrow {command} takes a project directory and an archive path");
            Err(ExitCode::from(2))
        }
    }
}

/// The plural suffix for a record count: `""` for one, `"s"` otherwise.
fn plural(count: u64) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Back up a project's saved data to a portable archive:
/// `marrow backup <projectdir> <archive>`.
pub(crate) fn backup(args: &[String]) -> ExitCode {
    let (dir, archive) = match two_positionals("backup", args) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let config = match load_config(&dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let store = match open_owned_store(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let file = match std::fs::File::create(&archive) {
        Ok(file) => file,
        Err(error) => {
            report_io_error(&archive, &error, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    let mut writer = std::io::BufWriter::new(file);
    let count = match marrow_store::archive::write_archive(&*store, &mut writer) {
        Ok(count) => count,
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = writer.flush() {
        report_io_error(&archive, &error, CheckFormat::Text);
        return ExitCode::FAILURE;
    }
    println!("backed up {count} record{} to {archive}", plural(count));
    ExitCode::SUCCESS
}

/// Restore a project's saved data from a portable archive into an empty store:
/// `marrow restore <projectdir> <archive>`.
pub(crate) fn restore(args: &[String]) -> ExitCode {
    let (dir, archive) = match two_positionals("restore", args) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let config = match load_config(&dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let mut store = match open_owned_store(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    // A normal restore writes into an empty target; replace/merge/repair would
    // be explicit maintenance actions, which this command does not offer.
    match store.roots() {
        Ok(roots) if !roots.is_empty() => {
            report_simple_error(
                "restore.not_empty",
                "restore target already holds data; restore writes into an empty store",
                CheckFormat::Text,
            );
            return ExitCode::FAILURE;
        }
        Ok(_) => {}
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    }
    let file = match std::fs::File::open(&archive) {
        Ok(file) => file,
        Err(error) => {
            report_io_error(&archive, &error, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    let mut reader = std::io::BufReader::new(file);
    match marrow_store::archive::read_archive(&mut reader, &mut *store) {
        Ok(count) => {
            println!("restored {count} record{} from {archive}", plural(count));
            ExitCode::SUCCESS
        }
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            ExitCode::FAILURE
        }
    }
}
