//! `marrow data`: inspection and recovery of tree-cell project data.

use std::io::{self, Write};
use std::process::ExitCode;

use marrow_check::CheckedProgram;
use marrow_check::tooling::{
    count_data_records, data_roots_in_store, render_data_value, visit_data_records,
};
use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::{
    CheckFormat, load_checked_project_with_format, load_config_with_format, native_store_path,
    open_store_for_inspection, report_simple_error, write_json,
};

#[path = "cmd_data/get.rs"]
pub(crate) mod get;
#[path = "cmd_data/integrity.rs"]
pub(crate) mod integrity;

/// Shared `--format` parsing for the `data` inspection subcommands, so the flag
/// grammar stays uniform across the CLI; text is the default.
fn one_positional_with_format(
    command: &str,
    args: &[String],
) -> Result<(String, CheckFormat), ExitCode> {
    let mut dir = None;
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                crate::parse_format_flag(args, &mut index, &mut saw_format, &mut format)?;
            }
            "--help" | "-h" => {
                print!("Usage:\n  marrow {command} [--format text|json|jsonl] <projectdir>\n");
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => {
                return Err(crate::unknown_option(command, value));
            }
            value => {
                crate::take_single_target(&mut dir, value, command, "project directory")?;
            }
        }
        index += 1;
    }
    let dir = dir.ok_or_else(|| {
        eprintln!("missing project directory");
        ExitCode::from(2)
    })?;
    Ok((dir, format))
}

struct DataReadArgs {
    dir: String,
    format: CheckFormat,
    backup: Option<String>,
}

struct DataReadTarget {
    dir: String,
    format: CheckFormat,
    program: CheckedProgram,
    store: Option<TreeStore>,
}

fn one_positional_with_format_and_backup(
    command: &str,
    args: &[String],
) -> Result<DataReadArgs, ExitCode> {
    let mut dir = None;
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut backup = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                crate::parse_format_flag(args, &mut index, &mut saw_format, &mut format)?;
            }
            "--backup" => {
                parse_backup_flag(args, &mut index, &mut backup)?;
            }
            "--help" | "-h" => {
                print!(
                    "Usage:\n  marrow {command} [--backup <artifact>] [--format text|json|jsonl] <projectdir>\n"
                );
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => {
                return Err(crate::unknown_option(command, value));
            }
            value => {
                crate::take_single_target(&mut dir, value, command, "project directory")?;
            }
        }
        index += 1;
    }
    let dir = dir.ok_or_else(|| {
        eprintln!("missing project directory");
        ExitCode::from(2)
    })?;
    Ok(DataReadArgs {
        dir,
        format,
        backup,
    })
}

fn dir_and_path_args_with_backup(
    command: &str,
    path_label: &str,
    args: &[String],
) -> Result<(String, String, CheckFormat, Option<String>), ExitCode> {
    let mut positionals = Vec::new();
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut backup = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                crate::parse_format_flag(args, &mut index, &mut saw_format, &mut format)?;
            }
            "--backup" => {
                parse_backup_flag(args, &mut index, &mut backup)?;
            }
            "--help" | "-h" => {
                print!(
                    "Usage:\n  marrow {command} [--backup <artifact>] [--format text|json|jsonl] <projectdir> <{path_label}>\n"
                );
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => return Err(crate::unknown_option(command, value)),
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    match positionals.as_slice() {
        [dir, path] => Ok((dir.clone(), path.clone(), format, backup)),
        [] | [_] => {
            eprintln!("marrow {command} requires a project directory and a {path_label}");
            Err(ExitCode::from(2))
        }
        _ => {
            eprintln!("marrow {command} accepts one project directory and one {path_label}");
            Err(ExitCode::from(2))
        }
    }
}

fn parse_backup_flag(
    args: &[String],
    index: &mut usize,
    backup: &mut Option<String>,
) -> Result<(), ExitCode> {
    if backup.is_some() {
        eprintln!("duplicate --backup");
        return Err(ExitCode::from(2));
    }
    *index += 1;
    let Some(value) = args.get(*index) else {
        eprintln!("missing value for --backup");
        return Err(ExitCode::from(2));
    };
    *backup = Some(value.to_string());
    Ok(())
}

