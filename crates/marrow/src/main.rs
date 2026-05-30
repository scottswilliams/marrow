use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marrow_store::path::display_path;
use marrow_syntax::Diagnose;
use serde_json::json;

mod lsp;
mod serve;

const HELP: &str = "\
Marrow

Usage:
  marrow check [--format text|json|jsonl] <file.mw>
  marrow fmt [--check | --write] <file.mw | projectdir>
  marrow run <projectdir>
  marrow test <projectdir>
  marrow backup <projectdir> <archive>
  marrow restore <projectdir> <archive>
  marrow data <roots|stats|dump|integrity> <projectdir>
  marrow data get <projectdir> <path>
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
        "check" => check(rest),
        "fmt" => fmt(rest),
        "run" => run(rest),
        "test" => test(rest),
        "backup" => backup(rest),
        "restore" => restore(rest),
        "data" => data(rest),
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
    check_single_file(&file, &source, format)
}

/// Check one `.mw` file with the full type checker, not just the parser.
///
/// Parse errors are reported as before, with their help text and byte offsets.
/// A file that parses cleanly is then run through the same project checker the
/// project-directory path and `run` use, by synthesizing a throwaway one-file
/// project, so `check.*` type diagnostics (assignment, return, and operator type
/// errors) surface for a single file instead of being silently skipped.
fn check_single_file(file: &str, source: &str, format: CheckFormat) -> ExitCode {
    let parsed = marrow_syntax::parse_source(source);
    // Source that does not parse cannot be type-checked; report its parse
    // diagnostics (which carry help and byte offsets the project checker drops).
    if parsed.has_errors() {
        report_check(file, &parsed, format);
        return ExitCode::FAILURE;
    }
    match check_one_file_project(file, source, parsed.file.module.as_ref()) {
        Ok(report) => {
            report_single_file_check(file, &parsed, &report, format);
            if report.has_errors() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        // A synthesized project should always check; if its scratch directory
        // could not be written, fall back to the clean parse result rather than
        // failing a file that parsed.
        Err(error) => {
            report_io_error(&error.0.display().to_string(), &error.1, format);
            report_check(file, &parsed, format);
            ExitCode::FAILURE
        }
    }
}

/// Type-check a single parsed-clean file by synthesizing a throwaway one-file
/// project in a scratch directory and running the project checker over it, so a
/// lone file reaches the same `check.*` rules a project directory does. The file
/// is placed at the path its declared `module` implies (a module-less script
/// keeps its own stem), so the project checker raises no spurious module-path
/// diagnostic. Returns the report with each diagnostic relocated to `file`.
fn check_one_file_project(
    file: &str,
    source: &str,
    module: Option<&marrow_syntax::ModuleDecl>,
) -> Result<marrow_check::CheckReport, (PathBuf, std::io::Error)> {
    let scratch = ScratchDir::new().map_err(|error| (std::env::temp_dir(), error))?;
    let root = scratch.path();
    // A library file (`module a::b`) must sit at `a/b.mw`; a module-less script
    // is path-free, so any `.mw` name under the root works.
    let relative = module_relative_path(module);
    let target = root.join(&relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| (parent.to_path_buf(), error))?;
    }
    std::fs::write(&target, source).map_err(|error| (target.clone(), error))?;

    let config = marrow_project::ProjectConfig {
        source_roots: vec![".".to_string()],
        default_entry: None,
        store: None,
        tests: Vec::new(),
    };
    // The scratch root holds exactly one file, so discovery cannot walk a source
    // root it failed to create; an error here would be a genuine I/O fault.
    let (mut report, _program) = marrow_check::check_project(root, &config)
        .map_err(|error| (error.path.clone(), std::io::Error::other(error.message)))?;
    // Every diagnostic came from the one synthesized file; relocate them to the
    // path the operator named so the report reads as a single-file check.
    let real = PathBuf::from(file);
    for diagnostic in &mut report.diagnostics {
        diagnostic.file = real.clone();
    }
    Ok(report)
}

/// The source-root-relative path a single file is written to inside its
/// synthesized project: `a/b.mw` for a `module a::b`, else `script.mw` for a
/// module-less file.
fn module_relative_path(module: Option<&marrow_syntax::ModuleDecl>) -> PathBuf {
    match module {
        Some(module) => {
            let mut path = PathBuf::new();
            for segment in module.name.split("::") {
                path.push(segment);
            }
            path.set_extension("mw");
            path
        }
        None => PathBuf::from("script.mw"),
    }
}

