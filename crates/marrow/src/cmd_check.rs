//! `marrow check`: parse and type-check a file or project, and the shared
//! located-fault renderer the runner reuses.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{CheckFormat, report_check, report_io_error, report_project, report_simple_error};

pub(crate) fn check(args: &[String]) -> ExitCode {
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut file = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--format" => {
                if saw_format {
                    eprintln!("duplicate --format");
                    return ExitCode::from(2);
                }
                saw_format = true;
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
    let (mut report, program) = match marrow_check::check_project(Path::new(dir), &config) {
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
    if !report.has_errors() && !config.tests.is_empty() {
        let (test_report, _test_modules) =
            match marrow_check::check_tests(Path::new(dir), &config, &program) {
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
        report.diagnostics.extend(test_report.diagnostics);
    }

    report_project(dir, &report, format);
    if report.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Report an uncaught runtime fault on stderr. When the fault carries an origin
/// file, it renders located — `file:line:col: code: message`, the same shape
/// `check` and `test` already print. A fault with no origin (the bare-program
/// path, or an entry that never reached a project file) falls back to the bare
/// `code: message`, so nothing gains a spurious `:0:0:` location.
pub(crate) fn report_runtime_fault(
    program: &marrow_check::CheckedProgram,
    error: &marrow_run::RuntimeError,
) {
    match error.origin.and_then(|id| program.file_path(id)) {
        Some(path) => eprintln!(
            "{}:{}:{}: {}: {}",
            path.display(),
            error.span.line,
            error.span.column,
            error.code,
            error.message
        ),
        None => report_simple_error(error.code, &error.message, CheckFormat::Text),
    }
}
