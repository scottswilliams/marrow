//! `marrow data`: read-only inspection of checked tree-cell project data.

use std::io::{self, Write};
use std::process::ExitCode;

use marrow_check::CheckedProgram;
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::{
    CheckFormat, load_checked_project, open_store_for_inspection, report_simple_error, write_json,
};

#[path = "cmd_data/get.rs"]
pub(crate) mod get;
#[path = "cmd_data/inspect.rs"]
pub(crate) mod inspect;
#[path = "cmd_data/integrity.rs"]
pub(crate) mod integrity;

pub(crate) use inspect::render_value_bytes;

/// Parse one positional project directory plus an optional `--format` flag, for
/// the `data` inspection commands. Reuses `check`'s `--format` grammar so the
/// flag is uniform across the CLI; text is the default.
fn one_positional_with_format(
    command: &str,
    args: &[String],
) -> Result<(String, CheckFormat), ExitCode> {
    let mut dir = None;
    let mut format = CheckFormat::Text;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                index += 1;
                format = parse_format_value(args.get(index))?;
            }
            "--help" | "-h" => {
                print!("Usage:\n  marrow {command} [--format text|json|jsonl] <projectdir>\n");
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => {
                eprintln!("unknown {command} option: {value}");
                return Err(ExitCode::from(2));
            }
            value => {
                if dir.replace(value.to_string()).is_some() {
                    eprintln!("marrow {command} accepts one project directory");
                    return Err(ExitCode::from(2));
                }
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

/// Parse a `--format` value (the argument after the flag), or a usage error when
/// it is missing or not a known format. Shared by the `data` command parsers.
fn parse_format_value(value: Option<&String>) -> Result<CheckFormat, ExitCode> {
    let Some(value) = value else {
        eprintln!("missing value for --format");
        return Err(ExitCode::from(2));
    };
    CheckFormat::parse(value).ok_or_else(|| {
        eprintln!("unknown format: {value}");
        ExitCode::from(2)
    })
}

fn report_store_error(error: StoreError, format: CheckFormat) -> ExitCode {
    report_simple_error(error.code(), &error.to_string(), format);
    ExitCode::FAILURE
}

fn open_tree_store(
    dir: &str,
    config: &marrow_project::ProjectConfig,
) -> Result<Option<TreeStore>, ExitCode> {
    open_store_for_inspection(dir, config)
}

/// Inspect a project's saved data, read-only:
/// `marrow data <roots|stats|dump|integrity|get> <projectdir>`.
pub(crate) fn data(args: &[String]) -> ExitCode {
    let Some((subcommand, rest)) = args.split_first() else {
        eprintln!(
            "missing data subcommand; expected `roots`, `stats`, `dump`, `integrity`, or `get`"
        );
        eprintln!("run `marrow data --help` for usage");
        return ExitCode::from(2);
    };
    match subcommand.as_str() {
        "--help" | "-h" => {
            print!(
                "\
Usage:
  marrow data roots [--format text|json|jsonl] <projectdir> list the saved roots
  marrow data stats [--format text|json|jsonl] <projectdir> count roots and records
  marrow data dump [--format text|json|jsonl] <projectdir> dump every (path, value)
  marrow data integrity [--format text|json|jsonl] <dir>   verify checked saved values decode
  marrow data get [--format text|json|jsonl] <projectdir> <path> read one path's value

Read-only inspection of a project's saved data; it never creates or modifies the
store. `diff` and `load` are deferred: they overlap typed data-evolution and
repair workflows and need source fingerprinting; they will route through the
maintenance capability when implemented.
"
            );
            ExitCode::SUCCESS
        }
        "roots" => data_roots(rest),
        "stats" => data_stats(rest),
        "dump" => data_dump(rest),
        "integrity" => integrity::data_integrity(rest),
        "get" => get::data_get(rest),
        other => {
            eprintln!("unknown data subcommand: {other}");
            eprintln!("expected `roots`, `stats`, `dump`, `integrity`, or `get`");
            ExitCode::from(2)
        }
    }
}

/// `marrow data roots`: list the project's saved roots, one `^root` per line in
/// text, or a `{ project, roots }` object with `--format json`.
fn data_roots(args: &[String]) -> ExitCode {
    let (dir, format) = match one_positional_with_format("data roots", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let store = match open_tree_store(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let roots = match &store {
        Some(store) => match inspect::data_roots_in_store(&program, store) {
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
            write_json(json!({ "project": dir, "roots": roots }));
        }
    }
    ExitCode::SUCCESS
}

/// `marrow data stats`: report how many saved roots and records the store holds,
/// as text lines or a `{ project, roots, records }` object with `--format json`.
fn data_stats(args: &[String]) -> ExitCode {
    let (dir, format) = match one_positional_with_format("data stats", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let store = match open_tree_store(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let (roots, records) = match &store {
        Some(store) => {
            let roots = match inspect::data_roots_in_store(&program, store) {
                Ok(roots) => roots.len(),
                Err(error) => return report_store_error(error, format),
            };
            let records = match inspect::count_data_records(&program, store) {
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
            println!("records: {records}");
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({ "project": dir, "roots": roots, "records": records }));
        }
    }
    ExitCode::SUCCESS
}

/// `marrow data dump`: print every checked stored `(path, value)` in encoded
/// order. Values render as their canonical bytes (UTF-8 text or `0x<hex>`).
fn data_dump(args: &[String]) -> ExitCode {
    let (dir, format) = match one_positional_with_format("data dump", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let store = match open_tree_store(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let records = match &store {
        Some(store) => match inspect::count_data_records(&program, store) {
            Ok(records) => records,
            Err(error) => return report_store_error(error, format),
        },
        None => 0,
    };
    match format {
        CheckFormat::Text => {
            if records == 0 {
                println!("(no saved data)");
            } else {
                let store = store.as_ref().expect("non-empty data dump has a store");
                if let Err(error) = inspect::visit_data_records(&program, store, |record| {
                    println!("{}\t{}", record.path, render_value_bytes(&record.value));
                    Ok(())
                }) {
                    return report_store_error(error, format);
                }
            }
        }
        CheckFormat::Json => {
            if let Some(store) = &store {
                if let Err(error) = write_dump_json(&dir, &program, store) {
                    return report_store_error(error, format);
                }
            } else {
                write_json(json!({ "project": dir, "records": [] }));
            }
        }
        CheckFormat::Jsonl => {
            if let Some(store) = &store {
                let result = inspect::visit_data_records(&program, store, |record| {
                    write_json(dump_record(&record.path, &record.value));
                    Ok(())
                });
                if let Err(error) = result {
                    return report_store_error(error, format);
                }
            }
            write_json(json!({ "kind": "summary", "records": records }));
        }
    }
    ExitCode::SUCCESS
}

fn write_dump_json(
    dir: &str,
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<(), StoreError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "{{\"project\":").expect("write dump JSON");
    serde_json::to_writer(&mut out, dir).expect("serialize project path");
    write!(out, ",\"records\":[").expect("write dump JSON");
    let mut first = true;
    inspect::visit_data_records(program, store, |record| {
        if !first {
            write!(out, ",").expect("write dump JSON separator");
        }
        first = false;
        serde_json::to_writer(&mut out, &dump_record(&record.path, &record.value))
            .expect("serialize dump record");
        Ok(())
    })?;
    writeln!(out, "]}}").expect("write dump JSON");
    Ok(())
}

/// Render a dump record as JSON: the human path plus base64 of the value bytes.
fn dump_record(path: &str, value: &[u8]) -> serde_json::Value {
    json!({
        "path": path,
        "value_b64": marrow_run::base64::encode(value),
    })
}
