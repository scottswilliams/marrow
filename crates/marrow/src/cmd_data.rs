//! `marrow data`: read-only inspection of a project's saved data, plus the
//! schema-typed integrity check and its saved-path rendering.

use std::io::{self, Write};
use std::process::ExitCode;

use marrow_store::backend::{Backend, StoreError};
use marrow_store::path::display_path;
use marrow_syntax::Diagnose;
use serde_json::json;

use crate::{
    CheckFormat, envelope, load_checked_project, load_config, open_store_for_inspection,
    report_simple_error, write_json,
};

const DATA_SCAN_LIMIT: usize = 128;

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

/// Parse `data get`'s arguments: a project directory, a path string, and an
/// optional `--format`, rejecting options and a wrong positional count.
fn data_get_args(args: &[String]) -> Result<(String, String, CheckFormat), ExitCode> {
    let mut positionals = Vec::new();
    let mut format = CheckFormat::Text;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                index += 1;
                format = parse_format_value(args.get(index))?;
            }
            "--help" | "-h" => {
                print!(
                    "Usage:\n  marrow data get [--format text|json|jsonl] <projectdir> <path>\n"
                );
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => {
                eprintln!("unknown data get option: {value}");
                return Err(ExitCode::from(2));
            }
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    match positionals.as_slice() {
        [dir, path] => Ok((dir.clone(), path.clone(), format)),
        [] | [_] => {
            eprintln!("marrow data get requires a project directory and a path");
            Err(ExitCode::from(2))
        }
        _ => {
            eprintln!("marrow data get accepts one project directory and one path");
            Err(ExitCode::from(2))
        }
    }
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

fn scan_saved_records(
    store: &dyn Backend,
    mut visit: impl FnMut(&[u8], &[u8]) -> Result<(), StoreError>,
) -> Result<usize, StoreError> {
    let mut cursor = None;
    let mut records = 0usize;
    loop {
        let page = match cursor.as_deref() {
            Some(cursor) => store.scan_after(&[], cursor, DATA_SCAN_LIMIT)?,
            None => store.scan(&[], DATA_SCAN_LIMIT)?,
        };
        records = records
            .checked_add(page.entries.len())
            .ok_or(StoreError::LimitExceeded {
                limit: "data record count",
            })?;
        for (path, value) in &page.entries {
            visit(path, value)?;
        }
        if !page.truncated {
            return Ok(records);
        }
        let Some((last_path, _)) = page.entries.last() else {
            return Err(StoreError::InvalidCursor {
                message: "bounded data scan truncated without a cursor".into(),
            });
        };
        cursor = Some(last_path.clone());
    }
}

fn count_saved_records(store: &dyn Backend) -> Result<usize, StoreError> {
    scan_saved_records(store, |_, _| Ok(()))
}

fn report_store_error(error: StoreError, format: CheckFormat) -> ExitCode {
    report_simple_error(error.code(), &error.to_string(), format);
    ExitCode::FAILURE
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
  marrow data integrity [--format text|json|jsonl] <dir>   verify saved values decode
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
        "integrity" => data_integrity(rest),
        "get" => data_get(rest),
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
    let config = match load_config(&dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let store = match open_store_for_inspection(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let roots = match &store {
        Some(store) => match store.roots() {
            Ok(roots) => roots,
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), format);
                return ExitCode::FAILURE;
            }
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
    let config = match load_config(&dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let store = match open_store_for_inspection(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let (roots, records) = match &store {
        Some(store) => {
            let roots = match store.roots() {
                Ok(roots) => roots.len(),
                Err(error) => {
                    report_simple_error(error.code(), &error.to_string(), format);
                    return ExitCode::FAILURE;
                }
            };
            let records = match count_saved_records(store.as_ref()) {
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

/// `marrow data dump`: print every stored `(path, value)` in encoded order. Raw
/// inspection — values render as their canonical bytes (UTF-8 text or `0x<hex>`),
/// not schema-typed, so dump works without source.
fn data_dump(args: &[String]) -> ExitCode {
    let (dir, format) = match one_positional_with_format("data dump", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let config = match load_config(&dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let store = match open_store_for_inspection(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let records = match &store {
        Some(store) => match count_saved_records(store.as_ref()) {
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
                if let Err(error) = scan_saved_records(store.as_ref(), |path, value| {
                    println!("{}\t{}", display_path(path), render_value_bytes(value));
                    Ok(())
                }) {
                    return report_store_error(error, format);
                }
            }
        }
        CheckFormat::Json => {
            if let Some(store) = &store {
                if let Err(error) = write_dump_json(&dir, store.as_ref()) {
                    return report_store_error(error, format);
                }
            } else {
                write_json(json!({ "project": dir, "records": [] }));
            }
        }
        CheckFormat::Jsonl => {
            if let Some(store) = &store {
                let result = scan_saved_records(store.as_ref(), |path, value| {
                    write_json(dump_record(path, value));
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

fn write_dump_json(dir: &str, store: &dyn Backend) -> Result<(), StoreError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "{{\"project\":").expect("write dump JSON");
    serde_json::to_writer(&mut out, dir).expect("serialize project path");
    write!(out, ",\"records\":[").expect("write dump JSON");
    let mut first = true;
    scan_saved_records(store, |path, value| {
        if !first {
            write!(out, ",").expect("write dump JSON separator");
        }
        first = false;
        serde_json::to_writer(&mut out, &dump_record(path, value)).expect("serialize dump record");
        Ok(())
    })?;
    writeln!(out, "]}}").expect("write dump JSON");
    Ok(())
}

/// Render a dump record as JSON: the human path plus base64 of the exact path and
/// value bytes, so a machine consumer reads them losslessly while a person reads
/// `path`. Uses the same base64 codec `serve` uses.
fn dump_record(path: &[u8], value: &[u8]) -> serde_json::Value {
    json!({
        "path": display_path(path),
        "path_b64": marrow_run::base64::encode(path),
        "value_b64": marrow_run::base64::encode(value),
    })
}

/// Render stored value bytes for raw text inspection: as a UTF-8 string when
/// valid (the common case, since canonical forms are ASCII text), else as
/// `0x<hex>`. This shows the canonical stored bytes honestly, never guessing a
/// type — dump and get work without source, and the trace and dry-run reports
/// render planned values the same way.
pub(crate) fn render_value_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let mut hex = String::from("0x");
            for byte in bytes {
                hex.push_str(&format!("{byte:02x}"));
            }
            hex
        }
    }
}

/// `marrow data integrity`: verify every stored value decodes against its
/// declared schema type, reporting decode mismatches, orphan data, and corrupt
/// keys. Read-only and typed — it needs the checked project to know each path's
/// type. Exits `1` when any problem is found, `0` on a clean store.
fn data_integrity(args: &[String]) -> ExitCode {
    let (dir, format) = match one_positional_with_format("data integrity", args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let store = match open_store_for_inspection(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let (records, problems) = match &store {
        Some(store) => match count_integrity_problems(store.as_ref(), &program) {
            Ok(counts) => counts,
            Err(error) => return report_store_error(error, format),
        },
        None => (0, 0),
    };

    if let Some(store) = &store {
        if let Err(error) =
            report_integrity(&dir, records, problems, store.as_ref(), &program, format)
        {
            return report_store_error(error, format);
        }
    } else {
        report_empty_integrity(&dir, format);
    }
    if problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn count_integrity_problems(
    store: &dyn Backend,
    program: &marrow_check::CheckedProgram,
) -> Result<(usize, usize), StoreError> {
    let mut problems = 0usize;
    let records = scan_saved_records(store, |path, value| {
        if check_record(program, path, value).is_some() {
            problems = problems.checked_add(1).ok_or(StoreError::LimitExceeded {
                limit: "data integrity problem count",
            })?;
        }
        Ok(())
    })?;
    Ok((records, problems))
}

/// One integrity finding: a dotted code and a message, located at a path string
/// (these findings have no source line, so the location is the saved path).
struct IntegrityProblem {
    code: &'static str,
    path: String,
    message: String,
}

impl Diagnose for IntegrityProblem {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
}

/// Check one stored record against the schema, returning a problem when the path
/// does not decode, names data the schema cannot account for, or holds bytes that
/// are not a canonical form of its declared type.
fn check_record(
    program: &marrow_check::CheckedProgram,
    path: &[u8],
    value: &[u8],
) -> Option<IntegrityProblem> {
    let Some(segments) = marrow_store::path::decode_path(path) else {
        return Some(IntegrityProblem {
            code: "store.corrupt_path",
            path: display_path(path),
            message: "stored key is not a well-formed saved path".into(),
        });
    };
    match marrow_run::classify_saved_path(program, &segments) {
        marrow_run::SavedPathClass::Scalar(ty) => {
            if marrow_store::value::decode_value(value, ty).is_some() {
                None
            } else {
                Some(IntegrityProblem {
                    code: "data.decode",
                    path: display_path(path),
                    message: format!("stored value is not a canonical {} form", ty.name()),
                })
            }
        }
        // A typed-reference leaf stores the referenced identity's canonical
        // encoding; it is sound when those bytes decode back to that many keys whose
        // scalar kinds match the referenced store's declared identity keys. A
        // wrong-scalar key decodes by arity alone, so the byte check passes it — the
        // reference would point at a record the referenced keyspace could never
        // hold, so the inner key type is checked too.
        marrow_run::SavedPathClass::Identity { store_root, arity } => {
            match marrow_run::decode_identity_arity(value, arity) {
                None => Some(IntegrityProblem {
                    code: "data.decode",
                    path: display_path(path),
                    message: format!(
                        "stored value is not a canonical `Id(^{store_root})` encoding"
                    ),
                }),
                Some(keys) => marrow_run::identity_leaf_key_mismatch(program, &store_root, &keys)
                    .map(|(expected, found)| IntegrityProblem {
                        code: "data.key_type",
                        path: display_path(path),
                        message: format!(
                            "stored `Id(^{store_root})` reference has a {} key where the schema \
                             declares {}",
                            found.name(),
                            expected.name()
                        ),
                    }),
            }
        }
        // Generated index entries are raw-only by design; they are legal.
        marrow_run::SavedPathClass::IndexMarker => None,
        marrow_run::SavedPathClass::KeyTypeMismatch { expected, found } => Some(IntegrityProblem {
            code: "data.key_type",
            path: display_path(path),
            message: format!(
                "stored key is a {} where the schema declares {}",
                found.name(),
                expected.name()
            ),
        }),
        marrow_run::SavedPathClass::Orphan => Some(IntegrityProblem {
            code: "data.orphan",
            path: display_path(path),
            message: "saved data under an unknown root or undeclared member".into(),
        }),
    }
}

fn report_integrity(
    dir: &str,
    records: usize,
    problems: usize,
    store: &dyn Backend,
    program: &marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<(), StoreError> {
    match format {
        CheckFormat::Text => {
            if problems == 0 {
                println!("ok: {dir} integrity verified ({records} records)");
            } else {
                write_integrity_problems_text(store, program)?;
            }
        }
        CheckFormat::Json => write_integrity_json(dir, records, store, program)?,
        CheckFormat::Jsonl => {
            write_integrity_problems_jsonl(store, program)?;
            write_json(json!({
                "kind": "summary",
                "records": records,
                "problems": problems,
            }));
        }
    }
    Ok(())
}

fn report_empty_integrity(dir: &str, format: CheckFormat) {
    match format {
        CheckFormat::Text => println!("ok: {dir} integrity verified (0 records)"),
        CheckFormat::Json => write_json(json!({
            "project": dir,
            "records": 0,
            "problems": [],
        })),
        CheckFormat::Jsonl => write_json(json!({
            "kind": "summary",
            "records": 0,
            "problems": 0,
        })),
    }
}

fn write_integrity_problems_text(
    store: &dyn Backend,
    program: &marrow_check::CheckedProgram,
) -> Result<(), StoreError> {
    scan_saved_records(store, |path, value| {
        if let Some(problem) = check_record(program, path, value) {
            eprintln!("{}: {}: {}", problem.path, problem.code, problem.message);
        }
        Ok(())
    })?;
    Ok(())
}

fn write_integrity_problems_jsonl(
    store: &dyn Backend,
    program: &marrow_check::CheckedProgram,
) -> Result<(), StoreError> {
    scan_saved_records(store, |path, value| {
        if let Some(problem) = check_record(program, path, value) {
            write_json(integrity_record(&problem));
        }
        Ok(())
    })?;
    Ok(())
}

fn write_integrity_json(
    dir: &str,
    records: usize,
    store: &dyn Backend,
    program: &marrow_check::CheckedProgram,
) -> Result<(), StoreError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "{{\"project\":").expect("write integrity JSON");
    serde_json::to_writer(&mut out, dir).expect("serialize project path");
    write!(out, ",\"records\":{records},\"problems\":[").expect("write integrity JSON");
    let mut first = true;
    scan_saved_records(store, |path, value| {
        if let Some(problem) = check_record(program, path, value) {
            if !first {
                write!(out, ",").expect("write integrity JSON separator");
            }
            first = false;
            serde_json::to_writer(&mut out, &integrity_record(&problem))
                .expect("serialize integrity problem");
        }
        Ok(())
    })?;
    writeln!(out, "]}}").expect("write integrity JSON");
    Ok(())
}

/// Render an integrity problem as the standard error envelope. These findings
/// have no source line, so the location is a `path` field rather than a span.
fn integrity_record(problem: &IntegrityProblem) -> serde_json::Value {
    envelope(problem, json!({ "path": problem.path }), None, None)
}

/// `marrow data get <projectdir> <path>`: read and print one path's value. Raw
/// like dump (value renders as UTF-8 text or hex); absence is a valid `0` result.
fn data_get(args: &[String]) -> ExitCode {
    let (dir, path_text, format) = match data_get_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    // A malformed path string fails before touching the store: a usage error.
    let segments = match marrow_store::path::parse_path(&path_text) {
        Ok(segments) => segments,
        Err(error) => {
            eprintln!("marrow data get: {}", error.message);
            return ExitCode::from(2);
        }
    };
    let encoded = marrow_store::path::encode_path(&segments);
    let config = match load_config(&dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let store = match open_store_for_inspection(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let value = match &store {
        Some(store) => match store.read(&encoded) {
            Ok(value) => value,
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), format);
                return ExitCode::FAILURE;
            }
        },
        // No store on disk yet: the path is simply absent.
        None => None,
    };
    let presence = match &store {
        Some(store) => match store.presence(&encoded) {
            Ok(presence) => presence,
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), format);
                return ExitCode::FAILURE;
            }
        },
        None => marrow_store::backend::Presence::Absent,
    };
    match format {
        CheckFormat::Text => match &value {
            Some(bytes) => println!("{}", render_value_bytes(bytes)),
            // A valueless path with children is distinct from a truly absent one.
            None => match presence {
                marrow_store::backend::Presence::ChildrenOnly => {
                    println!("(no value; has children)")
                }
                _ => println!("(absent)"),
            },
        },
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "path": display_path(&encoded),
                "presence": presence_name(presence),
                "value_b64": value.as_ref().map(|bytes| marrow_run::base64::encode(bytes)),
            }));
        }
    }
    ExitCode::SUCCESS
}

/// The presence-state name for the `get` JSON envelope, matching serve's
/// `op_saved_get` spelling.
fn presence_name(presence: marrow_store::backend::Presence) -> &'static str {
    use marrow_store::backend::Presence;
    match presence {
        Presence::Absent => "absent",
        Presence::ValueOnly => "value_only",
        Presence::ChildrenOnly => "children_only",
        Presence::ValueAndChildren => "value_and_children",
    }
}
