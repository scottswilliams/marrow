use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_syntax::Diagnose;
use serde_json::json;

mod backup;
mod cmd_backup;
mod cmd_check;
mod cmd_data;
mod cmd_evolve;
mod cmd_fmt;
mod cmd_restore;
mod cmd_run;
mod cmd_test;
mod dry_run;
mod trace;

const HELP: &str = "\
Marrow

Usage:
  marrow check [--data] [--format text|json|jsonl] <file.mw | projectdir>
  marrow evolve <preview|apply> [--format text|json|jsonl] <projectdir>
  marrow fmt [--check | --write] <file.mw | projectdir>
  marrow run [--entry <entry>] [--maintenance] [--trace] [--dry-run] [--format text|json|jsonl] <projectdir>
  marrow test [--trace] [--format text|json|jsonl] <projectdir>
  marrow data <roots|stats|dump|integrity|recover> <projectdir>
  marrow data get <projectdir> <path>
  marrow backup [--format text|json|jsonl] <projectdir> <output-file>
  marrow restore [--format text|json|jsonl] <projectdir> <backup-file>
  marrow --version
  marrow --help
";

fn main() -> ExitCode {
    install_broken_pipe_exit();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some((command, rest)) = args.split_first() else {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    };
    // Every command that parses, checks, or runs untrusted `.mw` source recurses
    // over the source on the call stack, so dispatch on a worker thread with a
    // generous stack. The recursion guards in the parser and runtime are sized to
    // trip far inside this stack, so deeply nested source or runaway recursion
    // surfaces a typed `check.nesting_limit` / `run.recursion_limit` diagnostic
    // instead of aborting the process with a native stack overflow.
    let command = command.clone();
    let rest = rest.to_vec();
    run_on_worker_stack(move || dispatch(&command, &rest))
}

fn dispatch(command: &str, rest: &[String]) -> ExitCode {
    match command {
        "check" => cmd_check::check(rest),
        "evolve" => cmd_evolve::evolve(rest),
        "fmt" => cmd_fmt::fmt(rest),
        "run" => cmd_run::run(rest),
        "test" => cmd_test::test(rest),
        "data" => cmd_data::data(rest),
        "backup" => cmd_backup::backup(rest),
        "restore" => cmd_restore::restore(rest),
        "--help" | "-h" | "help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        "--version" | "-V" | "version" => {
            println!("marrow {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command: {other}");
            eprintln!("run `marrow --help` for available commands");
            ExitCode::from(2)
        }
    }
}

/// The stack the parse/check/run pipeline runs on. 256 MiB comfortably holds the
/// recursion the typed limits permit — 256 nested parser frames and 1024 runtime
/// call frames — with wide margin, so a limit always trips before the stack does.
const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Run `command` on a worker thread with [`WORKER_STACK_BYTES`] of stack and
/// return its exit code. The main thread only waits, so the deep recursion the
/// parser and runtime perform over untrusted source has room to reach a typed
/// depth-limit diagnostic rather than overflowing the default main-thread stack.
/// A panic on the worker (a genuine bug, not a depth limit) is re-raised on the
/// main thread so it surfaces the same way it would unthreaded.
fn run_on_worker_stack(command: impl FnOnce() -> ExitCode + Send + 'static) -> ExitCode {
    std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(command)
        .expect("spawn the marrow worker thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// Exit cleanly when a downstream reader closes our stdout, instead of panicking.
///
/// Rust ignores `SIGPIPE`, so a write to a pipe whose read end has closed returns
/// `EPIPE` rather than killing the process. The `print!`/`println!` macros turn that
/// error into a panic ("failed printing to stdout: Broken pipe"), and the streaming
/// JSON writers surface the same `BrokenPipe` error through `.expect`. A consumer like
/// `head`, `less`, or `grep -m1` closing the pipe early is normal Unix behavior, not a
/// failure, so we install a panic hook that recognizes that one panic by its payload
/// and exits 0. Every other panic is delegated to the default hook so real crashes
/// still print their message and backtrace. This keeps the fix global without
/// rewriting the CLI's many `print!` sites to handle `EPIPE` individually.
fn install_broken_pipe_exit() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        if message.is_some_and(|message| message.contains("Broken pipe")) {
            std::process::exit(0);
        }
        default_hook(info);
    }));
}

