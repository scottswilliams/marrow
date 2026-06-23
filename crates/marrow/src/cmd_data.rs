//! `marrow data`: inspection and recovery of tree-cell project data.

use std::io::{self, Write};
use std::process::ExitCode;

use marrow_check::CheckedProgram;
use marrow_check::tooling::{
    StampedData, count_data_records, count_orphan_cells, data_roots_in_store, data_snapshot_stamp,
    render_data_value, stamped_data_roots_in_store, verify_store_index_integrity,
    visit_data_records,
};
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;
use serde_json::json;

use crate::{
    CheckFormat, load_checked_project_with_format, load_config_with_format, native_store_path,
    open_store_for_inspection, probe_checked_project, report_simple_error, store_path_is_absent,
    write_json,
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

/// The `data.orphan` code shared with `data integrity`. `data stats` and `data dump` render the
/// store through the current source-derived schema view, so cells under members the new source no
/// longer declares are not traversed. Surfacing this advisory keeps the reduced output from being
/// silent without changing the exit status: the data is physically intact, and the count of hidden
/// cells points the developer at `data integrity` for the full picture.
const DATA_ORPHAN_CODE: &str = "data.orphan";

/// When a drifted source is bound over a store, warn on stderr that the source-driven inspection
/// could not see the cells under undeclared members. Counting them silently would under-report
/// intact data; the exit status stays unchanged because the cells are durable and the inspection
/// is read-only. A store the schema fully declares has no orphans and stays silent.
pub(super) fn warn_on_hidden_orphans(program: &CheckedProgram, store: &Option<TreeStore>) {
    let Some(store) = store else {
        return;
    };
    if let Ok(orphans @ 1..) = count_orphan_cells(store, program) {
        eprintln!(
            "{DATA_ORPHAN_CODE}: {orphans} stored cell(s) under members the current source no longer declares are hidden from this view; run `marrow data integrity` to see them"
        );
    }
}

fn report_store_error(error: StoreError, format: CheckFormat) -> ExitCode {
    report_simple_error(error.code(), &error.to_string(), format);
    ExitCode::FAILURE
}

#[derive(Debug)]
pub(super) enum DataOutputError {
    Store(StoreError),
    Io(io::Error),
}

impl From<StoreError> for DataOutputError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<io::Error> for DataOutputError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl DataOutputError {
    fn from_json(error: serde_json::Error) -> Self {
        let kind = error.io_error_kind().unwrap_or(io::ErrorKind::Other);
        Self::Io(io::Error::new(kind, error.to_string()))
    }
}

fn report_data_output_error(error: DataOutputError, format: CheckFormat) -> ExitCode {
    match error {
        DataOutputError::Store(error) => report_store_error(error, format),
        DataOutputError::Io(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            ExitCode::SUCCESS
        }
        DataOutputError::Io(error) => {
            eprintln!("io.write: failed to write data output: {error}");
            ExitCode::FAILURE
        }
    }
}

fn stop_after_output_error() -> StoreError {
    StoreError::InvalidTransaction {
        message: "data output stopped after stdout write failed".into(),
    }
}

fn stop_on_output_error(
    output_error: &mut Option<DataOutputError>,
    result: Result<(), DataOutputError>,
) -> Result<(), StoreError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            *output_error = Some(error);
            Err(stop_after_output_error())
        }
    }
}

fn finish_output_visit<T>(
    result: Result<T, StoreError>,
    output_error: Option<DataOutputError>,
) -> Result<T, DataOutputError> {
    match output_error {
        Some(error) => Err(error),
        None => result.map_err(Into::into),
    }
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
            // A bare `marrow data <projectdir>` puts the project path where a subcommand belongs,
            // so the path reads as an unknown subcommand. When the token names a real directory,
            // the developer meant to inspect that project but omitted the subcommand: say so.
            if std::path::Path::new(other).is_dir() {
                eprintln!(
                    "marrow data requires a subcommand: roots, stats, dump, integrity, recover, or get"
                );
            } else {
                eprintln!("unknown data subcommand: {other}");
                eprintln!("expected `roots`, `stats`, `dump`, `integrity`, `recover`, or `get`");
            }
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
    if store_path_is_absent(&path) {
        return report_no_store_to_recover(&dir, Some(&path), format);
    }
    // Recover is the repair path for a store read-only commands refuse, so it must not
    // require the source to check: damaged source text must not block a store open. The
    // index-completeness cross-check needs the schema, so it runs only when the project
    // checks cleanly; a project that does not check still gets the store-level repair.
    // The probe is silent: a failed read-only store open or a failed check must not write
    // its own error envelope to the recover command's single structured-report object.
    let program = probe_checked_project(&dir, &config);
    match recover_store(&path, program.as_ref()) {
        Ok(()) => report_recovered_store(&dir, &path, format),
        Err(error) => report_store_error(error, format),
    }
}

