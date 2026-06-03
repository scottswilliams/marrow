use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_syntax::Diagnose;
use serde_json::json;

mod cmd_check;
mod cmd_data;
mod cmd_explain;
mod cmd_fmt;
mod cmd_run;
mod cmd_test;
mod dry_run;
mod lsp;
mod serve;
mod trace;

const HELP: &str = "\
Marrow

Usage:
  marrow check [--format text|json|jsonl] <file.mw>
  marrow fmt [--check | --write] <file.mw | projectdir>
  marrow run <projectdir>
  marrow test <projectdir>
  marrow data <roots|stats|dump|integrity> <projectdir>
  marrow data get <projectdir> <path>
  marrow explain [--format text|json|jsonl] <projectdir> <target>
  marrow lsp
  marrow serve [--port <port>] <projectdir>
  marrow --version
  marrow --help
";

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some((command, rest)) = args.split_first() else {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    };
    match command.as_str() {
        "check" => cmd_check::check(rest),
        "fmt" => cmd_fmt::fmt(rest),
        "run" => cmd_run::run(rest),
        "test" => cmd_test::test(rest),
        "data" => cmd_data::data(rest),
        "explain" => cmd_explain::explain(rest),
        "lsp" => lsp::run(rest),
        "serve" => serve::run(rest),
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
        CheckFormat::Json => {
            let diagnostics = report
                .diagnostics
                .iter()
                .map(check_diagnostic_record)
                .collect::<Vec<_>>();
            write_json(json!({
                "project": target,
                "status": status,
                "diagnostics": diagnostics,
            }));
        }
        CheckFormat::Jsonl => {
            for diagnostic in &report.diagnostics {
                write_json(check_diagnostic_record(diagnostic));
            }
            write_json(json!({
                "kind": "summary",
                "project": target,
                "status": status,
                "diagnostics": report.diagnostics.len(),
            }));
        }
    }
}

/// The error envelope shared by every diagnostic the CLI emits as JSON: the
/// `code`/`kind`/`message` scaffold every code carries, plus its per-source
/// `source_span`. Parse and project diagnostics add a `severity`; a parse
/// diagnostic also carries `help` (and byte offsets in its span). The optional
/// keys are passed in rather than read off the trait so the absent-key cases stay
/// exactly absent.
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

/// Render a project diagnostic as JSON. Unlike single-file parse diagnostics,
/// project diagnostics carry no `help` or byte offsets — they are reported at a
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

/// The project store's redb file path (native backend), or `Ok(None)` for the
/// in-memory default. No filesystem side effects. The `Result` mirrors
/// [`resolve_store_path`], which can fail while creating the directory.
fn native_store_path(
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
            // `parse_config` already rejected a native store with a missing or
            // empty `dataDir` as `config.invalid`, so it is present here.
            let data_dir = data_dir
                .as_deref()
                .expect("parse_config guarantees a native store has a dataDir");
            Ok(Some(Path::new(dir).join(data_dir).join("marrow.redb")))
        }
    }
}

/// Like [`native_store_path`], but creates the data directory so the store can be
/// opened for writing. `Ok(None)` is the in-memory default.
pub(crate) fn resolve_store_path(
    dir: &str,
    config: &marrow_project::ProjectConfig,
) -> Result<Option<PathBuf>, ExitCode> {
    let Some(path) = native_store_path(dir, config)? else {
        return Ok(None);
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            report_io_error(&parent.display().to_string(), &error, CheckFormat::Text);
            ExitCode::FAILURE
        })?;
    }
    Ok(Some(path))
}

/// Open the project's configured store read-only for inspection, or `Ok(None)` if
/// it holds no saved data on disk yet (the in-memory default, or the native file
/// does not exist). Never creates a store — inspection is read-only.
pub(crate) fn open_store_for_inspection(
    dir: &str,
    config: &marrow_project::ProjectConfig,
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
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            Err(ExitCode::FAILURE)
        }
    }
}

/// Load `<dir>/marrow.json`. Reports and returns the exit code if it is missing or
/// invalid. `load_checked_project` builds on this for commands that need checked
/// source facts.
pub(crate) fn load_config(dir: &str) -> Result<marrow_project::ProjectConfig, ExitCode> {
    let config_path = Path::new(dir).join("marrow.json");
    let config_text = std::fs::read_to_string(&config_path).map_err(|error| {
        report_io_error(
            &config_path.display().to_string(),
            &error,
            CheckFormat::Text,
        );
        ExitCode::FAILURE
    })?;
    marrow_project::parse_config(&config_text).map_err(|error| {
        report_simple_error(error.code, &error.message, CheckFormat::Text);
        ExitCode::FAILURE
    })
}

/// Load `<dir>/marrow.json` and check the project. On any failure — a missing or
/// invalid config, an unreadable source root, or check errors — the problem is
/// reported and the exit code is returned in `Err`; on success the parsed config
/// and checked program are returned. Shared by `run` and `test`.
pub(crate) fn load_checked_project(
    dir: &str,
) -> Result<(marrow_project::ProjectConfig, marrow_check::CheckedProgram), ExitCode> {
    let config = load_config(dir)?;
    let (report, program) =
        marrow_check::check_project(Path::new(dir), &config).map_err(|error| {
            report_simple_error(
                error.code,
                &format!("{}: {}", error.path.display(), error.message),
                CheckFormat::Text,
            );
            ExitCode::FAILURE
        })?;
    if report.has_errors() {
        report_project(dir, &report, CheckFormat::Text);
        return Err(ExitCode::FAILURE);
    }
    Ok((config, program))
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
        CheckFormat::Json => {
            let diagnostics = parsed
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic_record(file, diagnostic))
                .collect::<Vec<_>>();
            write_json(json!({
                "file": file,
                "status": if parsed.has_errors() { "failed" } else { "ok" },
                "diagnostics": diagnostics,
                "declarations": parsed.file.declarations.len(),
            }));
        }
        CheckFormat::Jsonl => {
            for diagnostic in &parsed.diagnostics {
                write_json(diagnostic_record(file, diagnostic));
            }
            write_json(json!({
                "kind": "summary",
                "file": file,
                "status": if parsed.has_errors() { "failed" } else { "ok" },
                "diagnostics": parsed.diagnostics.len(),
                "declarations": parsed.file.declarations.len(),
            }));
        }
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