pub(crate) fn report_project(
    target: &str,
    report: &marrow_check::CheckReport,
    format: CheckFormat,
) {
    let status = if report.has_errors() { "failed" } else { "ok" };
    match format {
        CheckFormat::Text => {
            if report.diagnostics.is_empty() {
                println!("ok: {target} checked");
            } else {
                for diagnostic in &report.diagnostics {
                    eprintln!(
                        "{}:{}:{}: {}: {}: {}",
                        diagnostic.file.display(),
                        diagnostic.span.line,
                        diagnostic.span.column,
                        diagnostic.severity.as_str(),
                        diagnostic.code,
                        diagnostic.message
                    );
                }
            }
        }
        CheckFormat::Json | CheckFormat::Jsonl => report_diagnostic_records(
            format,
            report.diagnostics.iter().map(check_diagnostic_record),
            serde_json::Map::from_iter([
                ("project".into(), json!(target)),
                ("status".into(), json!(status)),
            ]),
        ),
    }
}

/// `json` nests the records under `diagnostics` in one envelope; `jsonl` streams
/// each record followed by a `summary` line carrying the count. Callers route only
/// `json`/`jsonl` here; text stays caller-specific.
fn report_diagnostic_records(
    format: CheckFormat,
    records: impl Iterator<Item = serde_json::Value>,
    envelope: serde_json::Map<String, serde_json::Value>,
) {
    let records: Vec<serde_json::Value> = records.collect();
    match format {
        CheckFormat::Json => {
            let mut record = envelope;
            record.insert("diagnostics".into(), json!(records));
            write_json(serde_json::Value::Object(record));
        }
        CheckFormat::Jsonl => {
            for record in &records {
                write_json(record.clone());
            }
            let mut summary = envelope;
            summary.insert("kind".into(), json!("summary"));
            summary.insert("diagnostics".into(), json!(records.len()));
            write_json(serde_json::Value::Object(summary));
        }
        CheckFormat::Text => {}
    }
}

/// The JSON envelope shared by every diagnostic: `code`/`kind`/`message` plus a
/// per-source `source_span`. The optional `severity`/`help` are passed in rather
/// than read off the trait so an absent key stays exactly absent.
pub(crate) fn envelope(
    diagnostic: &dyn Diagnose,
    source_span: serde_json::Value,
    severity: Option<&str>,
    help: Option<Option<&str>>,
) -> serde_json::Value {
    let mut record = json!({
        "code": diagnostic.code(),
        "kind": diagnostic.kind(),
        "message": diagnostic.message(),
        "source_span": source_span,
    });
    let object = record.as_object_mut().expect("json! built an object");
    if let Some(severity) = severity {
        object.insert("severity".into(), json!(severity));
    }
    if let Some(help) = help {
        object.insert("help".into(), json!(help));
    }
    record
}

/// Project diagnostics carry no `help` or byte offsets: they are reported at a
/// declaration site rather than a byte span.
fn check_diagnostic_record(diagnostic: &marrow_check::CheckDiagnostic) -> serde_json::Value {
    envelope(
        diagnostic,
        json!({
            "file": diagnostic.file.display().to_string(),
            "line": diagnostic.span.line,
            "column": diagnostic.span.column,
        }),
        Some(diagnostic.severity.as_str()),
        None,
    )
}

pub(crate) fn report_simple_error(code: &str, message: &str, format: CheckFormat) {
    match format {
        CheckFormat::Text => eprintln!("{code}: {message}"),
        CheckFormat::Json | CheckFormat::Jsonl => write_json(json!({
            "code": code,
            "kind": marrow_syntax::kind_for_code(code),
            "message": message,
            "source_span": null,
        })),
    }
}