/// Attempt a write-capable repair of the store at `path`, then prove the repair
/// converged.
///
/// The write-capable open replays an interrupted commit but does not traverse the
/// data tree; a store damaged below its table roots opens cleanly and only faults
/// when read, so the first pass walks it to reject damage rather than bless it. The
/// repaired handle is then dropped, letting redb persist whatever clean state the
/// replay produced.
///
/// Some allocator- and slot-region damage replays into a handle that reads cleanly
/// yet leaves the on-disk file still demanding recovery on the next open, so a
/// single write-open pass would report success while every following read faulted
/// and re-running recover never converged. To rule that out, recover finally proves
/// the store is convergently readable the way the next command opens it: a fresh
/// read-only open and traversal. If that fresh open or walk fails, the store was not
/// made readable and recover reports corruption rather than a false repair.
fn recover_store(
    path: &std::path::Path,
    program: Option<&CheckedProgram>,
) -> Result<(), StoreError> {
    {
        let store = TreeStore::open_existing(path)?;
        store.verify_readable()?;
        if let Some(program) = program {
            verify_store_index_integrity(&store, program)?;
        }
    }
    let reopened = TreeStore::open_read_only(path).map_err(recovery_not_converged)?;
    reopened.verify_readable().map_err(recovery_not_converged)?;
    if let Some(program) = program {
        verify_store_index_integrity(&reopened, program).map_err(recovery_not_converged)?;
    }
    Ok(())
}