/// Report a single-file check: a clean check prints the friendly `ok` summary
/// (reusing the parse reporter for its declaration count), while any diagnostics
/// print through the located check reporter the project path uses.
fn report_single_file_check(
    file: &str,
    parsed: &marrow_syntax::ParsedSource,
    report: &marrow_check::CheckReport,
    format: CheckFormat,
) {
    if report.diagnostics.is_empty() {
        report_check(file, parsed, format);
    } else {
        report_project(file, report, format);
    }
}

/// A scratch directory under the system temp dir, removed on drop. Used to hand
/// the project checker a real on-disk one-file project without leaving litter.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new() -> std::io::Result<Self> {
        // A pid + nanosecond stamp keeps concurrent `marrow check` runs from
        // colliding; the directory is this process's alone.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("marrow-check-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best effort: a failed cleanup leaves a temp directory but is not worth
        // failing a check over.
        let _ = std::fs::remove_dir_all(&self.path);
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
fn envelope(
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

fn report_simple_error(code: &str, message: &str, format: CheckFormat) {
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

/// Run a project's entry function. Unlike `check`, `run` has no `--format`: its
/// output is the program's own `print`/`write` stream, which has no JSON envelope;
/// failures still report a dotted error code on stderr.
fn run(args: &[String]) -> ExitCode {
    let mut entry = None;
    let mut dir = None;
    let mut maintenance = false;
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
            // Grants the maintenance capability (whole-root delete, required-field
            // delete, raw quoted-segment access). An operator must type it; the
            // default run and `run.defaultEntry` can never inject it.
            "--maintenance" => maintenance = true,
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow run [--entry <module::function>] [--maintenance] <projectdir>

Check a Marrow project, then run an entry function over the store its
`marrow.json` selects (an in-memory store when none is configured). The entry
is `--entry` if given, else the project's `run.defaultEntry`. Output written
with `print`/`write` goes to stdout.

  --maintenance  Run with the maintenance capability, for migration, repair,
                 and restore tooling. It permits whole managed-root deletes,
                 required-field deletes, and raw quoted-segment access that the
                 default run rejects. Use it deliberately.
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
    run_project_dir(&dir, entry.as_deref(), maintenance)
}

/// Load and check `<dir>/marrow.json`'s project, then run its entry (the
/// `--entry` override, else `run.defaultEntry`) over the configured store. A
/// project must check cleanly before it runs.
fn run_project_dir(dir: &str, entry_override: Option<&str>, maintenance: bool) -> ExitCode {
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
            execute(&program, &store, entry, maintenance)
        }
        Ok(Some(path)) => match marrow_store::redb::RedbStore::open(&path) {
            Ok(store) => execute(&program, &RefCell::new(store), entry, maintenance),
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), CheckFormat::Text);
                ExitCode::FAILURE
            }
        },
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

/// Run `entry` from a checked `program` over `store`, printing its output. The run
/// gets the real system clock, environment, and filesystem, and sends `std::log`
/// output to standard error. `maintenance` grants the maintenance capability only
/// when the operator passed `--maintenance`.
fn execute(
    program: &marrow_check::CheckedProgram,
    store: &RefCell<dyn marrow_store::backend::Backend>,
    entry: &str,
    maintenance: bool,
) -> ExitCode {
    let log = std::rc::Rc::new(RefCell::new(String::new()));
    let mut host = marrow_run::Host::new()
        .with_system_clock()
        .with_system_environment()
        .with_log_sink(std::rc::Rc::clone(&log))
        .with_filesystem();
    if maintenance {
        host = host.with_maintenance();
    }
    let result = marrow_run::run_entry_with_host(program, store, &host, entry, &[]);
    // Log output is collected even on a failing run; it goes to stderr, off the
    // program's own stdout stream.
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

    // A test is a public, zero-parameter function in a test file. Each test keeps
    // its source file so a failure can be reported at its location.
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
    // A normal restore writes into an empty target; replace/merge/repair would
    // be explicit maintenance actions, which this command does not offer.
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

/// Inspect a project's saved data, read-only:
/// `marrow data <roots|stats|dump|integrity|get> <projectdir>`.
fn data(args: &[String]) -> ExitCode {
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
store. `diff` and `load` are deferred: they overlap restore's replace/merge/repair
modes and need typed source-fingerprinting; they will route through the
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
            // A full scan to count is fine for a local store; a bounded count
            // primitive on the backend would replace this if stores grow large.
            let records = match store.scan(&[], usize::MAX) {
                Ok(page) => page.entries.len(),
                Err(error) => {
                    report_simple_error(error.code(), &error.to_string(), format);
                    return ExitCode::FAILURE;
                }
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
    let entries = match &store {
        Some(store) => match store.scan(&[], usize::MAX) {
            Ok(page) => page.entries,
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), format);
                return ExitCode::FAILURE;
            }
        },
        None => Vec::new(),
    };
    match format {
        CheckFormat::Text => {
            if entries.is_empty() {
                println!("(no saved data)");
            } else {
                for (path, value) in &entries {
                    println!("{}\t{}", display_path(path), render_value_bytes(value));
                }
            }
        }
        CheckFormat::Json => {
            let records = entries.iter().map(dump_record).collect::<Vec<_>>();
            write_json(json!({ "project": dir, "records": records }));
        }
        CheckFormat::Jsonl => {
            for entry in &entries {
                write_json(dump_record(entry));
            }
            write_json(json!({ "kind": "summary", "records": entries.len() }));
        }
    }
    ExitCode::SUCCESS
}

