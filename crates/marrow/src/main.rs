use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_syntax::Diagnose;
use serde_json::json;

mod backup;
mod cmd_backup;
mod cmd_check;
mod cmd_client;
mod cmd_data;
mod cmd_doctor;
mod cmd_evolve;
mod cmd_fmt;
mod cmd_init;
mod cmd_restore;
mod cmd_run;
mod cmd_serve;
mod cmd_test;
mod dry_run;
mod trace;

const HELP: &str = "\
Marrow

Usage:
  marrow init [--client] <projectdir>
  marrow check [--format text|json|jsonl] [--locked] <projectdir>
  marrow doctor [--format text|json|jsonl] <projectdir>
  marrow evolve preview [--from-backup <artifact>] [--scaffold] [--format text|json|jsonl] <projectdir>
  marrow evolve apply [--maintenance] [--approve-retire <field-path>:<count>]
    [--backup <path> | --no-backup] [--format text|json|jsonl] <projectdir>
  marrow fmt [--check | --write] <file.mw | projectdir>
  marrow run [--entry <entry>] [--arg name=value]... [--maintenance] [--trace] [--dry-run] [--format text|json] <projectdir>
  marrow test [--trace] [--format text|json|jsonl] [--filter <substring>] <projectdir>
  marrow serve [--write] [--watch] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>
  marrow client typescript [--out <path>] <projectdir>
  marrow data <roots|stats|dump|integrity> [--backup <artifact>] [--format text|json|jsonl] <projectdir>
  marrow data recover [--format text|json|jsonl] <projectdir>
  marrow data get [--backup <artifact>] [--format text|json|jsonl] <projectdir> <path>
  marrow backup <projectdir> <output-file>
  marrow restore [--replace --count N] <projectdir> <backup-file>
  marrow --version
  marrow --help
";

fn main() -> ExitCode {
    install_broken_pipe_exit();
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let Some((command, rest)) = args.split_first() else {
        // A bare `marrow` is a usage error, not success: it ran no command. Printing usage to
        // stderr and exiting 2 keeps a forgotten subcommand from passing a CI gate green.
        eprint!("{HELP}");
        return ExitCode::from(2);
    };
    // Every command that parses, checks, or runs untrusted `.mw` source recurses
    // over the source on the call stack, so dispatch on a worker thread with a
    // generous stack. The recursion guards in the parser and runtime are sized to
    // trip far inside this stack, so deeply nested source or runaway recursion
    // surfaces a typed `check.nesting_limit` / `run.depth` diagnostic
    // instead of aborting the process with a native stack overflow.
    let command = command.clone();
    let rest = rest.to_vec();
    run_on_worker_stack(move || dispatch_os(&command, &rest))
}

