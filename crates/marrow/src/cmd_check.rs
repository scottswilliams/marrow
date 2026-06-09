//! `marrow check`: parse and type-check a file or project, and the shared
//! located-fault renderer the runner reuses.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{CheckFormat, report_check, report_io_error, report_project, report_simple_error};

pub(crate) fn check(args: &[String]) -> ExitCode {
    let mut format = CheckFormat::Text;
    let mut saw_format = false;
    let mut data = false;
    let mut file = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--data" => data = true,
            "--format" => {
                if let Err(code) =
                    crate::parse_format_flag(args, &mut index, &mut saw_format, &mut format)
                {
                    return code;
                }
            }
            "--help" | "-h" => {
                print!(
                    "\
Usage:
  marrow check [--data] [--format text|json|jsonl] <file.mw | projectdir>

Parse a Marrow source file, or check a whole project directory (one that
contains marrow.json), and report diagnostics. With --data, attach the
project store read-only and prove data-evolution obligations.
"
                );
                return ExitCode::SUCCESS;
            }
            value if value.starts_with('-') => return crate::unknown_option("check", value),
            value => {
                if let Err(code) = crate::take_single_target(
                    &mut file,
                    value,
                    "check",
                    "source file or project directory",
                ) {
                    return code;
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
        return check_project_dir(&file, format, data);
    }
    if data {
        eprintln!("marrow check --data accepts a project directory");
        return ExitCode::from(2);
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
    // Parse errors carry byte offsets the project checker drops.
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
        // A scratch-directory I/O fault still reports the clean parse result rather
        // than failing a file that parsed.
        Err(error) => {
            report_io_error(&error.0.display().to_string(), &error.1, format);
            report_check(file, &parsed, format);
            ExitCode::FAILURE
        }
    }
}

/// Type-check a single parsed-clean file by synthesizing a one-file project in a
/// scratch directory and running the project checker over it, so a lone file reaches
/// the same `check.*` rules a project directory does. The file is placed at the path
/// its declared `module` implies (`a/b.mw` for `module a::b`; a module-less script
/// keeps its own stem), because the project checker validates module paths against
/// layout and would otherwise raise a spurious module-path diagnostic. Returns the
/// report with each diagnostic relocated to `file`.
fn check_one_file_project(
    file: &str,
    source: &str,
    module: Option<&marrow_syntax::ModuleDecl>,
) -> Result<marrow_check::CheckReport, (PathBuf, std::io::Error)> {
    let scratch = ScratchDir::new().map_err(|error| (std::env::temp_dir(), error))?;
    let root = scratch.path();
    // A library file (`module a::b`) must sit at `a/b.mw`; a module-less script is
    // path-free, so any `.mw` name under the root works.
    let relative = match module {
        Some(module) => {
            let mut path = PathBuf::new();
            for segment in module.name.split("::") {
                path.push(segment);
            }
            path.set_extension("mw");
            path
        }
        None => PathBuf::from("script.mw"),
    };
    // Create parent dirs and write the file, attributing each I/O fault to the path
    // that failed so a scratch-write error names the directory or file it could not create.
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
        accepted_catalog: "marrow.catalog.json".to_string(),
    };
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

/// A temp-dir scratch directory removed on drop, for handing the project checker a
/// real on-disk one-file project.
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
/// checker over its source roots and configured test files.
fn check_project_dir(dir: &str, format: CheckFormat, data: bool) -> ExitCode {
    if data {
        return crate::cmd_evolve::check_data(dir, format);
    }
    let config = match crate::load_config_with_format(dir, format) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let report = match marrow_check::analyze_project(
        Path::new(dir),
        &config,
        &marrow_check::ProjectSources::new(),
    ) {
        Ok(snapshot) => snapshot.report,
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

/// Report an uncaught runtime fault on stderr. When the fault carries an origin
/// file, it renders located — `file:line:col: code: message`, the same shape
/// `check` and `test` already print. A fault with no origin (for example, an
/// entry that never reached a project file) falls back to the bare
/// `code: message`, so nothing gains a spurious `:0:0:` location.
pub(crate) fn report_runtime_fault(
    program: &marrow_check::CheckedRuntimeProgram,
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