/// Render a dump record as JSON: the human path plus base64 of the exact path and
/// value bytes, so a machine consumer reads them losslessly while a person reads
/// `path`. Uses the same base64 codec `serve` uses.
fn dump_record((path, value): &(Vec<u8>, Vec<u8>)) -> serde_json::Value {
    json!({
        "path": display_path(path),
        "path_b64": marrow_run::base64::encode(path),
        "value_b64": marrow_run::base64::encode(value),
    })
}

/// Render stored value bytes for raw text inspection: as a UTF-8 string when
/// valid (the common case, since canonical forms are ASCII text), else as
/// `0x<hex>`. This shows the canonical stored bytes honestly, never guessing a
/// type — dump and get work without source.
fn render_value_bytes(bytes: &[u8]) -> String {
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
    let entries = match &store {
        Some(store) => match store.scan(&[], usize::MAX) {
            Ok(page) => page.entries,
            Err(error) => {
                report_simple_error(error.code(), &error.to_string(), format);
                return ExitCode::FAILURE;
            }
        },
        None => Vec::new(),
    };

    let problems: Vec<IntegrityProblem> = entries
        .iter()
        .filter_map(|(path, value)| check_record(&program, path, value))
        .collect();

    report_integrity(&dir, entries.len(), &problems, format);
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
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
        // Generated index entries are raw-only by design; they are legal.
        marrow_run::SavedPathClass::IndexMarker => None,
        marrow_run::SavedPathClass::Orphan => Some(IntegrityProblem {
            code: "data.orphan",
            path: display_path(path),
            message: "saved data under an unknown root or undeclared member".into(),
        }),
    }
}

