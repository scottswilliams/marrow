//! `marrow data`: read-only inspection of checked tree-cell project data.

use std::io::{self, Write};
use std::process::ExitCode;

use marrow_check::CheckedProgram;
use marrow_check::tooling::{count_data_records, data_roots_in_store, visit_data_records};
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::{
    CheckFormat, load_checked_project, open_store_for_inspection, report_simple_error, write_json,
};

#[path = "cmd_data/get.rs"]
pub(crate) mod get;
#[path = "cmd_data/integrity.rs"]
pub(crate) mod integrity;

/// Parse one positional project directory plus an optional `--format` flag, for
/// the `data` inspection commands. Reuses the shared `--format` grammar so the flag
/// is uniform across the CLI; text is the default.
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

fn report_store_error(error: StoreError, format: CheckFormat) -> ExitCode {
    report_simple_error(error.code(), &error.to_string(), format);
    ExitCode::FAILURE
}

/// Pin a coherent read snapshot over an opened store, so every pass an inspection
/// command runs observes one version of saved data. An empty (`None`) store has
/// nothing to pin and yields `Ok(None)`. The returned guard must be held for the
/// duration of the reads it covers; a snapshot failure is reported and returns the
/// exit code. The shared coherent-read scaffold for the `data` inspection commands.
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
store.
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
    let store = match open_store_for_inspection(&dir, &config, format) {
        Ok(store) => store,
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
    let store = match open_store_for_inspection(&dir, &config, format) {
        Ok(store) => store,
        Err(code) => return code,
    };
    // One snapshot spans both passes, so the root count and the record count describe
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
    let store = match open_store_for_inspection(&dir, &config, format) {
        Ok(store) => store,
        Err(code) => return code,
    };
    // One snapshot spans the count and the dump traversal, so the emitted records
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

/// Print each stored `(path, value)` as a tab-separated line, the value rendered as
/// its canonical bytes (UTF-8 text or `0x<hex>`). An empty store prints a placeholder.
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
            crate::render_value_bytes(record.payload.as_bytes())
        );
        Ok(())
    })
    .map(|_| ())
}

/// Stream the dump as one `{ project, records: [...] }` JSON object. An empty store
/// emits the same envelope with no records.
fn render_dump_json(
    dir: &str,
    program: &CheckedProgram,
    store: &Option<TreeStore>,
) -> Result<(), StoreError> {
    match store {
        Some(store) => write_dump_json(dir, program, store),
        None => {
            write_json(json!({ "project": dir, "records": [] }));
            Ok(())
        }
    }
}

/// Stream the dump as one JSON record per line, followed by a `summary` line with the
/// record count.
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
    write_json(json!({ "kind": "summary", "records": records }));
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
            serde_json::to_writer(out, dir).expect("serialize project path");
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
/// bounded memory. `write_prefix` emits the leading object fields, and `visit` runs
/// the record traversal, calling `emit` once per item; this helper owns the `[`, the
/// comma separators between items, and the closing `]}`. The single owner of the
/// streaming JSON-array envelope shared by `data dump` and `data integrity`.
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

/// Render a dump record as JSON: the human path plus base64 of the value bytes.
fn dump_record(path: &str, value: &[u8]) -> serde_json::Value {
    json!({
        "path": path,
        "value_b64": marrow_run::base64::encode(value),
    })
}