/// Parse `marrow <command> [--format ...] <projectdir> <path>`; `path_label` names
/// the second argument in usage and errors.
pub(crate) fn dir_and_path_args(
    command: &str,
    path_label: &str,
    args: &[String],
) -> Result<(String, String, CheckFormat), ExitCode> {
    let mut positionals = Vec::new();
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                parse_format_flag(args, &mut index, &mut saw_format, &mut format)?;
            }
            "--help" | "-h" => {
                print!(
                    "Usage:\n  marrow {command} [--format text|json|jsonl] <projectdir> <{path_label}>\n"
                );
                return Err(ExitCode::SUCCESS);
            }
            value if value.starts_with('-') => return Err(unknown_option(command, value)),
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    match positionals.as_slice() {
        [dir, path] => Ok((dir.clone(), path.clone(), format)),
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

/// The native backend's redb file path, or `Ok(None)` for the in-memory default.
/// No filesystem side effects.
pub(crate) fn native_store_path(
    dir: &str,
    config: &marrow_project::ProjectConfig,
) -> Result<Option<PathBuf>, ExitCode> {
    match &config.store {
        None
        | Some(marrow_project::StoreConfig {
            backend: marrow_project::StoreBackend::Memory,
            ..
        }) => Ok(None),
        Some(marrow_project::StoreConfig {
            backend: marrow_project::StoreBackend::Native,
            data_dir,
        }) => {
            let data_dir = data_dir
                .as_deref()
                .expect("parse_config guarantees a native store has a dataDir");
            Ok(Some(Path::new(dir).join(data_dir).join("marrow.redb")))
        }
    }
}

/// Like [`native_store_path`], but creates the data directory so the store can be
/// opened for writing.
pub(crate) fn resolve_store_path(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<PathBuf>, ExitCode> {
    let Some(path) = native_store_path(dir, config)? else {
        return Ok(None);
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            report_io_error(&parent.display().to_string(), &error, format);
            ExitCode::FAILURE
        })?;
    }
    Ok(Some(path))
}

/// Open the project's store read-only, or `Ok(None)` if it holds no saved data on
/// disk yet (in-memory default, or the native file does not exist). Never creates.
pub(crate) fn open_store_for_inspection(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<marrow_store::tree::TreeStore>, ExitCode> {
    let Some(path) = native_store_path(dir, config)? else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    match marrow_store::tree::TreeStore::open_read_only(&path) {
        Ok(store) => Ok(Some(store)),
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), format);
            Err(ExitCode::FAILURE)
        }
    }
}

/// Load `<dir>/marrow.json`, reporting an exit code if it is missing or invalid.
pub(crate) fn load_config(dir: &str) -> Result<marrow_project::ProjectConfig, ExitCode> {
    load_config_with_format(dir, CheckFormat::Text)
}

pub(crate) fn load_config_with_format(
    dir: &str,
    format: CheckFormat,
) -> Result<marrow_project::ProjectConfig, ExitCode> {
    let config_path = Path::new(dir).join("marrow.json");
    let config_text = std::fs::read_to_string(&config_path).map_err(|error| {
        report_io_error(&config_path.display().to_string(), &error, format);
        ExitCode::FAILURE
    })?;
    marrow_project::parse_config(&config_text).map_err(|error| {
        report_simple_error(error.code, &error.message, format);
        ExitCode::FAILURE
    })
}