fn load_data_read_target_from_args(
    command: &str,
    args: &[String],
) -> Result<DataReadTarget, ExitCode> {
    let DataReadArgs {
        dir,
        format,
        backup,
    } = one_positional_with_format_and_backup(command, args)?;
    load_data_read_target(dir, format, backup)
}

fn load_data_read_target(
    dir: String,
    format: CheckFormat,
    backup: Option<String>,
) -> Result<DataReadTarget, ExitCode> {
    if let Some(backup) = backup {
        let config = load_config_with_format(&dir, format)?;
        let (program, store) =
            crate::cmd_restore::mount_backup_for_inspection(&dir, &config, &backup, format)?;
        return Ok(DataReadTarget {
            dir,
            format,
            program,
            store: Some(store),
        });
    }

    let (config, program) = load_checked_project_with_format(&dir, format)?;
    let store = open_store_for_inspection(&dir, &config, format)?;
    Ok(DataReadTarget {
        dir,
        format,
        program,
        store,
    })
}

fn report_store_error(error: StoreError, format: CheckFormat) -> ExitCode {
    report_simple_error(error.code(), &error.to_string(), format);
    ExitCode::FAILURE
}

/// Pin a read snapshot so every pass of an inspection command observes one version of
/// saved data. The caller must hold the returned guard for the duration of its reads;
/// an empty store has nothing to pin and yields `Ok(None)`. The shared coherent-read
/// scaffold for the `data` inspection commands.
pub(super) fn pin_snapshot(
    store: &Option<TreeStore>,
    format: CheckFormat,
) -> Result<Option<marrow_store::tree::ReadSnapshot<'_>>, ExitCode> {
    match store {
        Some(store) => match store.read_snapshot() {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(error) => Err(report_store_error(error, format)),
        },
        None => Ok(None),
    }
}

pub(crate) fn data(args: &[String]) -> ExitCode {
    let Some((subcommand, rest)) = args.split_first() else {
        eprintln!(
            "missing data subcommand; expected `roots`, `stats`, `dump`, `integrity`, `recover`, or `get`"
        );
        eprintln!("run `marrow data --help` for usage");
        return ExitCode::from(2);
    };
    match subcommand.as_str() {
        "--help" | "-h" => {
            print!(
                "\
Usage:
  marrow data roots [--backup <artifact>] [--format text|json|jsonl] <projectdir> list the saved roots
  marrow data stats [--backup <artifact>] [--format text|json|jsonl] <projectdir> count roots and cells
  marrow data dump [--backup <artifact>] [--format text|json|jsonl] <projectdir> dump every (path, value)
  marrow data integrity [--backup <artifact>] [--format text|json|jsonl] <dir>   verify checked saved values decode
  marrow data recover [--format text|json|jsonl] <dir>     repair an unclean native store open
  marrow data get [--backup <artifact>] [--format text|json|jsonl] <projectdir> <path> read one path's value

Inspection of a project's saved data. `recover` is the only write-capable data
command; the other subcommands never create or modify the store.
"
            );
            ExitCode::SUCCESS
        }
        "roots" => data_roots(rest),
        "stats" => data_stats(rest),
        "dump" => data_dump(rest),
        "integrity" => integrity::data_integrity(rest),
        "recover" => data_recover(rest),
        "get" => get::data_get(rest),
        other => {
            eprintln!("unknown data subcommand: {other}");
            eprintln!("expected `roots`, `stats`, `dump`, `integrity`, `recover`, or `get`");
            ExitCode::from(2)
        }
    }
}