fn dispatch_os(command: &OsStr, rest: &[OsString]) -> ExitCode {
    if command == "init" {
        return cmd_init::init_os(rest);
    }
    let Some(command) = command.to_str() else {
        eprintln!("unknown command: {}", command.to_string_lossy());
        eprintln!("run `marrow --help` for available commands");
        return ExitCode::from(2);
    };
    let Some(rest) = utf8_args(rest) else {
        report_simple_error(
            "config.invalid",
            "command arguments must be valid UTF-8",
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    };
    dispatch(command, &rest)
}

fn utf8_args(args: &[OsString]) -> Option<Vec<String>> {
    args.iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect()
}

fn dispatch(command: &str, rest: &[String]) -> ExitCode {
    match command {
        "check" => cmd_check::check(rest),
        "doctor" => cmd_doctor::doctor(rest),
        "evolve" => cmd_evolve::evolve(rest),
        "fmt" => cmd_fmt::fmt(rest),
        "run" => cmd_run::run(rest),
        "test" => cmd_test::test(rest),
        "serve" => cmd_serve::serve(rest),
        "client" => cmd_client::client(rest),
        "data" => cmd_data::data(rest),
        "backup" => cmd_backup::backup(rest),
        "restore" => cmd_restore::restore(rest),
        "--help" | "-h" | "help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        "--version" | "-V" | "version" => {
            let profile = marrow_run::evolution::current_engine_profile();
            println!(
                "marrow {} engine-profile=(key=v{}, layout-epoch={}, digest={})",
                env!("CARGO_PKG_VERSION"),
                profile.key_profile_version(),
                profile.layout_epoch(),
                profile.digest_hex()
            );
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
/// recursion the typed limits permit — 256 nested parser frames and 256 runtime
/// call frames — with wide margin, so a limit always trips before the stack does.
const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Run `command` on a worker thread with [`WORKER_STACK_BYTES`] of stack and
/// return its exit code. The main thread only waits, so the deep recursion the
/// parser and runtime perform over untrusted source has room to reach a typed
/// depth-limit diagnostic rather than overflowing the default main-thread stack.
/// A panic on the worker (a genuine bug, not a depth limit) is re-raised on the
/// main thread so it surfaces the same way it would unthreaded.
fn run_on_worker_stack(command: impl FnOnce() -> ExitCode + Send + 'static) -> ExitCode {
    let worker = std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(command);
    run_worker_thread(worker)
}

fn run_worker_thread(worker: std::io::Result<std::thread::JoinHandle<ExitCode>>) -> ExitCode {
    match worker {
        Ok(worker) => worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        Err(error) => {
            report_simple_error(
                "io.thread",
                &format!("failed to spawn Marrow worker thread: {error}"),
                CheckFormat::Text,
            );
            ExitCode::FAILURE
        }
    }
}

/// Exit cleanly when a downstream reader closes our stdout, instead of panicking.
///
/// Rust ignores `SIGPIPE`, so a write to a pipe whose read end has closed returns
/// `EPIPE` rather than killing the process. The `print!`/`println!` macros turn that
/// error into a panic ("failed printing to stdout: Broken pipe"). A consumer like
/// `head`, `less`, or `grep -m1` closing the pipe early is normal Unix behavior, not
/// a failure, so we install a panic hook that recognizes that one panic by its
/// payload and exits 0. Every other panic is delegated to the default hook so real
/// crashes still print their message and backtrace. This keeps the fix global
/// without rewriting the CLI's many `print!` sites to handle `EPIPE` individually.
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
    report_project_with_footprints(target, report, None, format);
}

pub(crate) fn report_project_with_program(
    target: &str,
    report: &marrow_check::CheckReport,
    program: &marrow_check::CheckedProgram,
    format: CheckFormat,
) {
    report_project_with_footprints(target, report, Some(program), format);
}

/// Report a project as failed because of a CLI-level condition the checker itself does not raise,
/// such as a fatal `--locked` stale lock. The structured envelope reports `failed`, carries the
/// project's own diagnostics plus the supplied `diagnostic` record, and omits the success-only
/// footprints and surface descriptors so the envelope status always agrees with the nonzero exit.
pub(crate) fn report_project_failed_with_diagnostic(
    target: &str,
    report: &marrow_check::CheckReport,
    diagnostic: serde_json::Value,
    format: CheckFormat,
) {
    match format {
        CheckFormat::Text => {
            for line in project_diagnostic_lines(report) {
                eprintln!("{line}");
            }
            // The unspanned fatal condition carries an explicit severity that the text line must
            // surface, so an exit-1 error reads as `error:` rather than an unlabeled note.
            let label = match diagnostic["severity"].as_str() {
                Some("warning") => "warning: ",
                _ => "error: ",
            };
            eprintln!(
                "{label}{}: {}",
                diagnostic["code"].as_str().unwrap_or_default(),
                diagnostic["message"].as_str().unwrap_or_default()
            );
        }
        CheckFormat::Json | CheckFormat::Jsonl => report_diagnostic_records(
            format,
            report
                .diagnostics
                .iter()
                .map(check_diagnostic_record)
                .chain(std::iter::once(diagnostic)),
            project_envelope(target, "failed", None),
        ),
    }
}

/// A CLI-level advisory the checker itself does not raise — a stale `marrow.lock` or a stale
/// declared client. Each carries the structured diagnostic for the JSON envelope, the full
/// `code: message` note for stderr, and a short human `summary` folded into the stdout success
/// line so the happy path (which never reads stderr) still sees the advisory and counts it.
pub(crate) struct ProjectAdvisory {
    pub diagnostic: serde_json::Value,
    pub note: String,
    pub summary: String,
}

/// Report a clean (no errors) project check that also surfaces CLI-level advisories the checker
/// itself does not raise. The text path keeps the success line on stdout and the advisory notes on
/// stderr; the JSON envelope keeps its `ok` status and footprints while carrying the advisories as
/// structured diagnostics, so a machine consumer parsing the stdout envelope sees them without
/// reading stderr. Exactly one success envelope is rendered however many advisories apply.
pub(crate) fn report_project_ok_with_advisories(
    target: &str,
    report: &marrow_check::CheckReport,
    program: &marrow_check::CheckedProgram,
    advisories: Vec<ProjectAdvisory>,
    format: CheckFormat,
) {
    match format {
        CheckFormat::Text => {
            // The checker diagnostics (if any) print located on stderr; the advisory full notes
            // follow them there. The success line on stdout folds both the checker warnings and the
            // advisories into one count and inlines each advisory summary, so the happy path that
            // only reads stdout still sees and counts the staleness.
            for line in project_diagnostic_lines(report) {
                eprintln!("{line}");
            }
            for advisory in &advisories {
                eprintln!("{}", advisory.note);
            }
            let warnings = warning_count(report) + advisories.len();
            let summaries: Vec<&str> = advisories
                .iter()
                .map(|advisory| advisory.summary.as_str())
                .collect();
            println!("{}", success_line(target, warnings, &summaries));
        }
        CheckFormat::Json | CheckFormat::Jsonl => report_diagnostic_records(
            format,
            report
                .diagnostics
                .iter()
                .map(check_diagnostic_record)
                .chain(advisories.into_iter().map(|advisory| advisory.diagnostic)),
            project_envelope(target, "ok", Some(program)),
        ),
    }
}

fn warning_count(report: &marrow_check::CheckReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == marrow_syntax::Severity::Warning)
        .count()
}

/// The stdout success line for a clean check: always `ok: <target> checked`, with a parenthetical
/// warning count when any warning or advisory applies, and each advisory summary appended after the
/// count so a stale lock or client is visible without reading stderr.
fn success_line(target: &str, warnings: usize, advisory_summaries: &[&str]) -> String {
    if warnings == 0 {
        return format!("ok: {target} checked");
    }
    let suffix = if warnings == 1 { "warning" } else { "warnings" };
    let mut detail = format!("{warnings} {suffix}");
    for summary in advisory_summaries {
        detail.push_str("; ");
        detail.push_str(summary);
    }
    format!("ok: {target} checked ({detail})")
}

fn report_project_with_footprints(
    target: &str,
    report: &marrow_check::CheckReport,
    program: Option<&marrow_check::CheckedProgram>,
    format: CheckFormat,
) {
    let status = if report.has_errors() { "failed" } else { "ok" };
    match format {
        CheckFormat::Text => {
            if report.diagnostics.is_empty() {
                println!("{}", success_line(target, 0, &[]));
            } else {
                for line in project_diagnostic_lines(report) {
                    eprintln!("{line}");
                }
                if !report.has_errors() {
                    println!("{}", success_line(target, warning_count(report), &[]));
                }
            }
        }
        CheckFormat::Json | CheckFormat::Jsonl => report_diagnostic_records(
            format,
            report.diagnostics.iter().map(check_diagnostic_record),
            project_envelope(target, status, program),
        ),
    }
}

fn project_envelope(
    target: &str,
    status: &str,
    program: Option<&marrow_check::CheckedProgram>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut envelope = serde_json::Map::from_iter([
        ("project".into(), json!(project_json_path(target))),
        ("status".into(), json!(status)),
    ]);
    if let Some(program) = program {
        let surface_abi = marrow_json::surface::SurfaceAbiJson::from_program(program);
        let surface_routes = marrow_json::surface::SurfaceRouteManifestJson::from_abi(&surface_abi);
        envelope.insert(
            "entry_footprints".into(),
            json!(entry_footprint_records(program)),
        );
        envelope.insert("surface_abi".into(), json!(surface_abi));
        envelope.insert("surface_routes".into(), json!(surface_routes));
    }
    envelope
}

pub(crate) fn project_json_path(dir: &str) -> String {
    match std::fs::canonicalize(dir).or_else(|_| std::path::absolute(dir)) {
        Ok(path) => path.display().to_string(),
        Err(_) => dir.to_string(),
    }
}

fn entry_footprint_records(program: &marrow_check::CheckedProgram) -> Vec<serde_json::Value> {
    program
        .entry_footprints()
        .into_iter()
        .map(|footprint| {
            json!({
                "entry": footprint.entry,
                "write_effects_reachable": footprint.write_effects_reachable,
                "stores_read": store_paths(program, &footprint.stores_read),
                "stores_written": store_paths(program, &footprint.stores_written),
                "indexes_touched": index_paths(program, &footprint.indexes_touched),
                "work_shape": work_shape_name(footprint.work_shape),
            })
        })
        .collect()
}

/// Footprints identify stores and indexes by their canonical catalog path rather
/// than the physical `cat_*` id: the path is deterministic at every check, even
/// before a freeze assigns a stable id, and joins to the catalog by path.
fn store_paths(
    program: &marrow_check::CheckedProgram,
    stores: &[marrow_check::StoreId],
) -> Vec<String> {
    stores
        .iter()
        .filter_map(|store| program.store_structural_path(*store))
        .collect()
}

fn index_paths(
    program: &marrow_check::CheckedProgram,
    indexes: &[marrow_check::StoreIndexId],
) -> Vec<String> {
    indexes
        .iter()
        .filter_map(|index| program.store_index_structural_path(*index))
        .collect()
}

fn work_shape_name(shape: marrow_check::WorkShapeClass) -> &'static str {
    match shape {
        marrow_check::WorkShapeClass::ComputeOnly => "compute_only",
        marrow_check::WorkShapeClass::ReadOnly => "read_only",
        marrow_check::WorkShapeClass::WritesSavedData => "writes_saved_data",
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
) -> serde_json::Map<String, serde_json::Value> {
    let mut record = serde_json::Map::from_iter([
        ("code".into(), json!(diagnostic.code())),
        ("kind".into(), json!(diagnostic.kind())),
        ("message".into(), json!(diagnostic.message())),
        ("source_span".into(), source_span),
    ]);
    if let Some(severity) = severity {
        record.insert("severity".into(), json!(severity));
    }
    if let Some(help) = help {
        record.insert("help".into(), json!(help));
    }
    record
}

/// The stderr lines for a project's diagnostics: one located `file:line:col: severity: code:
/// message` line each, plus any suggested-index payload line. Shared by the success path and the
/// failed-with-extra-diagnostic path so both render diagnostics identically.
fn project_diagnostic_lines(report: &marrow_check::CheckReport) -> Vec<String> {
    let mut lines = Vec::new();
    for diagnostic in &report.diagnostics {
        lines.push(format!(
            "{}:{}:{}: {}: {}: {}",
            diagnostic.file.display(),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.severity.as_str(),
            diagnostic.code,
            diagnostic.message
        ));
        if let Some(line) = check_diagnostic_payload_text(diagnostic) {
            lines.push(line);
        }
    }
    lines
}

/// Project diagnostics carry no `help` or byte offsets: they are reported at a
/// declaration site rather than a byte span.
fn check_diagnostic_record(diagnostic: &marrow_check::CheckDiagnostic) -> serde_json::Value {
    serde_json::Value::Object(envelope(
        diagnostic,
        json!({
            "file": diagnostic.file.display().to_string(),
            "line": diagnostic.span.line,
            "column": diagnostic.span.column,
        }),
        Some(diagnostic.severity.as_str()),
        None,
    ))
}

fn check_diagnostic_payload_text(diagnostic: &marrow_check::CheckDiagnostic) -> Option<String> {
    match &diagnostic.payload {
        marrow_check::DiagnosticPayload::SuggestedIndex { declaration } => {
            Some(format!("add: {declaration}"))
        }
        _ => None,
    }
}

pub(crate) fn report_simple_error(code: &str, message: &str, format: CheckFormat) {
    report_simple_error_with_data(code, message, serde_json::Map::new(), format);
}

pub(crate) fn report_simple_error_with_data(
    code: &str,
    message: &str,
    data: serde_json::Map<String, serde_json::Value>,
    format: CheckFormat,
) {
    match format {
        CheckFormat::Text => eprintln!("{code}: {message}"),
        CheckFormat::Json | CheckFormat::Jsonl => write_json(json!({
            "code": code,
            "kind": marrow_syntax::kind_for_code(code),
            "message": message,
            "data": data,
            "source_span": null,
        })),
    }
}

fn report_project_io_error(dir: &str, error: marrow_check::ProjectIoError, format: CheckFormat) {
    match error {
        marrow_check::ProjectIoError::Io { path, error } => {
            report_io_error(&path.display().to_string(), &error, format);
        }
        marrow_check::ProjectIoError::Check { report } => {
            report_project(dir, &report, format);
        }
        marrow_check::ProjectIoError::CheckLoad {
            code,
            path,
            message,
        } => {
            report_simple_error(code, &format!("{}: {message}", path.display()), format);
        }
        error => {
            report_simple_error(error.code(), &error.message(), format);
        }
    }
}

pub(crate) fn project_io_exit(
    dir: &str,
    error: marrow_check::ProjectIoError,
    format: CheckFormat,
) -> ExitCode {
    report_project_io_error(dir, error, format);
    ExitCode::FAILURE
}

/// The native backend's redb file path, or `Ok(None)` for an explicit memory store.
/// No filesystem side effects.
pub(crate) fn native_store_path(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<PathBuf>, ExitCode> {
    marrow_check::native_store_path(Path::new(dir), config)
        .map_err(|error| project_io_exit(dir, error, format))
}

/// Like [`native_store_path`], but creates the data directory so the store can be
/// opened for writing.
pub(crate) fn resolve_store_path(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<PathBuf>, ExitCode> {
    marrow_check::resolve_store_path(Path::new(dir), config)
        .map_err(|error| project_io_exit(dir, error, format))
}

/// Open the project's store read-only, or `Ok(None)` if it holds no saved data on
/// disk yet (explicit memory store, or the native file does not exist). Never creates.
pub(crate) fn open_store_for_inspection(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<marrow_store::tree::TreeStore>, ExitCode> {
    let Some(path) = native_store_path(dir, config, format)? else {
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
    marrow_check::load_config(Path::new(dir)).map_err(|error| project_io_exit(dir, error, format))
}

/// Bind the project's accepted catalog from the live store, failing closed on a corrupt
/// committed lock. The store is the sole accepted authority: when no store snapshot is present
/// this is the first-run `None`, never a lock-derived snapshot, and the read never writes the
/// source tree.
pub(crate) fn read_accepted_store_catalog(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    format: CheckFormat,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ExitCode> {
    let store = open_store_for_inspection(dir, config, format)?;
    marrow_check::read_accepted_catalog_with_store(Path::new(dir), store.as_ref())
        .map_err(|error| project_io_exit(dir, error, format))
}

/// Read the committed lock as a typed value, failing closed on a corrupt lock. `marrow check`
/// uses it to report a stale lock by comparing the lock's source digest against the program's,
/// reading the committed projection without opening or repairing the store.
pub(crate) fn read_committed_lock(
    dir: &str,
    format: CheckFormat,
) -> Result<Option<marrow_catalog::CatalogLock>, ExitCode> {
    marrow_check::read_committed_lock(Path::new(dir))
        .map_err(|error| project_io_exit(dir, error, format))
}

/// What the read-only `check` surface found at the native store path. The distinction matters to
/// the `--locked` gate: a store file that is present carries durable shape that demands a committed
/// lock, whether or not it can be opened. Collapsing present-but-unreadable into absent would let a
/// post-crash, recovery-required store masquerade as a first run and pass an absent lock green.
pub(crate) enum AcceptedAuthority {
    /// A store opened read-only and yielded its accepted catalog snapshot (or had no accepted
    /// catalog yet); the snapshot binds the checked program's frozen shapes.
    Readable(Option<marrow_catalog::CatalogMetadata>),
    /// A store file is present but could not be opened read-only — unclean-shutdown
    /// recovery-required, locked by a writer, or hard-corrupted. `check` neither repairs nor
    /// fails on it, but its presence still counts as durable shape under `--locked`.
    ExistsButUnreadable,
    /// No store file is present: a legitimate first run with no durable shape to lock yet.
    Absent,
}

impl AcceptedAuthority {
    /// The accepted catalog snapshot to bind the checked program against, present only when a
    /// readable store carried one. An unreadable or absent store binds first-run identity from
    /// the committed lock instead.
    pub(crate) fn snapshot(&self) -> Option<&marrow_catalog::CatalogMetadata> {
        match self {
            Self::Readable(snapshot) => snapshot.as_ref(),
            Self::ExistsButUnreadable | Self::Absent => None,
        }
    }

    /// Whether the store carries durable shape a committed lock must project. The `--locked` gate
    /// keys its missing-lock failure on this. A store opened read-only that carried a committed
    /// catalog (`Readable(Some(_))`) and an unopenable store that may carry one
    /// (`ExistsButUnreadable`) both demand a lock, so an absent lock over them is a missing commit.
    /// A uid-only store (`Readable(None)`) is the crash-window remnant between stamping the store
    /// uid and publishing a baseline: it has no committed catalog, so like an absent store there is
    /// nothing to lock and the gate treats it as a first run.
    pub(crate) fn store_present(&self) -> bool {
        matches!(self, Self::Readable(Some(_)) | Self::ExistsButUnreadable)
    }
}

/// Classify the accepted authority at the native store path for the read-only `check` surface.
/// Unlike [`read_accepted_store_catalog`], a store that is absent or unreadable is not an error
/// here: `check` is read-only and must neither create a store nor fail on a hostile or
/// recovery-required store file. It distinguishes a present-but-unreadable store from no store at
/// all so the `--locked` gate can demand a committed lock over a crashed durable store. The read
/// never writes the source tree.
pub(crate) fn read_accepted_store_catalog_lenient(
    dir: &str,
    config: &marrow_project::ProjectConfig,
) -> AcceptedAuthority {
    let Ok(Some(path)) = marrow_check::native_store_path(Path::new(dir), config) else {
        return AcceptedAuthority::Absent;
    };
    if !path.exists() {
        return AcceptedAuthority::Absent;
    }
    let Ok(store) = marrow_store::tree::TreeStore::open_read_only(&path) else {
        return AcceptedAuthority::ExistsButUnreadable;
    };
    match marrow_check::read_accepted_catalog_with_store_read_only(Path::new(dir), Some(&store)) {
        Ok(snapshot) => AcceptedAuthority::Readable(snapshot),
        Err(_) => AcceptedAuthority::ExistsButUnreadable,
    }
}

/// Re-project the committed `marrow.lock` from the store's accepted snapshot. The store is the
/// sole write-time authority for accepted identity; the lock is its committed source-tree
/// projection. An authorized write path (evolve apply, baseline freeze) runs this after the store
/// transaction commits, so the command is not done until the re-projected lock is committed. The
/// projection derives its id ledger from the snapshot's reserved entries, runs through the single
/// lock-write owner, and is idempotent on the bytes a converged store already projected.
pub(crate) fn reproject_committed_lock(
    dir: &str,
    store: &marrow_store::tree::TreeStore,
    program: &marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<(), ExitCode> {
    let Some(snapshot) = store.read_catalog_snapshot().map_err(|error| {
        report_simple_error(error.code(), &error.to_string(), format);
        ExitCode::FAILURE
    })?
    else {
        return Ok(());
    };
    let projection =
        marrow_check::project_store_lock(Path::new(dir), &snapshot, &program.source_digest())
            .map_err(|error| project_io_exit(dir, error, format))?;
    // The committed lock is otherwise invisible on the happy path; announce a create or rewrite
    // once on stderr so the developer learns the file exists and must be committed. The note never
    // touches the JSON envelope or program stdout.
    if matches!(format, CheckFormat::Text) {
        match projection {
            marrow_check::LockProjection::Created => {
                eprintln!("wrote marrow.lock (commit this file alongside your source)");
            }
            marrow_check::LockProjection::Updated => {
                eprintln!("updated marrow.lock to match current source; commit the change");
            }
            marrow_check::LockProjection::Unchanged => {}
        }
    }
    Ok(())
}

/// Outcome of regenerating a project's declared TypeScript client.
pub(crate) enum ClientFreshness {
    /// No `client` path is configured; nothing to do.
    NotConfigured,
    /// `client` is configured but the project declares no surface; a configuration warning.
    SurfacelessConfigured,
    /// The on-disk client already carried the current surface-ABI digest; left untouched.
    AlreadyFresh,
    /// The client was (re)written because it was absent or carried a stale digest.
    Rewritten,
}

/// Classify the project's declared client against the current surface without writing. This is the
/// single owner of the stale-or-absent decision: it resolves the configured path, builds the
/// surface ABI through the render owner, short-circuits on no `client` or no surface, then compares
/// the on-disk header digest against the digest the surface would write. `Rewritten` means the
/// on-disk client is absent or stale (the write path would rewrite it); `AlreadyFresh` means it
/// already carries the current digest. A non-surface `.mw` edit leaves the digest unchanged, so a
/// fresh client stays fresh. Both the write path and the read-only check gate consume this verdict.
pub(crate) fn client_freshness(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    program: &marrow_check::CheckedProgram,
) -> ClientFreshness {
    let Some(rel) = config.client.as_deref() else {
        return ClientFreshness::NotConfigured;
    };
    let abi = marrow_json::surface::SurfaceAbiJson::from_program(program);
    if abi.surfaces.is_empty() {
        return ClientFreshness::SurfacelessConfigured;
    }
    let routes = marrow_json::surface::SurfaceRouteManifestJson::from_abi(&abi);
    let want = marrow_json::surface::surface_abi_digest(&abi, &routes);
    let on_disk = std::fs::read_to_string(Path::new(dir).join(rel))
        .ok()
        .and_then(|contents| marrow_json::surface::surface_client_header_digest(&contents));
    if on_disk.as_deref() == Some(want.as_str()) {
        ClientFreshness::AlreadyFresh
    } else {
        ClientFreshness::Rewritten
    }
}

/// Regenerate the project's declared TypeScript client write-if-changed: classify freshness through
/// the single verdict owner, and on a stale-or-absent verdict render through the render owner and
/// write. A non-surface `.mw` edit leaves the digest unchanged, so the file is not churned. Skips
/// when no `client` is configured; reports a surfaceless configuration when one is set without a
/// surface.
pub(crate) fn write_declared_client_if_changed(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    program: &marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<ClientFreshness, ExitCode> {
    let verdict = client_freshness(dir, config, program);
    if !matches!(verdict, ClientFreshness::Rewritten) {
        return Ok(verdict);
    }
    // A `Rewritten` verdict guarantees a configured path and a non-empty surface, so the ABI and
    // route manifest rebuild cleanly here for the render the write needs.
    let rel = config
        .client
        .as_deref()
        .expect("Rewritten verdict implies a configured client path");
    let abi = marrow_json::surface::SurfaceAbiJson::from_program(program);
    let routes = marrow_json::surface::SurfaceRouteManifestJson::from_abi(&abi);
    let rendered = match marrow_json::surface::render_typescript_client(&abi, &routes) {
        Ok(rendered) => rendered,
        Err(error) => {
            report_simple_error(
                "surface.abi_mismatch",
                &format!("surface client render failed: {error}"),
                format,
            );
            return Err(ExitCode::FAILURE);
        }
    };
    let path = Path::new(dir).join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            report_io_error(&parent.display().to_string(), &error, format);
            ExitCode::FAILURE
        })?;
    }
    std::fs::write(&path, rendered).map_err(|error| {
        report_io_error(&path.display().to_string(), &error, format);
        ExitCode::FAILURE
    })?;
    Ok(ClientFreshness::Rewritten)
}

/// Regenerate the declared client and report a surfaceless configuration as a warning. The three
/// write paths (run, serve startup, evolve apply) share this so the warning prose lives in one
/// place; a render or write failure propagates its exit code.
pub(crate) fn sync_declared_client(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    program: &marrow_check::CheckedProgram,
    format: CheckFormat,
) -> Result<(), ExitCode> {
    if let ClientFreshness::SurfacelessConfigured =
        write_declared_client_if_changed(dir, config, program, format)?
    {
        report_simple_error(
            "config.client_without_surface",
            "`client` is set in marrow.json but the project declares no surface; no client written",
            format,
        );
    }
    Ok(())
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
    // When no store snapshot is bound, the committed lock drives first-run adoption so the evolve
    // path binds the lock's accepted epoch and identity rather than minting fresh ids: a store
    // behind the committed lock then fences as store-behind, and a fresh checkout adopts. A valid
    // store is the sole authority, so the lock is read only when no snapshot is present.
    let lock = if accepted.is_some() {
        None
    } else {
        read_committed_lock(dir, format)?
    };
    let program = marrow_check::check_project_against(
        Path::new(dir),
        &config,
        accepted.as_ref(),
        lock.as_ref(),
    )
    .map_err(|error| project_io_exit(dir, error, format))?;
    Ok((config, program))
}

/// Freeze a project's pending durable identity into the write `store` as its baseline,
/// then re-check the program against the now-accepted store snapshot so the caller binds
/// the frozen identity.
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
    let program = recheck_against_store_catalog(dir, config, store, format)?;
    reproject_committed_lock(dir, store, &program, format)?;
    Ok(program)
}

/// Re-check the project binding durable identity against the store's accepted catalog
/// snapshot.
pub(crate) fn recheck_against_store_catalog(
    dir: &str,
    config: &marrow_project::ProjectConfig,
    store: &marrow_store::tree::TreeStore,
    format: CheckFormat,
) -> Result<marrow_check::CheckedProgram, ExitCode> {
    marrow_check::recheck_against_store_catalog(Path::new(dir), config, store)
        .map_err(|error| project_io_exit(dir, error, format))
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
        eprintln!("unknown format: {value} (expected text, json, or jsonl)");
        return Err(ExitCode::from(2));
    };
    *format = parsed;
    Ok(())
}

pub(crate) fn unknown_option(command: &str, value: &str) -> ExitCode {
    eprintln!("unknown {command} option: {value}; run marrow {command} --help for usage");
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

/// Reject a project target that names an existing non-directory (a bare `.mw`
/// file). A missing path is left alone so the command's loader reports the
/// accurate `io.read` failure, consistent with `run`/`test`/`data`.
pub(crate) fn reject_bare_file_target(command: &str, target: &str) -> Result<(), ExitCode> {
    let path = Path::new(target);
    if path.exists() && !path.is_dir() {
        eprintln!(
            "marrow {command} accepts a project directory containing marrow.json, not a bare file"
        );
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
    serde_json::Value::Object(envelope(
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
    ))
}

pub(crate) fn write_json(value: serde_json::Value) {
    println!("{value}");
}

/// Emit one JSON record on standard error. Tooling reports (the trace and dry-run
/// plan) use this so they never interleave with the program's own stdout output.
pub(crate) fn write_json_err(value: serde_json::Value) {
    eprintln!("{value}");
}

const LOWER_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn push_hex(out: &mut String, bytes: &[u8]) {
    for &byte in bytes {
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
}

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

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::{project_json_path, run_worker_thread};

    #[test]
    fn project_json_path_keeps_missing_absolute_path() {
        let missing = std::env::temp_dir()
            .join(format!(
                "marrow-project-json-path-missing-{}",
                std::process::id()
            ))
            .join("project")
            .display()
            .to_string();

        assert_eq!(project_json_path(&missing), missing);
    }

    #[test]
    fn project_json_path_keeps_empty_path_when_absolute_path_fails() {
        assert_eq!(project_json_path(""), "");
    }

    #[test]
    fn worker_thread_spawn_error_returns_failure() {
        let result = run_worker_thread(Err(std::io::ErrorKind::WouldBlock.into()));

        assert_eq!(result, ExitCode::FAILURE);
    }

    #[test]
    fn worker_thread_returns_worker_exit_code() {
        let result = run_worker_thread(Ok(std::thread::spawn(|| ExitCode::from(7))));

        assert_eq!(result, ExitCode::from(7));
    }
}
