use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;

mod lsp;
mod serve;

const HELP: &str = "\
Marrow

Usage:
  marrow check [--format text|json|jsonl] <file.mw>
  marrow fmt [--check | --write] <file.mw>
  marrow run <projectdir>
  marrow test <projectdir>
  marrow backup <projectdir> <archive>
  marrow restore <projectdir> <archive>
  marrow data <roots|stats> <projectdir>
  marrow lsp
  marrow serve [--port <port>] <projectdir>
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
    if args.first().is_some_and(|arg| arg == "test") {
        return test(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "backup") {
        return backup(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "restore") {
        return restore(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "data") {
        return data(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "lsp") {
        return lsp::run(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "serve") {
        return serve::run(&args[1..]);
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

/// The broad envelope category for a dotted error code (docs/error-codes.md).
/// Derived from the code's first segment so every machine-readable surface emits
/// a consistent `kind` field. The code prefix is not always the kind name
/// (`run.*` is `runtime`, `store.*` is `storage`), so the mapping is explicit.
fn kind_for_code(code: &str) -> &'static str {
    match code.split('.').next().unwrap_or("") {
        "parse" => "parse",
        "check" | "schema" => "check",
        "run" => "runtime",
        "store" => "storage",
        "io" => "io",
        "protocol" => "protocol",
        // Configuration and project-discovery failures are tooling errors.
        _ => "tooling",
    }
}

/// Render a project diagnostic as JSON. Project diagnostics carry a code, kind,
/// severity, message, and file position; unlike single-file parse diagnostics
/// they have no `help` or byte offsets, since module-path and duplicate-module
/// problems are reported at a declaration site.
fn check_diagnostic_record(diagnostic: &marrow_check::CheckDiagnostic) -> serde_json::Value {
    json!({
        "code": diagnostic.code,
        "kind": kind_for_code(&diagnostic.code),
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
            "kind": kind_for_code(code),
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
    let (config, program) = match load_checked_project(dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };

    let Some(entry) = entry_override.or(config.default_entry.as_deref()) else {
        report_simple_error(
            "run.no_entry",
            "no entry to run; pass --entry <name> or set `run.defaultEntry` in marrow.json",
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    };

    match resolve_store_path(dir, &config) {
        Err(code) => code,
        Ok(None) => {
            let store = RefCell::new(marrow_store::mem::MemStore::new());
            execute(&program, &store, entry)
        }
        Ok(Some(path)) => match marrow_store::redb::RedbStore::open(&path) {
            Ok(store) => execute(&program, &RefCell::new(store), entry),
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                ExitCode::FAILURE
            }
        },
    }
}

/// The project store's redb file path (native backend), or `Ok(None)` for the
/// in-memory default. Pure — no filesystem side effects; reports and returns the
/// exit code only when a native store omits its `dataDir`.
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
            let Some(data_dir) = data_dir.as_deref() else {
                report_simple_error(
                    marrow_project::CONFIG_INVALID,
                    "native store requires `store.dataDir`",
                    CheckFormat::Text,
                );
                return Err(ExitCode::FAILURE);
            };
            Ok(Some(Path::new(dir).join(data_dir).join("marrow.redb")))
        }
    }
}

/// Like [`native_store_path`], but creates the data directory so the store can be
/// opened for writing. `Ok(None)` is the in-memory default.
fn resolve_store_path(
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

/// Open the project's configured store for exclusive access (used by backup and
/// restore, which own the store rather than sharing it with a run). Reports and
/// returns the exit code on failure.
fn open_owned_store(
    dir: &str,
    config: &marrow_project::ProjectConfig,
) -> Result<Box<dyn marrow_store::backend::Backend>, ExitCode> {
    match resolve_store_path(dir, config)? {
        None => Ok(Box::new(marrow_store::mem::MemStore::new())),
        Some(path) => match marrow_store::redb::RedbStore::open(&path) {
            Ok(store) => Ok(Box::new(store)),
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                Err(ExitCode::FAILURE)
            }
        },
    }
}

/// Open the project's configured store read-only for inspection, or `Ok(None)` if
/// it holds no saved data on disk yet (the in-memory default, or the native file
/// does not exist). Never creates a store — inspection is read-only.
pub(crate) fn open_store_for_inspection(
    dir: &str,
    config: &marrow_project::ProjectConfig,
) -> Result<Option<Box<dyn marrow_store::backend::Backend>>, ExitCode> {
    let Some(path) = native_store_path(dir, config)? else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    match marrow_store::redb::RedbStore::open_read_only(&path) {
        Ok(store) => Ok(Some(Box::new(store))),
        Err(error) => {
            report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
            Err(ExitCode::FAILURE)
        }
    }
}

/// Run `entry` from a checked `program` over `store`, printing its output. The
/// store is the ordered-tree backend the project selected; the run reads the
/// real system clock for `std::clock::now()`, the real environment for
/// `std::env`, the real filesystem for `std::io`, and writes `std::log` output to
/// standard error.
fn execute(
    program: &marrow_check::CheckedProgram,
    store: &RefCell<dyn marrow_store::backend::Backend>,
    entry: &str,
) -> ExitCode {
    let log = std::rc::Rc::new(RefCell::new(String::new()));
    let host = marrow_run::Host::new()
        .with_system_clock()
        .with_system_environment()
        .with_log_sink(std::rc::Rc::clone(&log))
        .with_filesystem();
    let result = marrow_run::run_entry_with_host(program, store, &host, entry, &[]);
    // Flush any log output (collected even on a failing run) to standard error,
    // keeping it off the program's own stdout stream.
    eprint!("{}", log.borrow());
    match result {
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

/// Load `<dir>/marrow.json`. Reports and returns the exit code if it is missing or
/// invalid. `load_checked_project` builds on this; backup and restore use it
/// directly, since raw saved data needs no source checking.
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
fn load_checked_project(
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

/// Run a project's tests: `marrow test <projectdir>`.
fn test(args: &[String]) -> ExitCode {
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow test <projectdir>

Check a Marrow project, then run its tests: every `pub fn` with no parameters in
a test file (the `tests` patterns in marrow.json). Each test runs against a fresh
in-memory store; a `std::assert::*` failure is a located test failure.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => {
                eprintln!("unknown test option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if dir.replace(value.to_string()).is_some() {
                    eprintln!("marrow test accepts one project directory");
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
    test_project_dir(&dir)
}

/// Check `<dir>`'s project and its test files, then run each test over a fresh
/// in-memory store. Reports each result and a summary; exits non-zero if any test
/// fails or errors, if the project does not check, or if no tests are found.
fn test_project_dir(dir: &str) -> ExitCode {
    let (config, src_program) = match load_checked_project(dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };

    let (test_report, test_modules) =
        match marrow_check::check_tests(Path::new(dir), &config, &src_program) {
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
    if test_report.has_errors() {
        report_project(dir, &test_report, CheckFormat::Text);
        return ExitCode::FAILURE;
    }

    // A test is a public, zero-parameter function in a test file. Keep each test's
    // source file so a failure can be reported at its location.
    let tests: Vec<(String, PathBuf)> = test_modules
        .iter()
        .flat_map(|module| {
            module
                .functions
                .iter()
                .filter(|function| function.public && function.params.is_empty())
                .map(|function| {
                    (
                        format!("{}::{}", module.name, function.name),
                        module.source_file.clone(),
                    )
                })
        })
        .collect();
    if tests.is_empty() {
        report_simple_error(
            "test.none",
            "no tests found; check the `tests` patterns in marrow.json",
            CheckFormat::Text,
        );
        return ExitCode::FAILURE;
    }

    // The runner resolves test names against the project plus the test modules.
    let mut program = src_program;
    program.modules.extend(test_modules);

    // Tests get the same host capabilities as a run; their `std::log` output goes
    // to a discard sink so it stays out of the pass/fail report.
    let host = marrow_run::Host::new()
        .with_system_clock()
        .with_system_environment()
        .with_log_sink(std::rc::Rc::new(RefCell::new(String::new())))
        .with_filesystem();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut errored = 0usize;
    for (name, source_file) in &tests {
        let store = RefCell::new(marrow_store::mem::MemStore::new());
        match marrow_run::run_entry_with_host(&program, &store, &host, name, &[]) {
            Ok(_) => {
                println!("ok    {name}");
                passed += 1;
            }
            Err(error) if error.code == marrow_run::RUN_ASSERT => {
                println!("FAIL  {name}");
                println!(
                    "      {}:{}:{}: {}: {}",
                    source_file.display(),
                    error.span.line,
                    error.span.column,
                    error.code,
                    error.message
                );
                failed += 1;
            }
            Err(error) => {
                println!("ERROR {name}");
                println!(
                    "      {}:{}:{}: {}: {}",
                    source_file.display(),
                    error.span.line,
                    error.span.column,
                    error.code,
                    error.message
                );
                errored += 1;
            }
        }
    }
    println!(
        "\n{} test{}: {passed} passed, {failed} failed, {errored} errored",
        tests.len(),
        if tests.len() == 1 { "" } else { "s" }
    );
    if failed == 0 && errored == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

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
fn backup(args: &[String]) -> ExitCode {
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
fn restore(args: &[String]) -> ExitCode {
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
    // A normal restore writes into an empty target; replace/merge/repair are
    // explicit maintenance actions, deferred for now.
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

/// Parse exactly one positional path for a command, handling `--help` and
/// rejecting options or a wrong count.
fn one_positional(command: &str, args: &[String]) -> Result<String, ExitCode> {
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                print!("Usage:\n  marrow {command} <projectdir>\n");
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
    dir.ok_or_else(|| {
        eprintln!("missing project directory");
        ExitCode::from(2)
    })
}

/// Inspect a project's saved data, read-only:
/// `marrow data <roots|stats> <projectdir>`.
fn data(args: &[String]) -> ExitCode {
    let Some((subcommand, rest)) = args.split_first() else {
        eprintln!("missing data subcommand; expected `roots` or `stats`");
        eprintln!("run `marrow data --help` for usage");
        return ExitCode::from(2);
    };
    match subcommand.as_str() {
        "--help" | "-h" => {
            print!(
                "\
Usage:
  marrow data roots <projectdir>   list the saved roots
  marrow data stats <projectdir>   count saved roots and records

Read-only inspection of a project's saved data; it never creates or modifies the
store.
"
            );
            ExitCode::SUCCESS
        }
        "roots" => data_roots(rest),
        "stats" => data_stats(rest),
        other => {
            eprintln!("unknown data subcommand: {other}");
            eprintln!("expected `roots` or `stats`");
            ExitCode::from(2)
        }
    }
}

/// `marrow data roots`: list the project's saved roots, one `^root` per line.
fn data_roots(args: &[String]) -> ExitCode {
    let dir = match one_positional("data roots", args) {
        Ok(dir) => dir,
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
                report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                return ExitCode::FAILURE;
            }
        },
        None => Vec::new(),
    };
    if roots.is_empty() {
        println!("(no saved data)");
    } else {
        for root in roots {
            println!("^{root}");
        }
    }
    ExitCode::SUCCESS
}

/// `marrow data stats`: report how many saved roots and records the store holds.
fn data_stats(args: &[String]) -> ExitCode {
    let dir = match one_positional("data stats", args) {
        Ok(dir) => dir,
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
                    report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                    return ExitCode::FAILURE;
                }
            };
            // A full scan to count is fine for a local store; a bounded count
            // primitive on the backend would replace this if stores grow large.
            let records = match store.scan(&[], usize::MAX) {
                Ok(page) => page.entries.len(),
                Err(error) => {
                    report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                    return ExitCode::FAILURE;
                }
            };
            (roots, records)
        }
        None => (0, 0),
    };
    println!("roots: {roots}");
    println!("records: {records}");
    ExitCode::SUCCESS
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