/// Report integrity findings in the chosen format. Text prints one line per
/// problem and a final `ok` line on a clean store; JSON wraps the problems in the
/// standard envelope; jsonl streams one envelope per problem plus a summary.
fn report_integrity(dir: &str, records: usize, problems: &[IntegrityProblem], format: CheckFormat) {
    match format {
        CheckFormat::Text => {
            if problems.is_empty() {
                println!("ok: {dir} integrity verified ({records} records)");
            } else {
                for problem in problems {
                    eprintln!("{}: {}: {}", problem.path, problem.code, problem.message);
                }
            }
        }
        CheckFormat::Json => {
            let records_json = problems.iter().map(integrity_record).collect::<Vec<_>>();
            write_json(json!({
                "project": dir,
                "records": records,
                "problems": records_json,
            }));
        }
        CheckFormat::Jsonl => {
            for problem in problems {
                write_json(integrity_record(problem));
            }
            write_json(json!({
                "kind": "summary",
                "records": records,
                "problems": problems.len(),
            }));
        }
    }
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

fn fmt(args: &[String]) -> ExitCode {
    let mut mode = FmtMode::Print;
    let mut target = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--check" => mode = FmtMode::Check,
            "--write" => mode = FmtMode::Write,
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow fmt [--check | --write] <file.mw | projectdir>

Format Marrow source. With a single `.mw` file and no flag, print the formatted
source to stdout. With a project directory (one that contains marrow.json),
format every `.mw` file under its source roots; a directory requires --check or
--write, since printing many files to stdout is meaningless. --check exits
non-zero if any file is not already formatted; --write rewrites changed files in
place. `marrow fmt` does not read from stdin.
"
                );
                return ExitCode::SUCCESS;
            }
            // A stdin pipe has no path to --write and no project to discover, so
            // reject it explicitly rather than mislabel `-` as an unknown option.
            "-" => {
                eprintln!("marrow fmt does not read from stdin; pass a file or project directory");
                return ExitCode::from(2);
            }
            value if value.starts_with('-') => {
                eprintln!("unknown fmt option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if target.replace(value.to_string()).is_some() {
                    eprintln!("marrow fmt accepts one source file or project directory");
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }

    let Some(target) = target else {
        eprintln!("missing source file or project directory");
        return ExitCode::from(2);
    };
    if Path::new(&target).is_dir() {
        return fmt_project_dir(&target, mode);
    }
    let source = match std::fs::read_to_string(&target) {
        Ok(source) => source,
        Err(error) => {
            report_io_error(&target, &error, CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    match fmt_one(&target, &source, mode) {
        Ok(FmtOutcome::Formatted) | Ok(FmtOutcome::Unchanged) => ExitCode::SUCCESS,
        Ok(FmtOutcome::NeedsFormatting) | Err(()) => ExitCode::FAILURE,
    }
}

/// Format every `.mw` file under a project's source roots. A directory requires a
/// mode: printing many files to stdout is meaningless, so bare `fmt <dir>` is a
/// usage error. A missing/invalid `marrow.json` is a typed `config.*` error
/// through `load_config`, not a raw OS "Is a directory".
fn fmt_project_dir(dir: &str, mode: FmtMode) -> ExitCode {
    if matches!(mode, FmtMode::Print) {
        eprintln!("marrow fmt on a directory requires --check or --write");
        return ExitCode::from(2);
    }
    let config = match load_config(dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let modules = match marrow_project::discover_modules(Path::new(dir), &config) {
        Ok(modules) => modules,
        Err(error) => {
            report_simple_error(error.code, &error.to_string(), CheckFormat::Text);
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    for module in &modules {
        let path = module.path.display().to_string();
        let source = match std::fs::read_to_string(&module.path) {
            Ok(source) => source,
            Err(error) => {
                report_io_error(&path, &error, CheckFormat::Text);
                failed = true;
                continue;
            }
        };
        match fmt_one(&path, &source, mode) {
            // A whole-project run reports every problem, then fails overall, so
            // the operator sees all unformatted or unparseable files at once.
            Ok(FmtOutcome::Formatted) | Ok(FmtOutcome::Unchanged) => {}
            Ok(FmtOutcome::NeedsFormatting) | Err(()) => failed = true,
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The result of formatting one file in `--check`/`--write` mode.
enum FmtOutcome {
    /// `--write`: the file was rewritten with new formatting.
    Formatted,
    /// `--check`/`--write`: already formatted, nothing to do.
    Unchanged,
    /// `--check`: the file is not formatted (a finding, not an error).
    NeedsFormatting,
}

/// Format one file's `source` in `mode`, reporting parse errors, `--check`
/// findings, and `--write` I/O failures. Source that does not parse is left
/// untouched and reported (`Err`). The `Print` mode writes to stdout (only valid
/// for a single file).
fn fmt_one(file: &str, source: &str, mode: FmtMode) -> Result<FmtOutcome, ()> {
    // Do not reformat source that does not parse; report its diagnostics and
    // leave the file untouched.
    let parsed = marrow_syntax::parse_source(source);
    if parsed.has_errors() {
        report_check(file, &parsed, CheckFormat::Text);
        return Err(());
    }
    let formatted = marrow_syntax::format_source(source);
    match mode {
        FmtMode::Print => {
            print!("{formatted}");
            Ok(FmtOutcome::Unchanged)
        }
        FmtMode::Check => {
            if source == formatted {
                Ok(FmtOutcome::Unchanged)
            } else {
                eprintln!("{file}: not formatted");
                Ok(FmtOutcome::NeedsFormatting)
            }
        }
        FmtMode::Write => {
            if source == formatted {
                Ok(FmtOutcome::Unchanged)
            } else if let Err(error) = std::fs::write(file, &formatted) {
                report_io_error(file, &error, CheckFormat::Text);
                Err(())
            } else {
                Ok(FmtOutcome::Formatted)
            }
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

fn write_json(value: serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string(&value).expect("JSON value should serialize")
    );
}