fn data_recover(args: &[String]) -> ExitCode {
    let (dir, format) = match one_positional_with_format("data recover", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let config = match load_config_with_format(&dir, format) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let Some(path) = (match native_store_path(&dir, &config, format) {
        Ok(path) => path,
        Err(code) => return code,
    }) else {
        return report_no_store_to_recover(&dir, None, format);
    };
    if !path.exists() {
        return report_no_store_to_recover(&dir, Some(&path), format);
    }
    match TreeStore::open_existing(&path) {
        Ok(_store) => report_recovered_store(&dir, &path, format),
        Err(error) => report_store_error(error, format),
    }
}

fn report_no_store_to_recover(
    dir: &str,
    path: Option<&std::path::Path>,
    format: CheckFormat,
) -> ExitCode {
    match format {
        CheckFormat::Text => match path {
            Some(path) => println!("no store file at {}; nothing to recover", path.display()),
            None => println!("no native store configured for {dir}; nothing to recover"),
        },
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "project": crate::project_json_path(dir),
                "status": "absent",
                "store": path.map(|path| path.display().to_string()),
            }));
        }
    }
    ExitCode::SUCCESS
}

fn report_recovered_store(dir: &str, path: &std::path::Path, format: CheckFormat) -> ExitCode {
    match format {
        CheckFormat::Text => println!("store open/repair completed: {}", path.display()),
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "project": crate::project_json_path(dir),
                "status": "opened",
                "store": path.display().to_string(),
            }));
        }
    }
    ExitCode::SUCCESS
}

fn data_roots(args: &[String]) -> ExitCode {
    let DataReadTarget {
        dir,
        format,
        program,
        store,
    } = match load_data_read_target_from_args("data roots", args) {
        Ok(target) => target,
        Err(code) => return code,
    };
    let roots = match &store {
        Some(store) => match data_roots_in_store(&program, store) {
            Ok(roots) => roots,
            Err(error) => return report_store_error(error, format),
        },
        None => Vec::new(),
    };
    match format {
        CheckFormat::Text => {
            if roots.is_empty() {
                println!("(no saved data)");
            } else {
                for root in roots {
                    println!("^{root}");
                }
            }
        }
        // jsonl carries no streaming meaning for roots, so it emits the same
        // single object as json, keeping one uniform `--format` flag.
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({ "project": crate::project_json_path(&dir), "roots": roots }));
        }
    }
    ExitCode::SUCCESS
}

fn data_stats(args: &[String]) -> ExitCode {
    let DataReadTarget {
        dir,
        format,
        program,
        store,
    } = match load_data_read_target_from_args("data stats", args) {
        Ok(target) => target,
        Err(code) => return code,
    };
    // One snapshot spans both passes, so the root count and the cell count describe
    // the same coherent version of the store.
    let _snapshot = match pin_snapshot(&store, format) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };
    let (roots, records) = match &store {
        Some(store) => {
            let roots = match data_roots_in_store(&program, store) {
                Ok(roots) => roots.len(),
                Err(error) => return report_store_error(error, format),
            };
            let records = match count_data_records(&program, store) {
                Ok(records) => records,
                Err(error) => return report_store_error(error, format),
            };
            (roots, records)
        }
        None => (0, 0),
    };
    match format {
        CheckFormat::Text => {
            println!("roots: {roots}");
            println!("cells: {records}");
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "project": crate::project_json_path(&dir),
                "roots": roots,
                "cells": records,
            }));
        }
    }
    ExitCode::SUCCESS
}

fn data_dump(args: &[String]) -> ExitCode {
    let DataReadTarget {
        dir,
        format,
        program,
        store,
    } = match load_data_read_target_from_args("data dump", args) {
        Ok(target) => target,
        Err(code) => return code,
    };
    // One snapshot spans the count and the dump traversal, so the emitted cells
    // and the trailing count describe the same coherent version of the store.
    let _snapshot = match pin_snapshot(&store, format) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };
    let records = match &store {
        Some(store) => match count_data_records(&program, store) {
            Ok(records) => records,
            Err(error) => return report_store_error(error, format),
        },
        None => 0,
    };
    let result = match format {
        CheckFormat::Text => render_dump_text(&program, &store, records),
        CheckFormat::Json => render_dump_json(&dir, &program, &store),
        CheckFormat::Jsonl => render_dump_jsonl(&program, &store, records),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report_store_error(error, format),
    }
}