/// Read the project's accepted-catalog snapshot from its engine-resident store, the
/// input the analysis core binds durable identity against. The store is opened
/// read-only and never created: a project on the in-memory backend, or one whose
/// native store file does not exist yet, has no accepted catalog (a first run). A
/// store-open or decode error surfaces as a typed exit code rather than a silent
/// `None`, so a corrupt store fails the check instead of being mistaken for a first
/// run. This is the one owner of "open the store read-only and read its accepted
/// snapshot", shared by every read path.
pub(crate) fn read_accepted_store_catalog(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ExitCode> {
    let Some(store) = open_store_for_inspection(dir, config, format)? else {
        return Ok(None);
    };
    store.read_catalog_snapshot().map_err(|error| {
        report_simple_error(error.code(), &error.to_string(), format);
        ExitCode::FAILURE
    })
}

/// Load the config and check the project. On any failure (config, unreadable
/// source, or check errors) the problem is reported and an exit code returned.
pub(crate) fn load_checked_project(
    dir: &str,
) -> Result<(marrow_project::ProjectConfig, marrow_check::CheckedProgram), ExitCode> {
    load_checked_project_with_format(dir, CheckFormat::Text)
}

pub(crate) fn load_checked_project_with_format(
    dir: &str,
    format: CheckFormat,
) -> Result<(marrow_project::ProjectConfig, marrow_check::CheckedProgram), ExitCode> {
    let config = load_config_with_format(dir, format)?;
    let accepted = read_accepted_store_catalog(dir, &config, format)?;
    let (report, program) =
        marrow_check::check_project_with_catalog(Path::new(dir), &config, accepted.as_ref())
            .map_err(|error| {
                report_simple_error(
                    error.code,
                    &format!("{}: {}", error.path.display(), error.message),
                    format,
                );
                ExitCode::FAILURE
            })?;
    if report.has_errors() {
        report_project(dir, &report, format);
        return Err(ExitCode::FAILURE);
    }
    Ok((config, program))
}

/// Freeze a project's pending durable identity into the write `store` as its baseline,
/// then re-check the program against the now-accepted store snapshot so the caller binds
/// the frozen identity. A store that already holds an accepted catalog, or a program
/// proposing none, writes nothing and the program is returned unchanged — a project past
/// its baseline never churns the catalog rows or the commit stamp. Shared by the
/// state-establishing flows (`run` and `evolve apply`).
pub(crate) fn establish_store_baseline(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    store: &marrow_store::tree::TreeStore,
    program: marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<marrow_check::CheckedProgram, ExitCode> {
    let wrote =
        marrow_run::evolution::commit_catalog_baseline(store, &program).map_err(|error| {
            report_simple_error(error.code(), &error.to_string(), format);
            ExitCode::FAILURE
        })?;
    if !wrote {
        return Ok(program);
    }
    recheck_against_store_catalog(dir, config, store, format)
}

/// Re-check the project binding durable identity against the store's accepted catalog
/// snapshot. Called after a baseline commit so the caller binds the freshly accepted
/// epoch rather than the pending one it loaded.
pub(crate) fn recheck_against_store_catalog(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    store: &marrow_store::tree::TreeStore,
    format: CheckFormat,
) -> Result<marrow_check::CheckedProgram, ExitCode> {
    let accepted = store.read_catalog_snapshot().map_err(|error| {
        report_simple_error(error.code(), &error.to_string(), format);
        ExitCode::FAILURE
    })?;
    let (report, program) =
        marrow_check::check_project_with_catalog(Path::new(dir), config, accepted.as_ref())
            .map_err(|error| {
                report_simple_error(
                    error.code,
                    &format!("{}: {}", error.path.display(), error.message),
                    format,
                );
                ExitCode::FAILURE
            })?;
    if report.has_errors() {
        report_project(dir, &report, format);
        return Err(ExitCode::FAILURE);
    }
    Ok(program)
}

#[derive(Clone, Copy)]
pub(crate) enum CheckFormat {
    Text,
    Json,
    Jsonl,
}

impl CheckFormat {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            "jsonl" => Some(Self::Jsonl),
            _ => None,
        }
    }
}

/// The one owner of the `--format <value>` grammar. `index` points at the
/// `--format` token and is advanced past its value. A missing value, unknown
/// format, or duplicate flag is a usage error (exit code 2).
pub(crate) fn parse_format_flag(
    args: &[String],
    index: &mut usize,
    saw_format: &mut bool,
    format: &mut CheckFormat,
) -> Result<(), ExitCode> {
    if *saw_format {
        eprintln!("duplicate --format");
        return Err(ExitCode::from(2));
    }
    *saw_format = true;
    *index += 1;
    let Some(value) = args.get(*index) else {
        eprintln!("missing value for --format");
        return Err(ExitCode::from(2));
    };
    let Some(parsed) = CheckFormat::parse(value) else {
        eprintln!("unknown format: {value}");
        return Err(ExitCode::from(2));
    };
    *format = parsed;
    Ok(())
}

