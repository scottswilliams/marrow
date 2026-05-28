use std::cell::RefCell;
use std::path::Path;
use std::process::ExitCode;

use serde_json::json;

const HELP: &str = "\
Marrow

Usage:
  marrow check [--format text|json|jsonl] <file.mw>
  marrow fmt [--check | --write] <file.mw>
  marrow run <projectdir>
  marrow --version
  marrow --help

Marrow is starting from the reference docs. Language commands will land as the
native .mw source model, checker, runtime, and storage kernel grow.
";

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "check") {
        return check(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "fmt") {
        return fmt(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "run") {
        return run(&args[1..]);
    }
    let mut args = args.into_iter();
    match args.next().as_deref() {
        None | Some("--help" | "-h" | "help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("--version" | "-V" | "version") => {
            println!("marrow {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unknown command: {command}");
            eprintln!("run `marrow --help` for available commands");
            ExitCode::from(2)
        }
    }
}

fn check(args: &[String]) -> ExitCode {
    let mut format = CheckFormat::Text;
    let mut file = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --format");
                    return ExitCode::from(2);
                };
                let Some(parsed) = CheckFormat::parse(value) else {
                    eprintln!("unknown check format: {value}");
                    return ExitCode::from(2);
                };
                format = parsed;
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow check [--format text|json|jsonl] <file.mw | projectdir>

Parse a Marrow source file, or check a whole project directory (one that
contains marrow.json), and report diagnostics.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                eprintln!("unknown check option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if file.replace(value.to_string()).is_some() {
                    eprintln!("marrow check accepts one source file or project directory");
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }

    let Some(file) = file else {
        eprintln!("missing source file or project directory");
        return ExitCode::from(2);
    };
    if Path::new(&file).is_dir() {
        return check_project_dir(&file, format);
    }
    let source = match std::fs::read_to_string(&file) {
        Ok(source) => source,
        Err(error) => {
            report_io_error(&file, &error, format);
            return ExitCode::FAILURE;
        }
    };
    let parsed = marrow_syntax::parse_source(&source);
    report_check(&file, &parsed, format);
    if parsed.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Check a whole project: load `<dir>/marrow.json`, then run the project
/// checker over its source roots.
fn check_project_dir(dir: &str, format: CheckFormat) -> ExitCode {
    let config_path = Path::new(dir).join("marrow.json");
    let config_text = match std::fs::read_to_string(&config_path) {
        Ok(text) => text,
        Err(error) => {
            report_io_error(&config_path.display().to_string(), &error, format);
            return ExitCode::FAILURE;
        }
    };
    let config = match marrow_project::parse_config(&config_text) {
        Ok(config) => config,
        Err(error) => {
            report_simple_error(error.code, &error.message, format);
            return ExitCode::FAILURE;
        }
    };
    let (report, _program) = match marrow_check::check_project(Path::new(dir), &config) {
        Ok(result) => result,
        Err(error) => {
            report_simple_error(
                error.code,
                &format!("{}: {}", error.path.display(), error.message),
                format,
            );
            return ExitCode::FAILURE;
        }
    };

    report_project(dir, &report, format);
    if report.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn report_project(target: &str, report: &marrow_check::CheckReport, format: CheckFormat) {
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
                        diagnostic.line,
                        diagnostic.column,
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

/// Render a project diagnostic as JSON. Project diagnostics carry only a code,
/// severity, message, and file position; unlike single-file parse diagnostics
/// they have no `kind`/`help` or byte offsets, since module-path and
/// duplicate-module problems are reported at a declaration site.
fn check_diagnostic_record(diagnostic: &marrow_check::CheckDiagnostic) -> serde_json::Value {
    json!({
        "code": diagnostic.code,
        "severity": diagnostic.severity.as_str(),
        "message": diagnostic.message,
        "source_span": {
            "file": diagnostic.file.display().to_string(),
            "line": diagnostic.line,
            "column": diagnostic.column,
        },
    })
}

fn report_simple_error(code: &str, message: &str, format: CheckFormat) {
    match format {
        CheckFormat::Text => eprintln!("{code}: {message}"),
        CheckFormat::Json | CheckFormat::Jsonl => write_json(json!({
            "code": code,
            "message": message,
            "source_span": null,
        })),
    }
}

/// Run a project's entry function. Unlike `check`, `run` has no `--format`: its
/// output is the program's own `print`/`write` stream, which has no JSON envelope;
/// failures still report a dotted error code on stderr.
fn run(args: &[String]) -> ExitCode {
    let mut entry = None;
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--entry" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("missing value for --entry");
                    return ExitCode::from(2);
                };
                entry = Some(value.clone());
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow run [--entry <module::function>] <projectdir>

Check a Marrow project, then run an entry function over the store its
`marrow.json` selects (an in-memory store when none is configured). The entry
is `--entry` if given, else the project's `run.defaultEntry`. Output written
with `print`/`write` goes to stdout.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                eprintln!("unknown run option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if dir.replace(value.to_string()).is_some() {
                    eprintln!("marrow run accepts one project directory");
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }

    let Some(dir) = dir else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };
    run_project_dir(&dir, entry.as_deref())
}

/// Load and check `<dir>/marrow.json`'s project, then run its entry (the
/// `--entry` override, else `run.defaultEntry`) over the configured store. A
/// project must check cleanly before it runs.
fn run_project_dir(dir: &str, entry_override: Option<&str>) -> ExitCode {
    let config_path = Path::new(dir).join("marrow.json");
    let config_text = match std::fs::read_to_string(&config_path) {
        Ok(text) => text,
        Err(error) => {
            report_io_error(
                &config_path.display().to_string(),
                &error,
                CheckFormat::Text,
            );
            return ExitCode::FAILURE;
        }
    };
    let config = match marrow_project::parse_config(&config_text) {
        Ok(config) => config,
        Err(error) => {
            report_simple_error(error.code, &error.message, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    let (report, program) = match marrow_check::check_project(Path::new(dir), &config) {
        Ok(result) => result,
        Err(error) => {
            report_simple_error(
                error.code,
                &format!("{}: {}", error.path.display(), error.message),
                CheckFormat::Text,
            );
            return ExitCode::FAILURE;
        }
    };
    if report.has_errors() {
        report_project(dir, &report, CheckFormat::Text);
        return ExitCode::FAILURE;
    }

    let Some(entry) = entry_override.or(config.default_entry.as_deref()) else {
        report_simple_error(
            "run.no_entry",
            "no entry to run; pass --entry <name> or set `run.defaultEntry` in marrow.json",
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    };

    match &config.store {
        None
        | Some(marrow_project::StoreConfig {
            backend: marrow_project::StoreBackend::Memory,
            ..
        }) => {
            let store = RefCell::new(marrow_store::mem::MemStore::new());
            execute(&program, &store, entry)
        }
        Some(marrow_project::StoreConfig {
            backend: marrow_project::StoreBackend::Native,
            data_dir,
        }) => {
            let Some(data_dir) = data_dir.as_deref() else {
                report_simple_error(
                    marrow_project::CONFIG_INVALID,
                    "native store requires `store.dataDir`",
                    CheckFormat::Text,
                );
                return ExitCode::FAILURE;
            };
            let data_path = Path::new(dir).join(data_dir);
            if let Err(error) = std::fs::create_dir_all(&data_path) {
                report_io_error(&data_path.display().to_string(), &error, CheckFormat::Text);
                return ExitCode::FAILURE;
            }
            let store = match marrow_store::redb::RedbStore::open(&data_path.join("marrow.redb")) {
                Ok(store) => RefCell::new(store),
                Err(error) => {
                    report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                    return ExitCode::FAILURE;
                }
            };
            execute(&program, &store, entry)
        }
    }
}

/// Run `entry` from a checked `program` over `store`, printing its output. The
/// store is the ordered-tree backend the project selected; the run reads the
/// real system clock for `std::clock::now()`.
fn execute(
    program: &marrow_check::CheckedProgram,
    store: &RefCell<dyn marrow_store::backend::Backend>,
    entry: &str,
) -> ExitCode {
    let host = marrow_run::Host::with_system_clock();
    match marrow_run::run_entry_with_host(program, store, &host, entry, &[]) {
        Ok(outcome) => {
            print!("{}", outcome.output);
            ExitCode::SUCCESS
        }
        Err(error) => {
            report_simple_error(error.code, &error.message, CheckFormat::Text);
            ExitCode::FAILURE
        }
    }
}

fn fmt(args: &[String]) -> ExitCode {
    let mut mode = FmtMode::Print;
    let mut file = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--check" => mode = FmtMode::Check,
            "--write" => mode = FmtMode::Write,
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow fmt [--check | --write] <file.mw>

Format a Marrow source file. With no flag, print the formatted source to
stdout. --check exits non-zero if the file is not already formatted. --write
rewrites the file in place.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                eprintln!("unknown fmt option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if file.replace(value.to_string()).is_some() {
                    eprintln!("marrow fmt accepts one source file");
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }

    let Some(file) = file else {
        eprintln!("missing source file");
        return ExitCode::from(2);
    };
    let source = match std::fs::read_to_string(&file) {
        Ok(source) => source,
        Err(error) => {
            report_io_error(&file, &error, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };

    // Do not reformat source that does not parse; report its diagnostics and
    // leave the file untouched.
    let parsed = marrow_syntax::parse_source(&source);
    if parsed.has_errors() {
        report_check(&file, &parsed, CheckFormat::Text);
        return ExitCode::FAILURE;
    }

    let formatted = marrow_syntax::format_source(&source);
    match mode {
        FmtMode::Print => {
            print!("{formatted}");
            ExitCode::SUCCESS
        }
        FmtMode::Check => {
            if source == formatted {
                ExitCode::SUCCESS
            } else {
                eprintln!("{file}: not formatted");
                ExitCode::FAILURE
            }
        }
        FmtMode::Write => {
            if source != formatted
                && let Err(error) = std::fs::write(&file, &formatted)
            {
                report_io_error(&file, &error, CheckFormat::Text);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
    }
}

#[derive(Clone, Copy)]
enum FmtMode {
    Print,
    Check,
    Write,
}

#[derive(Clone, Copy)]
enum CheckFormat {
    Text,
    Json,
    Jsonl,
}

impl CheckFormat {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            "jsonl" => Some(Self::Jsonl),
            _ => None,
        }
    }
}

fn report_check(file: &str, parsed: &marrow_syntax::ParsedSource, format: CheckFormat) {
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

fn report_io_error(file: &str, error: &std::io::Error, format: CheckFormat) {
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
    json!({
        "code": diagnostic.code,
        "kind": diagnostic.kind,
        "severity": diagnostic.severity.as_str(),
        "message": diagnostic.message,
        "help": diagnostic.help,
        "source_span": {
            "file": file,
            "line": diagnostic.span.line,
            "column": diagnostic.span.column,
            "start_byte": diagnostic.span.start_byte,
            "end_byte": diagnostic.span.end_byte,
        },
    })
}

fn write_json(value: serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string(&value).expect("JSON value should serialize")
    );
}