fn render_dump_text(
    program: &CheckedProgram,
    store: &Option<TreeStore>,
    records: usize,
) -> Result<(), StoreError> {
    let Some(store) = store.as_ref().filter(|_| records > 0) else {
        println!("(no saved data)");
        return Ok(());
    };
    visit_data_records(program, store, |record| {
        println!(
            "{}\t{}",
            record.path,
            render_data_value(program, record.leaf(), record.payload.as_bytes())
        );
        Ok(())
    })
    .map(|_| ())
}

fn render_dump_json(
    dir: &str,
    program: &CheckedProgram,
    store: &Option<TreeStore>,
) -> Result<(), StoreError> {
    match store {
        Some(store) => write_dump_json(dir, program, store),
        None => {
            write_json(json!({ "project": crate::project_json_path(dir), "records": [] }));
            Ok(())
        }
    }
}

fn render_dump_jsonl(
    program: &CheckedProgram,
    store: &Option<TreeStore>,
    records: usize,
) -> Result<(), StoreError> {
    if let Some(store) = store {
        visit_data_records(program, store, |record| {
            write_json(dump_record(&record.path, record.payload.as_bytes()));
            Ok(())
        })?;
    }
    write_json(json!({ "kind": "summary", "cells": records }));
    Ok(())
}

fn write_dump_json(
    dir: &str,
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(), StoreError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write_json_array_envelope(
        &mut out,
        |out| {
            write!(out, "\"project\":").expect("write dump JSON");
            serde_json::to_writer(out, &crate::project_json_path(dir))
                .expect("serialize project path");
        },
        "records",
        |emit| {
            visit_data_records(program, store, |record| {
                emit(&dump_record(&record.path, record.payload.as_bytes()));
                Ok(())
            })
            .map(|_| ())
        },
    )
}

/// Stream a `{ <prefix>, "<array_field>": [ <items> ] }` JSON object to `out` in
/// bounded memory: `write_prefix` emits the leading fields, `visit` calls `emit` once
/// per item, and this helper owns the `[`, the comma separators, and the closing `]}`.
/// The single owner of the streaming JSON-array envelope shared by `data dump` and
/// `data integrity`.
pub(super) fn write_json_array_envelope(
    out: &mut impl Write,
    write_prefix: impl FnOnce(&mut dyn Write),
    array_field: &str,
    visit: impl FnOnce(&mut dyn FnMut(&serde_json::Value)) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    write!(out, "{{").expect("write JSON envelope");
    write_prefix(out);
    write!(out, ",\"{array_field}\":[").expect("write JSON envelope");
    {
        let mut first = true;
        let mut emit = |item: &serde_json::Value| {
            if !first {
                write!(out, ",").expect("write JSON envelope separator");
            }
            first = false;
            serde_json::to_writer(&mut *out, item).expect("serialize JSON envelope item");
        };
        visit(&mut emit)?;
    }
    writeln!(out, "]}}").expect("write JSON envelope");
    Ok(())
}

fn dump_record(path: &str, value: &[u8]) -> serde_json::Value {
    json!({
        "path": path,
        "value_b64": marrow_run::base64::encode(value),
    })
}

pub(crate) fn saved_key_json(key: &SavedKey) -> serde_json::Value {
    match key {
        SavedKey::Int(value) => json!({ "type": "int", "value": value }),
        SavedKey::Bool(value) => json!({ "type": "bool", "value": value }),
        SavedKey::Str(value) => json!({ "type": "string", "value": value }),
        SavedKey::Date(value) => json!({ "type": "date", "days_since_epoch": value }),
        SavedKey::Duration(value) => json!({ "type": "duration", "nanos": value.to_string() }),
        SavedKey::Instant(value) => {
            json!({ "type": "instant", "nanos_since_epoch": value.to_string() })
        }
        SavedKey::Bytes(value) => {
            json!({ "type": "bytes", "value_b64": marrow_run::base64::encode(value) })
        }
    }
}