pub(crate) fn unknown_option(command: &str, value: &str) -> ExitCode {
    eprintln!("unknown {command} option: {value}");
    ExitCode::from(2)
}

/// Record one positional `target` into `slot`, rejecting a second one.
/// `target_label` names what the command takes so the error reads naturally.
pub(crate) fn take_single_target(
    slot: &mut Option<String>,
    target: &str,
    command: &str,
    target_label: &str,
) -> Result<(), ExitCode> {
    if slot.replace(target.to_string()).is_some() {
        eprintln!("marrow {command} accepts one {target_label}");
        return Err(ExitCode::from(2));
    }
    Ok(())
}

pub(crate) fn report_check(file: &str, parsed: &marrow_syntax::ParsedSource, format: CheckFormat) {
    match format {
        CheckFormat::Text => {
            if parsed.diagnostics.is_empty() {
                println!(
                    "ok: {file} parsed ({} declaration{})",
                    parsed.file.declarations.len(),
                    if parsed.file.declarations.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            } else {
                for diagnostic in &parsed.diagnostics {
                    eprintln!("{file}:{diagnostic}");
                    if let Some(help) = &diagnostic.help {
                        eprintln!("help: {help}");
                    }
                }
            }
        }
        CheckFormat::Json | CheckFormat::Jsonl => report_diagnostic_records(
            format,
            parsed
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic_record(file, diagnostic)),
            serde_json::Map::from_iter([
                ("file".into(), json!(file)),
                (
                    "status".into(),
                    json!(if parsed.has_errors() { "failed" } else { "ok" }),
                ),
                ("declarations".into(), json!(parsed.file.declarations.len())),
            ]),
        ),
    }
}

pub(crate) fn report_io_error(file: &str, error: &std::io::Error, format: CheckFormat) {
    match format {
        CheckFormat::Text => eprintln!("io.read: failed to read {file}: {error}"),
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "code": "io.read",
                "kind": "io",
                "message": format!("failed to read {file}: {error}"),
                "source_span": null,
            }));
        }
    }
}

fn diagnostic_record(file: &str, diagnostic: &marrow_syntax::Diagnostic) -> serde_json::Value {
    envelope(
        diagnostic,
        json!({
            "file": file,
            "line": diagnostic.span.line,
            "column": diagnostic.span.column,
            "start_byte": diagnostic.span.start_byte,
            "end_byte": diagnostic.span.end_byte,
        }),
        Some(diagnostic.severity.as_str()),
        Some(diagnostic.help()),
    )
}

pub(crate) fn write_json(value: serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string(&value).expect("JSON value should serialize")
    );
}

/// Emit one JSON record on standard error. Tooling reports (the trace and dry-run
/// plan) use this so they never interleave with the program's own stdout output.
pub(crate) fn write_json_err(value: serde_json::Value) {
    eprintln!(
        "{}",
        serde_json::to_string(&value).expect("JSON value should serialize")
    );
}

/// Append the lowercase hex of `bytes` to `out` (two digits per byte, no prefix).
/// Writing into the caller's buffer avoids a per-byte allocation.
pub(crate) fn push_hex(out: &mut String, bytes: &[u8]) {
    use std::fmt::Write;
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
}

/// Allocate and return the lowercase hex string of `bytes`. The single owner of
/// the digest-to-hex conversion.
pub(crate) fn hex_string(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    push_hex(&mut text, bytes);
    text
}

/// The one owner of raw saved bytes display for execution traces: UTF-8 text when
/// valid, otherwise `0x<hex>`.
pub(crate) fn render_value_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let mut text = String::from("0x");
            push_hex(&mut text, bytes);
            text
        }
    }
}