/// Map a fresh-open or fresh-traversal failure after a repair attempt to the
/// corruption recover must report. A store still asking for recovery, or one whose
/// re-open traversal faults, was not made readable: it is damaged beyond repair, not
/// a clean recoverable store, so `store.recovery_required` would wrongly invite the
/// developer to run recover again on a store recover cannot fix.
fn recovery_not_converged(error: StoreError) -> StoreError {
    match error {
        StoreError::Corruption { .. } => error,
        _ => StoreError::Corruption {
            message: "the store could not be made readable by recovery".into(),
        },
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
    let (roots, store_snapshot) = match &store {
        Some(store) => match stamped_data_roots_in_store(&program, store) {
            Ok(StampedData { data, stamp }) => (data, Some(stamp)),
            Err(error) => return report_store_error(error, format),
        },
        None => (Vec::new(), None),
    };
    warn_on_hidden_orphans(&program, &store);
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
            write_json(json!({
                "project": crate::project_json_path(&dir),
                "roots": roots,
                "store_snapshot": store_snapshot
                    .as_ref()
                    .map(marrow_json::data_generation_stamp_to_json),
            }));
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
    // One snapshot spans every pass, so the root, entity, and cell counts describe the
    // same coherent version of the store.
    let _snapshot = match pin_snapshot(&store, format) {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };
    let (roots, records, cells) = match &store {
        Some(store) => {
            let roots = match data_roots_in_store(&program, store) {
                Ok(roots) => roots.len(),
                Err(error) => return report_store_error(error, format),
            };
            let records = match crate::backup::count_live_entities(&program, store) {
                Ok(records) => records,
                Err(error) => return report_store_error(error, format),
            };
            let cells = match count_data_records(&program, store) {
                Ok(cells) => cells,
                Err(error) => return report_store_error(error, format),
            };
            (roots, records, cells)
        }
        None => (0, 0, 0),
    };
    warn_on_hidden_orphans(&program, &store);
    let store_snapshot = match (&store, format) {
        (Some(store), CheckFormat::Json | CheckFormat::Jsonl) => {
            match data_snapshot_stamp(&program, store) {
                Ok(stamp) => Some(stamp),
                Err(error) => return report_store_error(error, format),
            }
        }
        _ => None,
    };
    match format {
        CheckFormat::Text => {
            println!("roots: {roots}");
            println!("records: {records}");
            println!("cells: {cells}");
        }
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "project": crate::project_json_path(&dir),
                "roots": roots,
                "records": records,
                "cells": cells,
                "store_snapshot": store_snapshot
                    .as_ref()
                    .map(marrow_json::data_generation_stamp_to_json),
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
    warn_on_hidden_orphans(&program, &store);
    let store_snapshot = match (&store, format) {
        (Some(store), CheckFormat::Json | CheckFormat::Jsonl) => {
            match data_snapshot_stamp(&program, store) {
                Ok(stamp) => Some(stamp),
                Err(error) => return report_store_error(error, format),
            }
        }
        _ => None,
    };
    let result = match format {
        CheckFormat::Text => render_dump_text(&program, &store, records).map_err(Into::into),
        CheckFormat::Json => render_dump_json(&dir, &program, &store, store_snapshot.as_ref()),
        CheckFormat::Jsonl => render_dump_jsonl(&program, &store, records, store_snapshot.as_ref())
            .map_err(Into::into),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report_data_output_error(error, format),
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
    store_snapshot: Option<&marrow_check::tooling::DataSnapshotStamp>,
) -> Result<(), DataOutputError> {
    match store {
        Some(store) => write_dump_json(dir, program, store, store_snapshot),
        None => {
            write_json(json!({
                "project": crate::project_json_path(dir),
                "cells": [],
                "store_snapshot": serde_json::Value::Null,
            }));
            Ok(())
        }
    }
}

fn render_dump_jsonl(
    program: &CheckedProgram,
    store: &Option<TreeStore>,
    records: usize,
    store_snapshot: Option<&marrow_check::tooling::DataSnapshotStamp>,
) -> Result<(), StoreError> {
    if let Some(store) = store {
        visit_data_records(program, store, |record| {
            write_json(dump_record(&record.path, record.payload.as_bytes()));
            Ok(())
        })?;
    }
    write_json(json!({
        "kind": "summary",
        "cells": records,
        "store_snapshot": store_snapshot
            .map(marrow_json::data_generation_stamp_to_json),
    }));
    Ok(())
}

fn write_dump_json(
    dir: &str,
    program: &CheckedProgram,
    store: &TreeStore,
    store_snapshot: Option<&marrow_check::tooling::DataSnapshotStamp>,
) -> Result<(), DataOutputError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write_json_array_envelope(
        &mut out,
        |out| {
            write!(out, "\"project\":")?;
            serde_json::to_writer(&mut *out, &crate::project_json_path(dir))
                .map_err(DataOutputError::from_json)?;
            write!(out, ",\"store_snapshot\":")?;
            serde_json::to_writer(
                &mut *out,
                &store_snapshot.map(marrow_json::data_generation_stamp_to_json),
            )
            .map_err(DataOutputError::from_json)
        },
        "cells",
        |emit| {
            let mut output_error = None;
            let result = visit_data_records(program, store, |record| {
                stop_on_output_error(
                    &mut output_error,
                    emit(&dump_record(&record.path, record.payload.as_bytes())),
                )
            });
            finish_output_visit(result, output_error).map(|_| ())
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
    write_prefix: impl FnOnce(&mut dyn Write) -> Result<(), DataOutputError>,
    array_field: &str,
    visit: impl FnOnce(
        &mut dyn FnMut(&serde_json::Value) -> Result<(), DataOutputError>,
    ) -> Result<(), DataOutputError>,
) -> Result<(), DataOutputError> {
    write!(out, "{{")?;
    write_prefix(out)?;
    write!(out, ",\"{array_field}\":[")?;
    {
        let mut first = true;
        let mut emit = |item: &serde_json::Value| {
            if !first {
                write!(out, ",")?;
            }
            first = false;
            serde_json::to_writer(&mut *out, item).map_err(DataOutputError::from_json)
        };
        visit(&mut emit)?;
    }
    writeln!(out, "]}}")?;
    Ok(())
}

fn dump_record(path: &str, value: &[u8]) -> serde_json::Value {
    json!({
        "path": path,
        "value_b64": marrow_run::base64::encode(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};

    struct FailOnWrite {
        writes: usize,
        fail_on: usize,
    }

    impl FailOnWrite {
        fn new(fail_on: usize) -> Self {
            Self { writes: 0, fail_on }
        }
    }

    impl Write for FailOnWrite {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self.writes == self.fail_on {
                return Err(io::Error::other("writer failed"));
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn write_test_prefix(out: &mut dyn Write) -> Result<(), DataOutputError> {
        write!(out, "\"project\":null")?;
        Ok(())
    }

    fn assert_io_error(result: Result<(), DataOutputError>) {
        assert!(matches!(result, Err(DataOutputError::Io(_))));
    }

    #[test]
    fn json_envelope_reports_prefix_write_failure() {
        let mut out = FailOnWrite::new(2);

        let result = write_json_array_envelope(&mut out, write_test_prefix, "items", |_| Ok(()));

        assert_io_error(result);
    }

    #[test]
    fn json_envelope_reports_item_write_failure() {
        let mut out = FailOnWrite::new(4);

        let result = write_json_array_envelope(&mut out, write_test_prefix, "items", |emit| {
            emit(&json!({ "item": 1 }))?;
            Ok(())
        });

        assert_io_error(result);
    }

    #[test]
    fn json_envelope_reports_closing_write_failure() {
        let mut out = FailOnWrite::new(4);

        let result = write_json_array_envelope(&mut out, write_test_prefix, "items", |_| Ok(()));

        assert_io_error(result);
    }

    #[test]
    fn json_envelope_preserves_store_traversal_errors() {
        let mut out = Vec::new();

        let result = write_json_array_envelope(&mut out, write_test_prefix, "items", |_| {
            Err(DataOutputError::Store(StoreError::Corruption {
                message: "bad cell".into(),
            }))
        });

        assert!(matches!(
            result,
            Err(DataOutputError::Store(StoreError::Corruption { message }))
                if message == "bad cell"
        ));
    }
}
